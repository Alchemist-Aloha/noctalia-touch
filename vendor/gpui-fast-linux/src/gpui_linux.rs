// Modified by Longbridge for gpui-fast.
#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod fast;
mod linux;

pub use linux::{current_platform, linux_platform};

#[cfg(feature = "wayland")]
pub use linux::wayland::{
    RawTouchHandler, display_id, raise_layer, set_layer_margins, set_raw_touch_handler,
};
