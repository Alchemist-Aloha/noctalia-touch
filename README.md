# Noctalia Touch

A touchscreen setup for Hyprland and Noctalia. It adds a tablet bar, a floating on-screen keyboard, and touch controls for moving and resizing windows. It is designed for a scrolling workspace on a tablet, laptop, or handheld.

## Before you start

- Use a working **Hyprland Lua** session and **Noctalia 5**. The package was built and checked with Hyprland 0.57.2 and Noctalia 5.5.0 on an ARM64 tablet.
- Install **Python 3**, **PyGObject**, **GTK 4**, **gtk4-layer-shell** with its introspection data, and **wtype**. The native swipe plugin also needs **Hyprland development headers**, **pkg-config**, and a **C++23 compiler**. Use `--no-swipes` to try the bar and keyboard without a compiler or plugin.
- For autorotation, install **iio-sensor-proxy** and confirm that `monitor-sensor --accel` reports orientation changes.
- Inside Hyprland, run `hyprctl monitors` to find the connector name and scale for the touchscreen. Common names are `DSI-1` on tablets and `eDP-1` on laptops. Keep a keyboard handy the first time you try the swipe plugin.

## Install

Download or clone this repository, open a terminal in it, and run the following as your desktop user. Replace `DSI-1` with your connector.

```bash
./install.sh --output DSI-1 --dry-run
./install.sh --output DSI-1
```

Sign out and back in. The **Apps** and **Windows** buttons should appear in the bottom tablet bar. If Noctalia does not already start with Hyprland, add `--start-noctalia` to the install command. If you already have a Noctalia autostart command or user service, keep using it.

For a Legion Y700 Gen 4 with the 1904 × 3040 panel, this is an example setup with sensor rotation and a compact top bar. Keep your existing scale unless you know that a different one works for your panel.

```bash
./install.sh --output DSI-1 --autorotate 2.666667 --compact-top --software-cursor
```

The installer prints a unique backup path under `${XDG_STATE_HOME:-$HOME/.local/state}/noctalia-touch/backups`. It installs separate modules and adds one marked block at the end of your existing `hyprland.lua`. Running it again updates the packaged files and creates a new backup. Pass the options you want each time. See `./install.sh --help` for the full list.

## Use the controls

| Control | What it does |
| --- | --- |
| **Apps** | Opens the Noctalia app launcher. |
| **Windows** | Shows thumbnails of open windows. |
| **‹ / ›** | Focuses the previous or next column in the current scrolling workspace. |
| **Width** | Cycles the current column through the configured widths. |
| **Maximize** | Maximizes or restores the focused window. |
| **Float** | Opens controls for the focused window. Tap **Float** to make a tiled window float or **Tile** to put it back in the layout. |
| **Keyboard** | Shows or hides the floating keyboard. Drag its grip to move it, or use **−** and **+** to change the key size. |
| **Close** | Closes the focused window. |

In the **Float** popup, drag the **Move** or **Resize** pad to adjust a floating window. The adjacent buttons make smaller, tap-only adjustments. Tap **Done** to dismiss the popup. **Super+K** toggles the keyboard. If autorotation is enabled, **Super+O** toggles the rotation lock.

### Edge swipes

Start near the middle of the edge and drag inward before lifting your finger.

| Gesture | Action |
| --- | --- |
| Down from the top | Open the app launcher. |
| Up from the bottom | Open the Windows thumbnail grid. |
| Right from the left edge | Focus the previous column. |
| Left from the right edge | Focus the next column. |

The side gestures stay within the active scrolling workspace. Existing workspace keybindings remain available. The swipe plugin only claims contacts near the edge of the configured touchscreen.

## Troubleshooting

- **The tablet bar is missing.** Run `noctalia config validate` and confirm the connector name with `hyprctl monitors`. If your Noctalia settings specify a custom `[bar].order`, add `"noctalia_touch"` to that list alongside your other bars.
- **The bar works but swipes do not.** Run `hyprctl plugin list` in your Hyprland session and look for `tablet-swipes`. Run `hyprctl configerrors` for config errors. The plugin uses Hyprland’s native ABI and needs matching headers. After upgrading Hyprland, rerun the installer and sign out and back in. The tablet falls back to Hyprland’s built-in workspace edge swipes when the plugin cannot load. The physical gestures still need verification on your hardware.
- **The keyboard appears but does not type.** Check that `wtype` is on your path and that your Hyprland session allows its virtual keyboard protocol. The keyboard sends keys to the app that already has focus.
- **The panel rotates the wrong way.** Check `monitor-sensor --accel`, then adjust the `left-up` and `right-up` entries in the installed `bin/autorotate`. Use `--software-cursor` if your tablet driver cannot display the hardware cursor.
- **The Float popup uses its fallback palette.** Run `noctalia msg templates-apply` in your session and reopen the popup.

## Remove or roll back

Remove the block between `-- BEGIN noctalia-touch` and `-- END noctalia-touch` in your `hyprland.lua`. Remove the following installed paths relative to your config directory, normally `~/.config`.

- `hypr/noctalia-touch/`
- `noctalia/noctalia-touch.toml`
- `noctalia/templates/noctalia-touch-colors.css`

Sign out and back in to unload the plugin. If you want to roll back an upgrade, restore the affected files from the backup path printed by that install. Backups are local and may contain your private desktop settings. Keep them outside the repository.

## Project files

- `hypr/` contains the Lua settings for scrolling layout, touch input, keybindings, optional autorotation, and plugin loading.
- `noctalia/` contains the tablet bar, optional compact status bar, and Material 3 palette template.
- `bin/` contains the GTK floating keyboard, Float popup, and sensor rotation helper.
- `plugin/` contains the swipe plugin source. The installer builds it against local Hyprland headers.

[Hyprland plugin documentation](https://wiki.hypr.land/hyprland-plugins/using-plugins/) · [Noctalia configuration documentation](https://docs.noctalia.dev/noctalia/configuration/)
