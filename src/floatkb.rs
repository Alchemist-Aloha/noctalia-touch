use crate::{
    hypr, icons,
    instance::Command,
    keyboard::{Action, Input, Key, Modifiers, Page, rows},
    shell, theme,
};
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const GAP: f32 = 5.;
const PAD: f32 = 8.;
const BAR: f32 = 44.;
/// Material 3 FAB: 56 dp square with a 16 dp corner and a 24 dp icon.
const FAB: f32 = 56.;
const MIN_WIDTH: f32 = 340.;
/// Panel and key alpha for each opacity step. Keys stay a little more opaque
/// than the panel so their outlines remain visible at the lowest step.
const OPACITIES: [(f32, f32); 4] = [(0.16, 0.30), (0.40, 0.52), (0.68, 0.76), (0.94, 0.96)];
/// Contact travel before a tap on the collapsed button turns into a drag.
const FAB_SLOP: f32 = 8.;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    left: i32,
    bottom: i32,
    unit: i32,
    opacity: usize,
    haptics: bool,
    expanded: bool,
    /// `#rrggbb`, or absent to follow the Noctalia theme.
    accent: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            left: 40,
            bottom: 40,
            unit: 44,
            opacity: 1,
            haptics: true,
            expanded: true,
            accent: None,
        }
    }
}
fn settings_path() -> Option<std::path::PathBuf> {
    Some(theme::xdg("XDG_CONFIG_HOME", ".config")?.join("noctalia-touch/keyboard-state.json"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Grip,
    Hide,
    Smaller,
    Bigger,
    Opacity,
    Vibrate,
    Accent,
    Key(usize, usize),
}

const TOOLS: [Target; 6] = [
    Target::Vibrate,
    Target::Accent,
    Target::Opacity,
    Target::Smaller,
    Target::Bigger,
    Target::Hide,
];

struct Contact {
    target: Target,
    origin: Point<Pixels>,
    start_offset: (i32, i32),
    steps: i32,
    sliding: bool,
    alt: bool,
    held: bool,
    moved: bool,
    token: u64,
}

fn page_index(page: Page) -> usize {
    match page {
        Page::Abc => 0,
        Page::Sym => 1,
        Page::More => 2,
    }
}

pub struct Keyboard {
    settings: Settings,
    input: Input,
    /// Every layout page, built once; `page` selects the visible one.
    pages: [Vec<Vec<Key>>; 3],
    page: usize,
    palette: theme::Palette,
    accent: theme::Accent,
    mods: Modifiers,
    caps: bool,
    last_shift: Option<Instant>,
    monitor: Option<(i32, i32)>,
    touches: HashMap<u64, Contact>,
    serial: u64,
    /// Effective key unit and expanded size, cached by `relayout`.
    unit: f32,
    size: (f32, f32),
    /// Margins last sent to the compositor.
    applied: (i32, i32),
}

impl Keyboard {
    fn new(collapsed: bool, accent: Option<Option<u32>>) -> Self {
        let mut settings: Settings = settings_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        settings.unit = settings.unit.clamp(28, 72);
        settings.opacity = settings.opacity.min(OPACITIES.len() - 1);
        if collapsed {
            settings.expanded = false;
        }
        if let Some(accent) = accent {
            settings.accent = accent.map(|rgb| format!("#{rgb:06x}"));
        }
        let palette = theme::palette();
        let mut this = Self {
            accent: palette.accent(None),
            settings,
            input: Input::new(),
            pages: [rows(Page::Abc), rows(Page::Sym), rows(Page::More)],
            page: 0,
            palette,
            mods: Modifiers::default(),
            caps: false,
            last_shift: None,
            monitor: hypr::monitor_size(),
            touches: HashMap::new(),
            serial: 0,
            unit: 0.,
            size: (0., 0.),
            applied: (0, 0),
        };
        this.refresh_accent();
        this.relayout();
        this.applied = this.position();
        this
    }

    fn keys(&self) -> &[Vec<Key>] {
        &self.pages[self.page]
    }

    fn custom_accent(&self) -> Option<u32> {
        self.settings.accent.as_deref().and_then(theme::parse_hex)
    }
    fn refresh_accent(&mut self) {
        self.accent = self.palette.accent(self.custom_accent());
    }
    fn set_accent(&mut self, accent: Option<u32>, cx: &mut Context<Self>) {
        self.settings.accent = accent.map(|rgb| format!("#{rgb:06x}"));
        self.refresh_accent();
        self.save();
        cx.notify();
    }
    /// Theme color, then each preset, then back to the theme color.
    fn next_accent(&self) -> Option<u32> {
        let presets: Vec<u32> = (0..theme::ACCENT_PRESETS.len())
            .map(|i| self.palette.preset(i))
            .collect();
        match self.custom_accent() {
            None => presets.first().copied(),
            Some(current) => match presets.iter().position(|&p| p == current) {
                Some(i) => presets.get(i + 1).copied(),
                None => presets.first().copied(),
            },
        }
    }

    fn save(&self) {
        let result = (|| -> anyhow::Result<()> {
            let path = settings_path()
                .ok_or_else(|| anyhow::anyhow!("missing HOME or XDG_CONFIG_HOME"))?;
            let dir = path.parent().expect("state parent");
            std::fs::create_dir_all(dir)?;
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            serde_json::to_writer(tmp.as_file_mut(), &self.settings)?;
            tmp.flush()?;
            tmp.persist(path)?;
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("floatkb: could not save settings: {e}");
        }
    }

    /// Recompute cached metrics after the unit, monitor or page changes.
    fn relayout(&mut self) {
        let u = self.settings.unit as f32;
        self.unit = match self.monitor {
            None => u,
            Some((w, h)) => u
                .min(((w as f32 - 2. * PAD - 10. * GAP) / 11.).floor())
                .min(((h as f32 - 152.) / 5.2).floor())
                .max(18.),
        };
        let width = self
            .pages
            .iter()
            .flatten()
            .map(|row| self.row_width(row))
            .fold(MIN_WIDTH, f32::max)
            + 2. * PAD;
        let height = 2. * PAD
            + BAR
            + (0..self.keys().len())
                .map(|i| self.row_height(i) + GAP)
                .sum::<f32>();
        self.size = (width, height);
    }
    fn row_height(&self, i: usize) -> f32 {
        (self.unit * if i == 0 { 0.83 } else { 1.08 }).round()
    }
    fn key_width(&self, key: &Key) -> f32 {
        ((self.unit + GAP) * key.width - GAP).round()
    }
    fn row_width(&self, row: &[Key]) -> f32 {
        row.iter().map(|k| self.key_width(k) + GAP).sum::<f32>() - GAP
    }
    fn dimensions(&self) -> (f32, f32) {
        if self.settings.expanded {
            self.size
        } else {
            (FAB, FAB)
        }
    }
    fn position(&self) -> (i32, i32) {
        let (w, h) = self.dimensions();
        let (mw, mh) = self.monitor.unwrap_or((i32::MAX, i32::MAX));
        (
            self.settings.left.clamp(0, (mw - w as i32).max(0)),
            self.settings.bottom.clamp(0, (mh - h as i32 - 56).max(0)),
        )
    }
    fn geometry(&mut self, window: &mut Window, resize: bool) {
        let position = self.position();
        // Each move is a surface commit; skip the ones that would not move it.
        if position != self.applied {
            shell::move_layer(window, position.0, position.1);
            self.applied = position;
        }
        if resize {
            let (w, h) = self.dimensions();
            window.resize(size(px(w), px(h)));
        }
    }
    fn set_expanded(&mut self, expanded: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.expanded == expanded {
            return;
        }
        self.settings.expanded = expanded;
        self.touches.clear();
        if !expanded {
            self.mods = Modifiers::default();
            self.caps = false;
            self.last_shift = None;
        }
        self.geometry(window, true);
        self.save();
        cx.notify();
    }
    fn buzz(&self) {
        if self.settings.haptics {
            self.input.buzz();
        }
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Shift => {
                let now = Instant::now();
                if self.caps {
                    self.caps = false;
                    self.mods.shift = false;
                } else if self.mods.shift
                    && self
                        .last_shift
                        .is_some_and(|t| now.duration_since(t) < Duration::from_millis(400))
                {
                    self.caps = true;
                } else {
                    self.mods.shift = !self.mods.shift;
                }
                self.last_shift = Some(now);
            }
            Action::Ctrl => self.mods.ctrl = !self.mods.ctrl,
            Action::Alt => self.mods.alt = !self.mods.alt,
            Action::Super => self.mods.logo = !self.mods.logo,
            Action::Page(page) => {
                self.page = page_index(page);
                self.touches.clear();
            }
            action => {
                let mut mods = self.mods;
                if let Action::Char(ch) = action
                    && !(mods.ctrl || mods.alt || mods.logo)
                {
                    let ch = if mods.shift || self.caps {
                        ch.to_ascii_uppercase()
                    } else {
                        ch
                    };
                    mods.shift = false;
                    self.input.send(Action::Char(ch), mods);
                } else {
                    mods.shift &= !self.caps;
                    self.input.send(action, mods);
                }
                self.mods = Modifiers::default();
            }
        }
    }

    fn tools(&self) -> impl DoubleEndedIterator<Item = Target> + '_ {
        TOOLS
            .into_iter()
            .filter(|&t| t != Target::Vibrate || self.input.has_haptics())
    }
    fn tool_label(target: Target) -> &'static str {
        match target {
            Target::Vibrate => "〰",
            Target::Opacity => "◐",
            Target::Smaller => "−",
            Target::Bigger => "+",
            Target::Hide => "Hide",
            _ => "",
        }
    }
    fn tool_width(target: Target) -> f32 {
        if matches!(target, Target::Hide) {
            61.
        } else {
            42.
        }
    }
    fn target(&self, p: Point<Pixels>) -> Option<Target> {
        let (x, y) = (f32::from(p.x), f32::from(p.y));
        let (w, h) = self.dimensions();
        if !(0. ..w).contains(&x) || !(0. ..h).contains(&y) {
            return None;
        }
        if !self.settings.expanded {
            return Some(Target::Hide);
        }
        if (PAD..PAD + BAR).contains(&y) {
            let mut right = w - PAD;
            for target in self.tools().rev() {
                let left = right - Self::tool_width(target);
                if (left..right).contains(&x) {
                    return Some(target);
                }
                if (left - GAP..left).contains(&x) {
                    return None;
                }
                right = left - GAP;
            }
            return (x >= PAD).then_some(Target::Grip);
        }
        let mut top = PAD + BAR + GAP;
        for (i, row) in self.keys().iter().enumerate() {
            if (top..top + self.row_height(i)).contains(&y) {
                let mut left = (w - self.row_width(row)) / 2.;
                for (j, key) in row.iter().enumerate() {
                    let width = self.key_width(key);
                    if (left..left + width).contains(&x) {
                        return Some(Target::Key(i, j));
                    }
                    left += width + GAP;
                }
                return None;
            }
            top += self.row_height(i) + GAP;
        }
        None
    }

    fn start(&mut self, id: u64, p: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.target(p) else {
            return false;
        };
        if (target == Target::Grip || target == Target::Hide && !self.settings.expanded)
            && self.touches.values().any(|c| c.target == target)
        {
            return false;
        }
        self.serial = self.serial.wrapping_add(1);
        let token = self.serial;
        self.touches.insert(
            id,
            Contact {
                target,
                origin: p,
                start_offset: self.applied,
                steps: 0,
                sliding: false,
                alt: false,
                held: false,
                moved: false,
                token,
            },
        );
        if let Target::Key(i, j) = target {
            let key = &self.keys()[i][j];
            let action = key.action;
            let hint = key.hint.is_some();
            self.buzz();
            if hint {
                cx.spawn(async move |entity, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(380))
                        .await;
                    let _ = entity.update(cx, |this, cx| {
                        if let Some(c) = this
                            .touches
                            .get_mut(&id)
                            .filter(|c| c.token == token && !c.alt && !c.moved)
                        {
                            c.held = true;
                            c.alt = true;
                            this.buzz();
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            if repeatable(action) {
                self.act(action);
                cx.spawn(async move |entity, cx| {
                    let mut delay = 400;
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(delay))
                            .await;
                        let active = entity
                            .update(cx, |this, cx| {
                                if this.touches.get(&id).is_some_and(|c| c.token == token) {
                                    this.act(action);
                                    cx.notify();
                                    true
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);
                        if !active {
                            break;
                        }
                        delay = 65;
                    }
                })
                .detach();
            }
        }
        cx.notify();
        true
    }

    fn motion(&mut self, id: u64, p: Point<Pixels>, w: &mut Window, cx: &mut Context<Self>) {
        let expanded = self.settings.expanded;
        let Some(c) = self.touches.get_mut(&id) else {
            return;
        };
        let dx = f32::from(p.x - c.origin.x);
        let dy = f32::from(p.y - c.origin.y);
        match c.target {
            Target::Grip | Target::Hide if c.target == Target::Grip || !expanded => {
                // The grip has no tap action, so it follows the finger from the first
                // event. Hyprland (and the tablet-swipes plugin) report contact
                // positions relative to the surface origin at touch-down, not to its
                // current position, so the travel is offset from the starting margins.
                let slop = if c.target == Target::Grip {
                    0.
                } else {
                    FAB_SLOP
                };
                if c.moved || dx.abs() + dy.abs() > slop {
                    let first = !c.moved;
                    c.moved = true;
                    let (left, bottom) = c.start_offset;
                    self.settings.left = left.saturating_add(dx.round() as i32);
                    self.settings.bottom = bottom.saturating_sub(dy.round() as i32);
                    self.geometry(w, false);
                    if first && !expanded {
                        // Drop the pressed state layer once the tap became a drag.
                        cx.notify();
                    }
                }
            }
            Target::Key(i, j) => {
                let unit = self.unit;
                let key = &self.pages[self.page][i][j];
                if key.action == Action::Space {
                    let steps = (dx / (unit * 0.45)) as i32;
                    let difference = steps.saturating_sub(c.steps).clamp(-64, 64);
                    if difference != 0 {
                        c.sliding = true;
                        c.steps = steps;
                        let arrow = if difference > 0 { "Right" } else { "Left" };
                        for _ in 0..difference.abs() {
                            self.input.send(Action::Key(arrow), self.mods);
                        }
                        self.buzz();
                        cx.notify();
                    }
                } else if key.hint.is_some() && !c.held {
                    let alt = -dy >= unit * 0.4;
                    if c.alt != alt {
                        c.alt = alt;
                        c.moved |= alt;
                        if alt {
                            self.buzz();
                        }
                        cx.notify();
                    }
                }
            }
            _ => (),
        }
    }

    fn end(
        &mut self,
        id: u64,
        p: Point<Pixels>,
        cancel: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.touches.remove(&id) else {
            return;
        };
        if cancel && c.moved && matches!(c.target, Target::Grip | Target::Hide) {
            (self.settings.left, self.settings.bottom) = c.start_offset;
            self.geometry(window, false);
        }
        if !cancel {
            let released_on = |this: &Self| this.target(p) == Some(c.target);
            match c.target {
                Target::Key(i, j) => {
                    let key = &self.keys()[i][j];
                    if !c.sliding && !repeatable(key.action) {
                        let action = if c.alt {
                            key.hint.as_ref().map_or(key.action, |h| h.1)
                        } else {
                            key.action
                        };
                        self.act(action);
                    }
                }
                Target::Grip | Target::Hide if c.moved => {
                    (self.settings.left, self.settings.bottom) = self.applied;
                    self.save();
                }
                Target::Grip => (),
                Target::Hide if released_on(self) => {
                    self.set_expanded(!self.settings.expanded, window, cx)
                }
                Target::Smaller | Target::Bigger if released_on(self) => {
                    let diff = if matches!(c.target, Target::Smaller) {
                        -4
                    } else {
                        4
                    };
                    self.settings.unit = (self.unit as i32 + diff).clamp(28, 72);
                    self.relayout();
                    self.geometry(window, true);
                    self.save();
                }
                Target::Opacity if released_on(self) => {
                    self.settings.opacity = (self.settings.opacity + 1) % OPACITIES.len();
                    self.save();
                }
                Target::Vibrate if released_on(self) => {
                    self.settings.haptics = !self.settings.haptics;
                    self.buzz();
                    self.save();
                }
                Target::Accent if released_on(self) => {
                    let next = self.next_accent();
                    self.set_accent(next, cx);
                }
                _ => (),
            }
        }
        cx.notify();
    }

    fn pressed(&self, target: Target) -> bool {
        self.touches.values().any(|c| c.target == target)
    }
}

fn repeatable(action: Action) -> bool {
    matches!(
        action,
        Action::Key("BackSpace" | "Delete" | "Left" | "Right" | "Up" | "Down")
    )
}

fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha.clamp(0., 1.);
    color
}

impl Render for Keyboard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let input = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                window.on_mouse_event({
                    let weak = weak.clone();
                    move |e: &MouseDownEvent, phase, w, cx| {
                        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                            let accepted = weak
                                .update(cx, |this, cx| this.start(0, e.position, cx))
                                .unwrap_or(false);
                            if accepted {
                                w.prevent_default();
                            }
                        }
                    }
                });
                window.on_mouse_event({
                    let weak = weak.clone();
                    move |e: &MouseMoveEvent, phase, w, cx| {
                        if phase == DispatchPhase::Bubble && e.pressed_button.is_some() {
                            let _ = weak.update(cx, |this, cx| this.motion(0, e.position, w, cx));
                        }
                    }
                });
                window.on_mouse_event({
                    let weak = weak.clone();
                    move |e: &MouseUpEvent, phase, w, cx| {
                        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                            let _ =
                                weak.update(cx, |this, cx| this.end(0, e.position, false, w, cx));
                        }
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let accent = self.accent;
        if !self.settings.expanded {
            // Material 3 FAB with a state layer while pressed (and not dragging).
            let pressed = self
                .touches
                .values()
                .any(|c| c.target == Target::Hide && !c.moved);
            let container = if pressed {
                theme::mix(accent.container, accent.on_container, 0.12)
            } else {
                accent.container
            };
            return div()
                .size(px(FAB))
                .relative()
                .child(input)
                .rounded(px(16.))
                .bg(with_alpha(container, 0.94))
                .border_1()
                .border_color(with_alpha(accent.on_container, 0.16))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(icons::KEYBOARD)
                        .size(px(24.))
                        .text_color(accent.on_container),
                );
        }

        let (surface_alpha, key_alpha) = OPACITIES[self.settings.opacity];
        let surface = with_alpha(accent.surface, surface_alpha);
        // Transparent keys need an outline to stay distinct from what is behind them.
        let outline = with_alpha(accent.outline, if key_alpha < 0.6 { 0.30 } else { 0.16 });
        let tonal = with_alpha(accent.key, key_alpha);
        // Pressed and latched keys keep enough fill for their label to read.
        let fill = with_alpha(accent.fill, (key_alpha + 0.4).min(1.));
        let enter = with_alpha(accent.fill, key_alpha.max(0.62));
        // Labels take the accent (the theme primary unless customized).
        let label = accent.on_container;
        let hint_color = accent.outline;
        let unit = self.unit;
        let label_size = px((unit * 0.42).clamp(11., 26.));
        let hint_size = px((unit * 0.23).clamp(8., 13.));
        let mut root = div()
            .size_full()
            .relative()
            .child(input)
            .flex()
            .flex_col()
            .gap(px(GAP))
            .p(px(PAD))
            .rounded(px(24.))
            .bg(surface)
            .border_1()
            .border_color(with_alpha(accent.outline, 0.24))
            .text_color(accent.on_container);
        let mut bar = div().flex().h(px(BAR)).items_center().gap(px(GAP)).child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_move()
                .child(div().w(px(38.)).h(px(4.)).rounded_full().bg(
                    if self.pressed(Target::Grip) {
                        accent.fill
                    } else {
                        accent.outline
                    },
                )),
        );
        for target in self.tools() {
            let active =
                self.pressed(target) || matches!(target, Target::Vibrate) && self.settings.haptics;
            let mut tool = div()
                .w(px(Self::tool_width(target)))
                .h(px(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(outline)
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .bg(if active { fill } else { tonal })
                .text_color(if active { accent.on_fill } else { label });
            tool = if target == Target::Accent {
                tool.child(
                    div()
                        .size(px(18.))
                        .rounded_full()
                        .bg(accent.fill)
                        .border_2()
                        .border_color(with_alpha(accent.on_container, 0.7)),
                )
            } else {
                tool.child(Self::tool_label(target))
            };
            bar = bar.child(tool);
        }
        root = root.child(bar);
        let shifted = self.mods.shift || self.caps;
        for (i, row) in self.keys().iter().enumerate() {
            let mut line = div()
                .flex()
                .justify_center()
                .gap(px(GAP))
                .h(px(self.row_height(i)));
            for (j, key) in row.iter().enumerate() {
                let c = self
                    .touches
                    .values()
                    .find(|c| c.target == Target::Key(i, j));
                let alt = c.is_some_and(|c| c.alt);
                let latched = match key.action {
                    Action::Shift => shifted,
                    Action::Ctrl => self.mods.ctrl,
                    Action::Alt => self.mods.alt,
                    Action::Super => self.mods.logo,
                    _ => false,
                };
                let active = c.is_some() || latched;
                let (bg, fg) = if active {
                    (fill, accent.on_fill)
                } else if key.action == Action::Key("Return") {
                    (enter, accent.on_fill)
                } else {
                    (tonal, label)
                };
                let label = if alt {
                    key.hint
                        .as_ref()
                        .map_or_else(|| key.label.clone(), |h| h.0.clone())
                } else if self.caps && key.action == Action::Shift {
                    SharedString::new_static("⇪")
                } else if shifted {
                    key.upper.clone()
                } else {
                    key.label.clone()
                };
                let mut tile = div()
                    .relative()
                    .flex_none()
                    .w(px(self.key_width(key)))
                    .h_full()
                    .rounded(px(12.))
                    .bg(bg)
                    .border_1()
                    .border_color(outline)
                    .text_color(fg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(label_size)
                    .child(label);
                if let Some((hint, _)) = &key.hint
                    && !alt
                    && c.is_none()
                {
                    tile = tile.child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .right(px(4.))
                            .text_color(hint_color)
                            .text_size(hint_size)
                            .child(hint.clone()),
                    );
                }
                line = line.child(tile);
            }
            root = root.child(line);
        }
        root
    }
}

/// Drop touch motion that a later motion of the same contact supersedes, so a
/// backlog costs one layout or surface move per contact instead of one per event.
/// Every start, end and cancel is kept, and motion is never moved across them.
fn coalesce(events: &mut Vec<TouchEvent>) {
    let mut later_motion = HashSet::new();
    let mut keep = vec![true; events.len()];
    for (i, e) in events.iter().enumerate().rev() {
        if e.phase == TouchPhase::Moved {
            keep[i] = later_motion.insert(e.id);
        } else {
            later_motion.remove(&e.id);
        }
    }
    let mut keep = keep.into_iter();
    events.retain(|_| keep.next().unwrap_or(true));
}

pub fn run(
    cx: &mut App,
    collapsed: bool,
    accent: Option<Option<u32>>,
    commands: async_channel::Receiver<Command>,
) -> anyhow::Result<()> {
    let model = Keyboard::new(collapsed, accent);
    if accent.is_some() {
        model.save();
    }
    let (w, h) = model.dimensions();
    let position = model.applied;
    let handle = shell::open(cx, "floatkb", w, h, Some(position), |_, cx| {
        cx.new(|_| model)
    })?;
    let (touch_tx, touch_rx) = async_channel::unbounded();
    handle.update(cx, |_, window, _| {
        gpui_fast_linux::set_raw_touch_handler(
            window,
            Some(std::rc::Rc::new(move |event| {
                touch_tx.try_send(event.clone()).is_ok()
            })),
        );
    })?;
    cx.spawn(async move |cx| {
        let mut batch = Vec::new();
        while let Ok(first) = touch_rx.recv().await {
            batch.push(first);
            while let Ok(e) = touch_rx.try_recv() {
                batch.push(e);
            }
            coalesce(&mut batch);
            let alive = handle.update(cx, |this, w, cx| {
                for e in batch.drain(..) {
                    let id = e.id.0.wrapping_add(1);
                    match e.phase {
                        TouchPhase::Started => {
                            this.start(id, e.position, cx);
                        }
                        TouchPhase::Moved => this.motion(id, e.position, w, cx),
                        TouchPhase::Ended => this.end(id, e.position, false, w, cx),
                        TouchPhase::Cancelled => this.end(id, e.position, true, w, cx),
                    }
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
    let events = watch_events();
    cx.spawn(async move |cx| {
        while let Ok(cmd) = commands.recv().await {
            if handle
                .update(cx, |this, w, cx| match cmd {
                    Command::Toggle => this.set_expanded(!this.settings.expanded, w, cx),
                    Command::Expand => this.set_expanded(true, w, cx),
                    Command::Accent(accent) => this.set_accent(accent, cx),
                    Command::Close => cx.quit(),
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(event) = events.recv().await {
            if handle
                .update(cx, |this, w, cx| match event {
                    OverlayEvent::Raise => {
                        shell::raise_layer(w);
                    }
                    OverlayEvent::Palette(palette) => {
                        this.palette = palette;
                        this.refresh_accent();
                        cx.notify();
                    }
                    OverlayEvent::Monitor(dimensions) => {
                        if this.monitor != dimensions {
                            this.monitor = dimensions;
                            this.relayout();
                            this.geometry(w, true);
                            cx.notify();
                        }
                    }
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    Ok(())
}

enum OverlayEvent {
    Raise,
    Monitor(Option<(i32, i32)>),
    Palette(theme::Palette),
}
fn watch_events() -> async_channel::Receiver<OverlayEvent> {
    let (tx, rx) = async_channel::bounded(16);
    // Noctalia rewrites the color template when the theme or wallpaper changes.
    // A once-a-second stat is cheaper than keeping an inotify watch on the cache dir.
    let colors = tx.clone();
    std::thread::spawn(move || {
        let modified = || {
            theme::palette_path()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok())
        };
        let mut last = modified();
        while !colors.is_closed() {
            std::thread::sleep(Duration::from_secs(1));
            let now = modified();
            if now != last {
                last = now;
                let _ = colors.try_send(OverlayEvent::Palette(theme::palette()));
            }
        }
    });
    std::thread::spawn(move || {
        while !tx.is_closed() {
            if let Ok(path) = hypr::socket_path(true)
                && let Ok(socket) = UnixStream::connect(path)
            {
                for event in BufReader::new(socket).lines().map_while(Result::ok) {
                    if tx.is_closed() {
                        return;
                    }
                    if event == "openlayer>>noctalia-panel" {
                        std::thread::sleep(Duration::from_millis(30));
                        let _ = tx.try_send(OverlayEvent::Raise);
                    } else if event.starts_with("monitoradded")
                        || event.starts_with("monitorremoved")
                        || event == "configreloaded>>"
                        || event.starts_with("openlayer>>noctalia-bar-")
                    {
                        let _ = tx.try_send(OverlayEvent::Monitor(hypr::monitor_size()));
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::coalesce;
    use gpui_kit::{TouchEvent, TouchId, TouchPhase, point, px};

    fn touch(id: u64, phase: TouchPhase, x: f32) -> TouchEvent {
        TouchEvent {
            id: TouchId(id),
            phase,
            position: point(px(x), px(0.)),
            ..Default::default()
        }
    }

    #[test]
    fn coalescing_keeps_last_motion_and_every_boundary() {
        use TouchPhase::*;
        let mut events = vec![
            touch(1, Started, 0.),
            touch(1, Moved, 1.),
            touch(2, Moved, 5.),
            touch(1, Moved, 2.),
            touch(1, Ended, 2.),
            touch(1, Started, 9.),
            touch(1, Moved, 10.),
            touch(2, Moved, 6.),
        ];
        coalesce(&mut events);
        let got: Vec<_> = events
            .iter()
            .map(|e| (e.id.0, e.phase, f32::from(e.position.x)))
            .collect();
        assert_eq!(
            got,
            [
                (1, Started, 0.),
                (1, Moved, 2.),
                (1, Ended, 2.),
                (1, Started, 9.),
                (1, Moved, 10.),
                (2, Moved, 6.),
            ]
        );
    }
}
