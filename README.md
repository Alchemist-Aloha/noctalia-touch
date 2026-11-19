# Noctalia Touch

A touchscreen setup for Hyprland and Noctalia. It adds a tablet bar, a floating on-screen keyboard, and touch controls for moving and resizing windows. It is designed for a scrolling workspace on a tablet, laptop, or handheld.

## Before you start

- Use a working **Hyprland Lua** session and **Noctalia 5**. The package was built and checked with Hyprland 0.57.2 and Noctalia 5.5.0 on an ARM64 tablet.
- Install **Rust and Cargo 1.98+**, a C linker, **pkg-config**, the development libraries for **Wayland**, **XKB/X11**, and **Fontconfig**, a working **Vulkan driver**, and **wtype**. The installer builds the GPUI/GPUI Kit overlays from source. Autorotation also needs **jq**. The swipe plugin requires **Hyprland development headers** and a **C++23 compiler**; use `--no-swipes` to skip only the plugin build.
- For autorotation, install **iio-sensor-proxy** and confirm that `monitor-sensor --accel` reports orientation changes.
- Inside Hyprland, run `hyprctl monitors` to find the connector name and scale for the touchscreen. Common names are `DSI-1` on tablets and `eDP-1` on laptops. Keep a keyboard handy the first time you try the swipe plugin.

## Install

Download or clone this repository, open a terminal in it, and run the following as your desktop user. Replace `DSI-1` with your connector.

```bash
./install.sh --output DSI-1 --dry-run
./install.sh --output DSI-1
```

Sign out and back in. The tablet bar should appear at the bottom of the touchscreen. If Noctalia does not already start with Hyprland, add `--start-noctalia` to the install command. If you already have a Noctalia autostart command or user service, keep using it.

For a Legion Y700 Gen 4 with the 1904 × 3040 panel, this is an example setup with sensor rotation and a compact top bar. Keep your existing scale unless you know that a different one works for your panel.

```bash
./install.sh --output DSI-1 --autorotate 2.666667 --compact-top --software-cursor
```

The installer prints a unique backup path under `${XDG_STATE_HOME:-$HOME/.local/state}/noctalia-touch/backups`. It installs separate modules and adds one marked block at the end of your existing `hyprland.lua`. Running it again updates the packaged files and creates a new backup. Pass the options you want each time. See `./install.sh --help` for the full list.

## Use the controls

The bar buttons are icons in rounded groups. Hold a mouse pointer over one to see its name.

| Control | What it does |
| --- | --- |
| **Search** | Opens the Noctalia app launcher. |
| **Keyboard** | Shows or hides the floating keyboard. |
| **Rotate** | With `--autorotate`, switches between following the sensor (highlighted rotate icon) and locking the current orientation (lock icon). |
| **Grid** | Shows thumbnails of open windows. |
| **Picture-in-picture** | Opens the Float popup for the focused window. Tap **Float** to make a tiled window float or **Tile** to put it back in the layout. |
| **‹ / ›** | Focuses the previous or next column in the current scrolling workspace. |
| **Columns** | Cycles the current column through the configured widths. |
| **Maximize** | Maximizes or restores the focused window. |
| **×** | Closes the focused window. |

In the **Float** popup, drag the **Move** or **Resize** pad to adjust a floating window. The adjacent buttons make smaller, tap-only adjustments. Tap **Done** to dismiss the popup. **Super+K** toggles the keyboard. If autorotation is enabled, **Super+O** toggles the rotation lock.

### Floating keyboard

The keyboard uses a phone-style layout with three pages: letters, **?123** for numbers and common symbols, and **=\\<** for brackets, more symbols, and Home, End, PgUp, PgDn, and Del. A short row above every page holds Esc, Tab, Ctrl, Alt, Super, and the arrow keys. Ctrl, Alt, and Super stay on until the next key, so you can tap **Ctrl**, then **c**.

- Swipe up on a key, or hold it, to type the small character in its corner. For example, swipe up on **q** to type **1**.
- Slide along the space bar to move the cursor.
- Tap shift once for one capital letter, or twice quickly for caps lock.
- Drag the grip to move the keyboard. The bar buttons change vibration, opacity, and key size. **Hide** shrinks it to a round button that you can tap to reopen or drag to move.

Keys go through a persistent virtual keyboard on `/dev/uinput` that uses your normal keyboard layout. This avoids lost keys in clients such as the Noctalia launcher, which reload the keymap each time `wtype` sends a key. Characters your layout lacks, such as **€** or **¹**, fall back to `wtype`. If `/dev/uinput` is not writable for your user, every key uses `wtype`. When a Noctalia panel opens, the keyboard moves itself above it. If your device has a force-feedback motor whose input device name contains `haptic`, keys vibrate briefly.

### Edge swipes

Start near the middle of the edge and drag inward before lifting your finger.

| Gesture | Action |
| --- | --- |
| Down from the top | Open the app launcher. |
| Up from the bottom | Open the Windows thumbnail grid. |
| Right from the left edge | Focus the previous column. |
| Left from the right edge | Focus the next column. |

The side gestures stay within the active scrolling workspace. Existing workspace keybindings remain available. The swipe plugin only claims contacts near the edge of the configured touchscreen. A touch that starts near the edge but barely moves still reaches the bar or panel under it as a tap.

The plugin also delivers touches on the floating keyboard directly to it. Without this, Hyprland would focus the window under your finger, which closes the Noctalia launcher as soon as you type.

## Troubleshooting

- **The tablet bar is missing.** Run `noctalia config validate` and confirm the connector name with `hyprctl monitors`. If your Noctalia settings specify a custom `[bar].order`, add `"noctalia_touch"` to that list alongside your other bars.
- **The bar works but swipes do not.** Run `hyprctl plugin list` in your Hyprland session and look for `tablet-swipes`. Run `hyprctl configerrors` for config errors. The plugin uses Hyprland’s native ABI and needs matching headers. After upgrading Hyprland, rerun the installer and sign out and back in. The tablet falls back to Hyprland’s built-in workspace edge swipes when the plugin cannot load. The physical gestures still need verification on your hardware.
- **The keyboard appears but does not type.** Run `hyprctl devices` and look for `floatkb virtual keyboard`. If it is missing, check that your user can write to `/dev/uinput`. Otherwise, check that `wtype` is on your path. The keyboard sends keys to the app that already has focus.
- **The launcher closes when you tap the keyboard.** The swipe plugin keeps launcher focus for touches only. A mouse click on the keyboard still closes it. Check that `tablet-swipes` appears in `hyprctl plugin list`.
- **The top bar disappears after a rotation.** Noctalia sometimes fails to recreate a bar after an output change. With autorotation, `bin/autorotate` restarts Noctalia if the top bar is still missing 4 seconds after the last rotation, and logs a warning under the `autorotate` tag in the journal.
- **The panel rotates the wrong way.** Check `monitor-sensor --accel`, then adjust the `left-up` and `right-up` entries in the installed `bin/autorotate`. Use `--software-cursor` if your tablet driver cannot display the hardware cursor.
- **The Float popup uses its fallback palette.** Run `noctalia msg templates-apply` in your session and reopen the popup.

## Remove or roll back

Remove the block between `-- BEGIN noctalia-touch` and `-- END noctalia-touch` in your `hyprland.lua`. Remove the following installed paths relative to your config directory, normally `~/.config`.

- `hypr/noctalia-touch/`
- `noctalia/noctalia-touch.toml`
- `noctalia/templates/noctalia-touch-colors.css`
- `noctalia/noctalia-touch-rotation.toml`, if you used `--autorotate`

Sign out and back in to unload the plugin. If you want to roll back an upgrade, restore the affected files from the backup path printed by that install. Backups are local and may contain your private desktop settings. Keep them outside the repository.

## Project files

- `hypr/` contains the Lua settings for scrolling layout, touch input, keybindings, optional autorotation, and plugin loading.
- `noctalia/` contains the tablet bar, the optional rotation button and compact status bar, and the Material 3 palette template.
- `src/` contains the Rust floating keyboard and Float popup. Both use GPUI Kit on Wayland layer shell and share the Noctalia Material 3 palette. The installer provides `bin/floatkb` and `bin/floatwindow` entry points to the same native binary.
- `bin/autorotate` is the shell-based sensor rotation helper.
- `plugin/` contains the swipe plugin source. The installer builds it against local Hyprland headers.
- `vendor/gpui-fast-linux/` is the pinned Linux backend, patched to forward Wayland touchscreen contacts and expose layer margins, restacking, and output selection to the overlays.

[Hyprland plugin documentation](https://wiki.hypr.land/hyprland-plugins/using-plugins/) · [Noctalia configuration documentation](https://docs.noctalia.dev/noctalia/configuration/)
