# gpui-fast-linux

GPUI with gpui-fast rendering and layout optimizations.

Source and documentation: [https://github.com/longbridge/gpui-fast](https://github.com/longbridge/gpui-fast).

Forked from Zed's GPUI; distributed under Apache-2.0.

Use the core as `gpui = { package = "gpui-fast", version = "0.1.16" }` to preserve paths emitted by its macros.

## Noctalia Touch changes

This copy is based on `gpui-fast-linux` 0.1.16 as published on crates.io. The local patch forwards Wayland touchscreen contacts, adds a raw multi-touch callback for layer surfaces, and exposes output lookup, layer margins, and Hyprland restacking. The original license is in `LICENSE`.
