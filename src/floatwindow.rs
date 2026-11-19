//! Non-focusable Material window controls. Hyprland I/O is serialized off the UI thread.
use crate::{
    hypr::{self, Client, Geometry, Target},
    theme::{Palette, palette},
};
use anyhow::{Result, anyhow, ensure};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_kit_material::{
    color::{Color, ColorRole, Scheme},
    components::{ButtonSize, ButtonVariant, MaterialButton},
    scope::Scope,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

const WIDTH: f32 = 424.;
const SHORT_HEIGHT: f32 = 96.;
const FULL_HEIGHT: f32 = 224.;
const PREVIEW_INTERVAL: Duration = Duration::from_millis(32);

type Pair = (i32, i32);
type Delta = (f64, f64);
type Area = (i32, i32, i32, i32);

fn bounded(mode: Geometry, (at, size): (Pair, Pair), area: Area, (dx, dy): Delta) -> Pair {
    let (left, top, right, bottom) = area;
    let (x, y) = at;
    let (w, h) = size;
    let limit = |base: i32, delta: f64, lo: i32, hi: i32| {
        (f64::from(base) + delta)
            .clamp(f64::from(lo), f64::from(hi.max(lo)))
            .round_ties_even() as i32
    };
    match mode {
        // An oversized window cannot fit on this axis without resizing it;
        // keep its leading edge in the work area so it remains reachable.
        Geometry::Move => (
            limit(x, dx, left, right.saturating_sub(w)),
            limit(y, dy, top, bottom.saturating_sub(h)),
        ),
        Geometry::Resize => {
            let max_w = right.saturating_sub(x).max(1);
            let max_h = bottom.saturating_sub(y).max(1);
            (
                limit(w, dx, 120.min(w).min(max_w), max_w),
                limit(h, dy, 88.min(h).min(max_h), max_h),
            )
        }
    }
}

struct Snapshot {
    mode: Geometry,
    at: Pair,
    size: Pair,
    monitor: i64,
    area: Area,
}

impl Snapshot {
    fn read(target: &Target, mode: Geometry) -> Result<Self> {
        let client = hypr::client(target)?.ok_or_else(|| anyhow!("window closed"))?;
        ensure!(
            client.adjustable() && client.floating,
            "window is not an adjustable float"
        );
        ensure!(
            client.size.0 > 0 && client.size.1 > 0,
            "invalid floating window size"
        );
        let monitor = hypr::monitors()?
            .into_iter()
            .find(|monitor| client.visible_on(monitor))
            .ok_or_else(|| anyhow!("window is not on a visible workspace"))?;
        Ok(Self {
            mode,
            at: client.at,
            size: client.size,
            monitor: monitor.id,
            area: monitor
                .work_area()
                .ok_or_else(|| anyhow!("invalid monitor work area"))?,
        })
    }

    fn original(&self) -> Pair {
        match self.mode {
            Geometry::Move => self.at,
            Geometry::Resize => self.size,
        }
    }

    fn values(&self, current: &Self, delta: Delta) -> Result<Pair> {
        ensure!(self.monitor == current.monitor, "window changed monitors");
        // The controlled axes use the drag's immutable origin; recheck the
        // complementary axes and monitor bounds against current compositor state.
        let geometry = match self.mode {
            Geometry::Move => (self.at, current.size),
            Geometry::Resize => (current.at, self.size),
        };
        Ok(bounded(self.mode, geometry, current.area, delta))
    }
}

struct Adjustment {
    initial: Snapshot,
    // A timeout can occur after the compositor applied a request. Remember
    // attempted previews, not just successful replies, for cancellation.
    attempted: bool,
}

impl Adjustment {
    fn apply(&mut self, target: &Target, delta: Delta) -> Result<()> {
        let now = Snapshot::read(target, self.initial.mode)?;
        let values = self.initial.values(&now, delta)?;
        self.attempted = true;
        hypr::set_geometry(target, self.initial.mode, values, now.monitor, true)
    }
}

enum Command {
    Toggle,
    Step(Geometry, Delta),
    Begin(Geometry),
    Preview(Delta),
    End(Option<Delta>),
    Stop,
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<Command>,
    stopping: bool,
}

impl Queue {
    fn push(&mut self, command: Command) {
        if self.stopping {
            return;
        }
        match command {
            Command::Stop => {
                self.stopping = true;
                // Quit cancels the open gesture and drops unprocessed previews.
                // An already released gesture still commits its final position.
                if self
                    .pending
                    .iter()
                    .any(|cmd| matches!(cmd, Command::End(Some(_))))
                {
                    self.pending
                        .retain(|cmd| matches!(cmd, Command::Begin(_) | Command::End(_)));
                } else {
                    self.pending.clear();
                }
            }
            Command::Preview(_) | Command::End(_) => {
                if matches!(self.pending.back(), Some(Command::Preview(_))) {
                    self.pending.pop_back();
                }
            }
            _ => {}
        }
        self.pending.push_back(command);
    }
}

#[derive(Clone, Default)]
struct Mailbox(Arc<(Mutex<Queue>, Condvar)>);

impl Mailbox {
    fn send(&self, command: Command) {
        let (queue, wake) = &*self.0;
        queue.lock().unwrap().push(command);
        wake.notify_one();
    }

    fn recv(&self) -> Command {
        let (queue, wake) = &*self.0;
        let queue = wake
            .wait_while(queue.lock().unwrap(), |queue| queue.pending.is_empty())
            .unwrap();
        let mut queue = queue;
        queue.pending.pop_front().unwrap()
    }
}

enum Update {
    Target(Option<Client>, Option<&'static str>),
    Status(&'static str),
    Complete(Option<&'static str>),
}

fn status_error(error: anyhow::Error, message: &'static str) -> &'static str {
    eprintln!("floatwindow: {message}: {error:#}");
    message
}

fn restore(target: &Target, drag: &Adjustment) -> Result<()> {
    if drag.attempted {
        // Restore the original geometry even if it was already partially
        // off-screen, but never mutate a window that has been tiled or replaced.
        hypr::set_geometry(
            target,
            drag.initial.mode,
            drag.initial.original(),
            drag.initial.monitor,
            false,
        )?;
    }
    Ok(())
}

fn worker(mailbox: Mailbox, updates: async_channel::Sender<Update>, target: Option<Target>) {
    let mut drag: Option<Adjustment> = None;
    let mut drag_error = None;
    loop {
        let command = mailbox.recv();
        if matches!(command, Command::Stop) {
            if let (Some(target), Some(drag)) = (&target, &drag)
                && let Err(error) = restore(target, drag)
            {
                status_error(error, "Could not restore window");
            }
            return;
        }
        let Some(target) = &target else { continue };
        let update = match command {
            Command::Toggle => {
                let toggle = || -> Result<Update> {
                    let Some(client) = hypr::client(target)? else {
                        return Ok(Update::Target(None, None));
                    };
                    if !client.adjustable() {
                        return Ok(Update::Target(Some(client), None));
                    }
                    let floating = !client.floating;
                    hypr::set_floating(target, floating)?;
                    let client = hypr::client(target)?;
                    let rejected = client
                        .as_ref()
                        .is_some_and(|client| client.floating != floating);
                    Ok(Update::Target(
                        client,
                        rejected.then_some("Could not change window type"),
                    ))
                };
                Some(toggle().unwrap_or_else(|error| {
                    Update::Complete(Some(status_error(error, "Could not change window type")))
                }))
            }
            Command::Step(mode, delta) => {
                let result = Snapshot::read(target, mode).and_then(|snapshot| {
                    hypr::set_geometry(
                        target,
                        mode,
                        snapshot.values(&snapshot, delta)?,
                        snapshot.monitor,
                        true,
                    )
                });
                Some(Update::Complete(
                    result
                        .err()
                        .map(|error| status_error(error, "Could not adjust window")),
                ))
            }
            Command::Begin(mode) => {
                drag_error = None;
                match Snapshot::read(target, mode) {
                    Ok(initial) => {
                        drag = Some(Adjustment {
                            initial,
                            attempted: false,
                        });
                        None
                    }
                    Err(error) => {
                        drag_error =
                            Some(status_error(error, "Reopen for a visible floating window"));
                        drag_error.map(Update::Status)
                    }
                }
            }
            Command::Preview(delta) => {
                if drag_error.is_some() {
                    continue;
                }
                let Some(drag) = drag.as_mut() else {
                    continue;
                };
                let result = drag.apply(target, delta);
                drag_error = result
                    .err()
                    .map(|error| status_error(error, "Could not adjust window"));
                drag_error.map(Update::Status)
            }
            Command::End(delta) => {
                let mut message = drag_error.take();
                if let Some(mut drag) = drag.take() {
                    if let Some(delta) = delta.filter(|_| message.is_none())
                        && let Err(error) = drag.apply(target, delta)
                    {
                        message = Some(status_error(error, "Could not adjust window"));
                    }
                    if (delta.is_none() || message.is_some())
                        && let Err(error) = restore(target, &drag)
                    {
                        message = Some(status_error(error, "Could not restore window"));
                    }
                }
                Some(Update::Complete(message))
            }
            Command::Stop => unreachable!(),
        };
        if let Some(update) = update {
            let _ = updates.try_send(update);
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Input {
    Mouse,
    Touch,
}

struct Drag {
    input: Input,
    mode: Geometry,
    start: Point<Pixels>,
    last_preview: Option<Instant>,
    changed: bool,
}

struct Panel {
    palette: Palette,
    target: Option<Client>,
    status: SharedString,
    mailbox: Mailbox,
    drag: Option<Drag>,
    busy: bool,
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.mailbox.send(Command::Stop);
    }
}

impl Panel {
    fn status(target: Option<&Client>) -> SharedString {
        match target {
            Some(client) if client.fullscreen => "Restore the window first".into(),
            Some(client) if client.adjustable() && !client.title.is_empty() => {
                client.title.clone().into()
            }
            Some(client) if client.adjustable() && client.floating => "Floating window".into(),
            Some(client) if client.adjustable() => "Tiled window".into(),
            _ => "Focus a window first".into(),
        }
    }

    fn adjustable(&self) -> bool {
        self.target.as_ref().is_some_and(Client::adjustable)
    }
    fn floating(&self) -> bool {
        self.adjustable() && self.target.as_ref().is_some_and(|client| client.floating)
    }

    fn enqueue(&mut self, command: Command, cx: &mut Context<Self>) {
        if !self.busy && self.drag.is_none() {
            self.busy = true;
            self.mailbox.send(command);
            cx.notify();
        }
    }

    fn begin(
        &mut self,
        input: Input,
        mode: Geometry,
        start: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.drag.is_some() || !self.floating() {
            return;
        }
        self.drag = Some(Drag {
            input,
            mode,
            start,
            last_preview: None,
            changed: false,
        });
        self.mailbox.send(Command::Begin(mode));
        cx.notify();
    }

    fn advance(
        &mut self,
        input: Input,
        position: Option<Point<Pixels>>,
        end: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.as_mut().filter(|drag| drag.input == input) else {
            return;
        };
        let delta = position.and_then(|position| {
            let offset = position - drag.start;
            let dx = f64::from(f32::from(offset.x));
            let dy = f64::from(f32::from(offset.y));
            if !dx.is_finite() || !dy.is_finite() {
                return None;
            }
            drag.changed |= dx.hypot(dy) >= 6.;
            drag.changed.then_some((dx, dy))
        });
        if end {
            self.drag = None;
            self.busy = true;
            self.mailbox.send(Command::End(delta));
            cx.notify();
        } else if let Some(delta) = delta
            && drag
                .last_preview
                .is_none_or(|time| time.elapsed() >= PREVIEW_INTERVAL)
        {
            drag.last_preview = Some(Instant::now());
            self.mailbox.send(Command::Preview(delta));
        }
    }

    fn button(id: &'static str, text: &'static str, primary: bool) -> MaterialButton {
        MaterialButton::new(id)
            .variant(if primary {
                ButtonVariant::Filled
            } else {
                ButtonVariant::Tonal
            })
            .button_size(ButtonSize::Small)
            .focusable(false)
            .child(text)
    }

    fn row(&self, mode: Geometry, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, steps) = match mode {
            Geometry::Move => (
                "Drag to move",
                [
                    ("←", "Move left"),
                    ("↑", "Move up"),
                    ("↓", "Move down"),
                    ("→", "Move right"),
                ],
            ),
            Geometry::Resize => (
                "Drag to resize",
                [
                    ("↔−", "Narrower"),
                    ("↕−", "Shorter"),
                    ("↕+", "Taller"),
                    ("↔+", "Wider"),
                ],
            ),
        };
        let pressed = self.drag.as_ref().is_some_and(|drag| drag.mode == mode);
        let p = &self.palette;
        let view = cx.weak_entity();
        let touch = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                window.on_mouse_event(move |event: &TouchDragEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble || !bounds.contains(&event.start_position) {
                        return;
                    }
                    let _ = view.update(cx, |view, cx| match event.phase {
                        TouchPhase::Started => {
                            view.begin(Input::Touch, mode, event.start_position, cx);
                            window.prevent_default(); // claim the entire touch sequence
                            cx.stop_propagation();
                        }
                        TouchPhase::Moved => {
                            view.advance(Input::Touch, Some(event.position), false, cx)
                        }
                        TouchPhase::Ended => {
                            view.advance(Input::Touch, Some(event.position), true, cx)
                        }
                        TouchPhase::Cancelled => view.advance(Input::Touch, None, true, cx),
                    });
                });
            },
        )
        .absolute()
        .size_full();
        let pad = div()
            .id(label)
            .relative()
            .flex()
            .flex_1()
            .h(px(48.))
            .items_center()
            .justify_center()
            .rounded(px(24.))
            .bg(if pressed { p.primary } else { p.tonal })
            .text_color(if pressed { p.on_primary } else { p.on_tonal })
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor(if pressed {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                    view.begin(Input::Mouse, mode, event.position, cx);
                }),
            )
            .child(touch)
            .child(label);
        div().flex().items_center().gap(px(4.)).child(pad).children(
            steps
                .into_iter()
                .zip([(-32., 0.), (0., -32.), (0., 32.), (32., 0.)])
                .map(|((text, id), delta)| {
                    Self::button(id, text, false)
                        .accessibility_label(id)
                        .px(px(8.))
                        .min_w(px(48.))
                        .disabled(self.busy || self.drag.is_some())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.enqueue(Command::Step(mode, delta), cx)
                        }))
                }),
        )
    }
}

fn material_scheme(p: &Palette) -> Scheme {
    let color = |c: Rgba| Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    };
    gpui_kit_material::theme::scheme()
        .with_role(ColorRole::SURFACE_CONTAINER, color(p.surface))
        .with_role(ColorRole::ON_SURFACE, color(p.text))
        .with_role(ColorRole::ON_SURFACE_VARIANT, color(p.muted))
        .with_role(ColorRole::PRIMARY, color(p.primary))
        .with_role(ColorRole::ON_PRIMARY, color(p.on_primary))
        .with_role(ColorRole::SECONDARY_CONTAINER, color(p.tonal))
        .with_role(ColorRole::ON_SECONDARY_CONTAINER, color(p.on_tonal))
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.palette;
        let view = cx.weak_entity();
        // Mouse events are observed for the entire surface, not just the pad
        // that started the gesture. Wayland's implicit button grab delivers up.
        let mouse = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let movement = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = movement.update(cx, |view, cx| {
                            view.advance(
                                Input::Mouse,
                                event.dragging().then_some(event.position),
                                !event.dragging(),
                                cx,
                            );
                        });
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        let _ = view.update(cx, |view, cx| {
                            view.advance(Input::Mouse, Some(event.position), true, cx)
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child("Window controls"),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_color(p.muted)
                            .text_size(px(12.))
                            .child(self.status.clone()),
                    ),
            )
            .when(self.adjustable(), |el| {
                el.child(
                    Self::button(
                        "float-tile",
                        if self.floating() { "Tile" } else { "Float" },
                        false,
                    )
                    .disabled(self.busy || self.drag.is_some())
                    .on_click(cx.listener(|view, _, _, cx| view.enqueue(Command::Toggle, cx))),
                )
            })
            .child(Self::button("done", "Done", true).on_click(|_, _, cx| cx.quit()));
        let panel = div()
            .relative()
            .rounded(px(24.))
            .bg(p.surface)
            .text_color(p.text)
            .shadow(vec![BoxShadow {
                color: Rgba {
                    a: p.shadow.a * 0.34,
                    ..p.shadow
                }
                .into(),
                offset: point(px(0.), px(5.)),
                blur_radius: px(12.),
                spread_radius: px(0.),
                inset: false,
            }])
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(mouse)
            .child(header)
            .when(self.floating(), |el| {
                el.child(self.row(Geometry::Move, cx))
                    .child(self.row(Geometry::Resize, cx))
            });
        Scope::new("floatwindow-M3")
            .scheme(material_scheme(p))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .p(px(8.))
                    .child(panel),
            )
    }
}

pub fn run(cx: &mut App) -> Result<()> {
    // Capture the original target before mapping the overlay; never reselect
    // the active window as focus changes during the session.
    let (target, client, message) = match hypr::active_client() {
        Ok(Some((target, client))) => (Some(target), Some(client), None),
        Ok(None) => (None, None, None),
        Err(error) => (
            None,
            None,
            Some(status_error(error, "Hyprland is unavailable")),
        ),
    };
    let status = message.map_or_else(|| Panel::status(client.as_ref()), Into::into);
    let (updates, result) = async_channel::unbounded();
    let (stopped, shutdown) = async_channel::bounded::<()>(1);
    let mailbox = Mailbox::default();
    let worker_mailbox = mailbox.clone();
    thread::Builder::new()
        .name("floatwindow-ipc".into())
        .spawn(move || {
            worker(worker_mailbox, updates, target);
            let _ = stopped.try_send(());
        })?;
    let cleanup = mailbox.clone();
    cx.on_app_quit(move |_| {
        cleanup.send(Command::Stop);
        let shutdown = shutdown.clone();
        async move {
            let _ = shutdown.recv().await;
        }
    })
    .detach();
    let full = client
        .as_ref()
        .is_some_and(|c| c.adjustable() && c.floating);
    let height = if full { FULL_HEIGHT } else { SHORT_HEIGHT };
    let close_on_error = mailbox.clone();
    let window = crate::shell::open(cx, "floatwindow", WIDTH, height, None, move |_, cx| {
        cx.new(|_| Panel {
            palette: palette(),
            target: client,
            status,
            mailbox,
            drag: None,
            busy: false,
        })
    });
    let window = match window {
        Ok(window) => window,
        Err(error) => {
            close_on_error.send(Command::Stop);
            return Err(error);
        }
    };
    cx.spawn(async move |cx| {
        while let Ok(update) = result.recv().await {
            if window
                .update(cx, |view, window, cx| {
                    match update {
                        Update::Target(client, message) => {
                            let was_floating = view.floating();
                            view.target = client;
                            view.busy = false;
                            view.status = message
                                .map_or_else(|| Panel::status(view.target.as_ref()), Into::into);
                            if was_floating != view.floating() {
                                window.resize(size(
                                    px(WIDTH),
                                    px(if view.floating() {
                                        FULL_HEIGHT
                                    } else {
                                        SHORT_HEIGHT
                                    }),
                                ));
                            }
                        }
                        Update::Status(message) => view.status = message.into(),
                        Update::Complete(message) => {
                            view.busy = false;
                            view.status = message
                                .map_or_else(|| Panel::status(view.target.as_ref()), Into::into);
                        }
                    }
                    cx.notify();
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

#[cfg(test)]
mod tests {
    use super::{Command, Geometry, Queue, bounded};

    #[test]
    fn pending_previews_coalesce_and_never_run_after_cancel() {
        let mut queue = Queue::default();
        queue.push(Command::Begin(Geometry::Move));
        queue.push(Command::Preview((10., 20.)));
        queue.push(Command::Preview((30., 40.)));
        assert!(matches!(
            queue.pending.back(),
            Some(Command::Preview((30., 40.)))
        ));
        assert_eq!(queue.pending.len(), 2);
        queue.push(Command::End(None));
        assert!(matches!(
            queue.pending.pop_front(),
            Some(Command::Begin(Geometry::Move))
        ));
        assert!(matches!(
            queue.pending.pop_front(),
            Some(Command::End(None))
        ));
        assert!(queue.pending.is_empty());
    }

    #[test]
    fn shutdown_keeps_a_released_drag_but_drops_unreleased_previews() {
        let mut queue = Queue::default();
        queue.push(Command::Begin(Geometry::Resize));
        queue.push(Command::End(Some((40., 50.))));
        queue.push(Command::Stop);
        assert!(matches!(
            queue.pending.pop_front(),
            Some(Command::Begin(Geometry::Resize))
        ));
        assert!(matches!(
            queue.pending.pop_front(),
            Some(Command::End(Some((40., 50.))))
        ));
        assert!(matches!(queue.pending.pop_front(), Some(Command::Stop)));

        let mut queue = Queue::default();
        queue.push(Command::Begin(Geometry::Move));
        queue.push(Command::Preview((10., 20.)));
        queue.push(Command::Stop);
        assert!(matches!(queue.pending.pop_front(), Some(Command::Stop)));
        assert!(queue.pending.is_empty());
    }

    #[::core::prelude::v1::test]
    fn movement_respects_negative_origin_and_reserved_edges() {
        let geometry = ((-400, 60), (240, 100));
        let area = (-500, 34, -10, 660);
        assert_eq!(
            bounded(Geometry::Move, geometry, area, (-900., 900.)),
            (-500, 560)
        );
    }

    #[::core::prelude::v1::test]
    fn resize_does_not_force_existing_small_window_to_grow() {
        let geometry = ((10, 20), (80, 60));
        assert_eq!(
            bounded(Geometry::Resize, geometry, (0, 0, 500, 400), (-32., -32.)),
            (80, 60)
        );
    }

    #[::core::prelude::v1::test]
    fn resize_near_output_edge_respects_room_over_nominal_minimum() {
        assert_eq!(
            bounded(
                Geometry::Resize,
                ((450, 350), (80, 60)),
                (0, 0, 500, 400),
                (100., 100.)
            ),
            (50, 50)
        );
    }
}
