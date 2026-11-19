use gpui_kit::{
    layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions},
    *,
};
use std::{env, fs};

pub fn move_layer(window: &Window, left: i32, bottom: i32) {
    gpui_fast_linux::set_layer_margins(window, 0, 0, bottom, left);
}

pub fn raise_layer(window: &Window) {
    gpui_fast_linux::raise_layer(window);
}

pub fn output() -> Option<String> {
    env::var("NOCTALIA_TOUCH_OUTPUT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let exe = env::current_exe().ok()?;
            let configured = fs::read_to_string(exe.parent()?.join("../output")).ok()?;
            let connector = configured.trim();
            (!connector.is_empty()).then(|| connector.to_owned())
        })
}

pub fn open<V: Render + 'static>(
    cx: &mut App,
    namespace: &'static str,
    width: f32,
    height: f32,
    position: Option<(i32, i32)>,
    create: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<WindowHandle<V>> {
    let (left, bottom) = position.unwrap_or((0, 8));
    let anchor = if position.is_some() {
        Anchor::BOTTOM | Anchor::LEFT
    } else {
        Anchor::BOTTOM
    };
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
    let display_id = output().and_then(|name| gpui_fast_linux::display_id(cx, &name));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        kind: WindowKind::LayerShell(LayerShellOptions {
            namespace: namespace.into(),
            layer: Layer::Overlay,
            anchor,
            exclusive_zone: Some(px(0.)),
            exclusive_edge: None,
            margin: Some((px(0.), px(0.), px(bottom as f32), px(left as f32))),
            keyboard_interactivity: KeyboardInteractivity::None,
        }),
        titlebar: None,
        focus: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some(format!("org.noctaliaTouch.{namespace}")),
        ..Default::default()
    };
    cx.open_window(opts, |window, cx| {
        window.disable_focus(cx);
        create(window, cx)
    })
}
