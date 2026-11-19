use crate::{
    hypr,
    instance::Command,
    keyboard::{Action, Input, Key, Modifiers, Page, rows},
    shell, theme,
};
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const GAP: f32 = 5.;
const PAD: f32 = 8.;
const BAR: f32 = 44.;
const PILL: f32 = 48.;
const OPACITIES: [f32; 4] = [0.28, 0.50, 0.73, 0.94];

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    left: i32,
    bottom: i32,
    unit: i32,
    opacity: usize,
    haptics: bool,
    expanded: bool,
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
    Key(usize, usize),
}

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

pub struct Keyboard {
    settings: Settings,
    input: Input,
    keys: Vec<Vec<Key>>,
    palette: theme::Palette,
    mods: Modifiers,
    caps: bool,
    last_shift: Option<Instant>,
    monitor: Option<(i32, i32)>,
    touches: HashMap<u64, Contact>,
    serial: u64,
}

impl Keyboard {
    fn new(collapsed: bool) -> Self {
        let mut settings: Settings = settings_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        settings.unit = settings.unit.clamp(28, 72);
        settings.opacity = settings.opacity.min(OPACITIES.len() - 1);
        if collapsed {
            settings.expanded = false;
        }
        Self {
            settings,
            input: Input::new(),
            keys: rows(Page::Abc),
            palette: theme::palette(),
            mods: Modifiers::default(),
            caps: false,
            last_shift: None,
            monitor: hypr::monitor_size(),
            touches: HashMap::new(),
            serial: 0,
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

    fn unit(&self) -> f32 {
        let u = self.settings.unit as f32;
        let Some((w, h)) = self.monitor else {
            return u;
        };
        u.min(((w as f32 - 2. * PAD - 10. * GAP) / 11.).floor())
            .min(((h as f32 - 152.) / 5.2).floor())
            .max(18.)
    }
    fn row_height(&self, i: usize) -> f32 {
        (self.unit() * if i == 0 { 0.83 } else { 1.08 }).round()
    }
    fn key_width(&self, key: &Key) -> f32 {
        ((self.unit() + GAP) * key.width - GAP).round()
    }
    fn row_width(&self, row: &[Key]) -> f32 {
        row.iter().map(|k| self.key_width(k) + GAP).sum::<f32>() - GAP
    }
    fn dimensions(&self) -> (f32, f32) {
        if !self.settings.expanded {
            return (PILL, PILL);
        }
        let width = [Page::Abc, Page::Sym, Page::More]
            .into_iter()
            .flat_map(rows)
            .map(|row| self.row_width(&row))
            .fold(310., f32::max)
            + 2. * PAD;
        let height = 2. * PAD
            + BAR
            + (0..self.keys.len())
                .map(|i| self.row_height(i) + GAP)
                .sum::<f32>();
        (width, height)
    }
    fn position(&self) -> (i32, i32) {
        let (w, h) = self.dimensions();
        let (mw, mh) = self.monitor.unwrap_or((i32::MAX, i32::MAX));
        (
            self.settings.left.clamp(0, (mw - w as i32).max(0)),
            self.settings.bottom.clamp(0, (mh - h as i32 - 56).max(0)),
        )
    }
    fn geometry(&self, window: &mut Window, resize: bool) {
        let (x, y) = self.position();
        shell::move_layer(window, x, y);
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
                self.keys = rows(page);
                self.touches.clear();
            }
            action => {
                let mut mods = self.mods;
                if matches!(action, Action::Char(_)) && !(mods.ctrl || mods.alt || mods.logo) {
                    let Action::Char(ch) = action else {
                        unreachable!()
                    };
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

    fn tools(&self) -> Vec<(Target, &'static str, &'static str)> {
        let mut tools = Vec::new();
        if self.input.has_haptics() {
            tools.push((Target::Vibrate, "〰", "Key vibration"));
        }
        tools.extend([
            (Target::Opacity, "◐", "Keyboard opacity"),
            (Target::Smaller, "−", "Smaller keys"),
            (Target::Bigger, "+", "Bigger keys"),
            (Target::Hide, "Hide", "Collapse keyboard"),
        ]);
        tools
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
            for (target, _, _) in self.tools().into_iter().rev() {
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
        for (i, row) in self.keys.iter().enumerate() {
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
                start_offset: self.position(),
                steps: 0,
                sliding: false,
                alt: false,
                held: false,
                moved: false,
                token,
            },
        );
        if let Target::Key(i, j) = target {
            let key = &self.keys[i][j];
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
        let unit = self.unit();
        let pos = self.position();
        let Some(c) = self.touches.get_mut(&id) else {
            return;
        };
        let dx = f32::from(p.x - c.origin.x);
        let dy = f32::from(p.y - c.origin.y);
        match c.target {
            Target::Grip | Target::Hide
                if matches!(c.target, Target::Grip) || !self.settings.expanded =>
            {
                if c.moved || dx.abs() + dy.abs() > 8. {
                    c.moved = true;
                    self.settings.left = pos.0.saturating_add(dx.round() as i32);
                    self.settings.bottom = pos.1.saturating_sub(dy.round() as i32);
                    self.geometry(w, false);
                }
            }
            Target::Key(i, j) => {
                let key = &self.keys[i][j];
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
            match c.target {
                Target::Key(i, j) => {
                    let key = &self.keys[i][j];
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
                    (self.settings.left, self.settings.bottom) = self.position();
                    self.save();
                }
                Target::Grip => (),
                Target::Hide if matches!(self.target(p), Some(Target::Hide)) => {
                    self.set_expanded(!self.settings.expanded, window, cx)
                }
                Target::Smaller | Target::Bigger if self.target(p) == Some(c.target) => {
                    let diff = if matches!(c.target, Target::Smaller) {
                        -4
                    } else {
                        4
                    };
                    self.settings.unit = (self.unit() as i32 + diff).clamp(28, 72);
                    self.geometry(window, true);
                    self.save();
                }
                Target::Opacity if self.target(p) == Some(c.target) => {
                    self.settings.opacity = (self.settings.opacity + 1) % OPACITIES.len();
                    self.save();
                }
                Target::Vibrate if self.target(p) == Some(c.target) => {
                    self.settings.haptics = !self.settings.haptics;
                    self.buzz();
                    self.save();
                }
                _ => (),
            }
        }
        cx.notify();
    }
}

fn repeatable(action: Action) -> bool {
    matches!(
        action,
        Action::Key("BackSpace" | "Delete" | "Left" | "Right" | "Up" | "Down")
    )
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
                        if phase == DispatchPhase::Bubble {
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

        let p = self.palette;
        if !self.settings.expanded {
            return div()
                .size(px(PILL))
                .relative()
                .child(input)
                .rounded_full()
                .bg(p.tonal)
                .text_color(p.on_tonal)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(24.))
                .child("⌨");
        }
        let mut surface = p.surface;
        surface.a = OPACITIES[self.settings.opacity];
        let mut stroke = p.muted;
        stroke.a = 0.18;
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
            .shadow_sm()
            .border_color(stroke)
            .text_color(p.text);
        let mut bar = div().flex().h(px(BAR)).items_center().gap(px(GAP)).child(
            div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_move()
                .child(div().w(px(38.)).h(px(4.)).rounded_full().bg(p.muted)),
        );
        for (target, label, _) in self.tools() {
            let pressed = self
                .touches
                .values()
                .any(|c| std::mem::discriminant(&c.target) == std::mem::discriminant(&target));
            let active = pressed || matches!(target, Target::Vibrate) && self.settings.haptics;
            bar = bar.child(
                div()
                    .w(px(Self::tool_width(target)))
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .text_size(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .bg(if active { p.primary } else { p.tonal })
                    .text_color(if active { p.on_primary } else { p.on_tonal })
                    .child(label),
            );
        }
        root = root.child(bar);
        for (i, row) in self.keys.iter().enumerate() {
            let mut line = div()
                .flex()
                .justify_center()
                .gap(px(GAP))
                .h(px(self.row_height(i)));
            for (j, key) in row.iter().enumerate() {
                let c = self
                    .touches
                    .values()
                    .find(|c| matches!(c.target,Target::Key(a,b) if a==i && b==j));
                let alt = c.is_some_and(|c| c.alt);
                let latched = match key.action {
                    Action::Shift => self.mods.shift || self.caps,
                    Action::Ctrl => self.mods.ctrl,
                    Action::Alt => self.mods.alt,
                    Action::Super => self.mods.logo,
                    _ => false,
                };
                let active = c.is_some() || latched || matches!(key.action, Action::Key("Return"));
                let label = if alt {
                    key.hint
                        .as_ref()
                        .map(|h| h.0.clone())
                        .unwrap_or_else(|| key.label.clone())
                } else if self.caps && key.action == Action::Shift {
                    "⇪".into()
                } else if (self.mods.shift || self.caps) && matches!(key.action, Action::Char(_)) {
                    key.label.to_ascii_uppercase()
                } else {
                    key.label.clone()
                };
                let mut tile = div()
                    .relative()
                    .flex_none()
                    .w(px(self.key_width(key)))
                    .h_full()
                    .rounded(px(12.))
                    .bg(if active { p.primary } else { p.tonal })
                    .text_color(if active { p.on_primary } else { p.on_tonal })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px((self.unit() * 0.42).clamp(11., 26.)))
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
                            .text_color(p.muted)
                            .text_size(px((self.unit() * 0.23).clamp(8., 13.)))
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

pub fn run(
    cx: &mut App,
    collapsed: bool,
    commands: async_channel::Receiver<Command>,
) -> anyhow::Result<()> {
    let model = Keyboard::new(collapsed);
    let (w, h) = model.dimensions();
    let position = model.position();
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
        while let Ok(e) = touch_rx.recv().await {
            let id = e.id.0.wrapping_add(1);
            if handle
                .update(cx, |this, w, cx| match e.phase {
                    TouchPhase::Started => {
                        this.start(id, e.position, cx);
                    }
                    TouchPhase::Moved => this.motion(id, e.position, w, cx),
                    TouchPhase::Ended => this.end(id, e.position, false, w, cx),
                    TouchPhase::Cancelled => this.end(id, e.position, true, w, cx),
                })
                .is_err()
            {
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
                    OverlayEvent::Monitor(dimensions) => {
                        this.monitor = dimensions;
                        this.geometry(w, true);
                        cx.notify();
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
}
fn watch_events() -> async_channel::Receiver<OverlayEvent> {
    let (tx, rx) = async_channel::bounded(16);
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
