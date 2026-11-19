#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'HELP'
Usage: ./install.sh [options]

  --output CONNECTOR     Tablet display from hyprctl monitors (default DSI-1)
  --autorotate SCALE     Enable sensor rotation at this monitor scale
  --compact-top          Also compact the Noctalia default bar on the tablet
  --software-cursor     Draw the cursor in software for affected tablet drivers
  --start-noctalia        Start Noctalia at login if you do not already start it
  --no-swipes            Skip the native plugin and keep existing gestures
  --dry-run              Print paths and choices without writing or building
  --help                 Show this help
HELP
}

fail() { printf 'noctalia-touch: %s\n' "$*" >&2; exit 1; }

output=DSI-1
scale=1
rotate=false
compact=false
software_cursor=false
start_noctalia=false
swipes=true
dry_run=false

while (($#)); do
    case $1 in
        --output) (($# >= 2)) || fail "--output needs a connector"; output=$2; shift ;;
        --autorotate) (($# >= 2)) || fail "--autorotate needs a scale"; scale=$2; rotate=true; shift ;;
        --compact-top) compact=true ;;
        --software-cursor) software_cursor=true ;;
        --start-noctalia) start_noctalia=true ;;
        --no-swipes) swipes=false ;;
        --dry-run) dry_run=true ;;
        --help|-h) usage; exit 0 ;;
        *) fail "unknown option $1" ;;
    esac
    shift
done

[[ $output =~ ^[A-Za-z0-9._-]+$ ]] || fail "invalid connector $output"
[[ $scale =~ ^[0-9]+([.][0-9]+)?$ && $scale == *[1-9]* ]] || fail "scale must be a positive decimal"
((EUID != 0)) || fail "run as your desktop user"
src=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
config=${XDG_CONFIG_HOME:-"$HOME/.config"}
state=${XDG_STATE_HOME:-"$HOME/.local/state"}
main=$config/hypr/hyprland.lua
root=$config/hypr/noctalia-touch
bar=$config/noctalia/noctalia-touch.toml
colors=$config/noctalia/templates/noctalia-touch-colors.css
rotation_style=$config/noctalia/noctalia-touch-rotation.toml
begin='-- BEGIN noctalia-touch'
end='-- END noctalia-touch'

[[ -f $main ]] || fail "no Lua config at $main. Set up Hyprland with a hyprland.lua first."
if grep -Fxq -- "$begin" "$main"; then
    grep -Fxq -- "$end" "$main" || fail "incomplete noctalia-touch block in $main"
    grep -Fxq -- "$end" <(tail -n 1 "$main") || fail "move the noctalia-touch block to the end of $main before reinstalling"
elif grep -Fxq -- "$end" "$main"; then
    fail "incomplete noctalia-touch block in $main"
fi

printf 'Tablet output: %s\nSwipes: %s\nAutorotation: %s (scale %s)\nCompact top bar: %s\nSoftware cursor: %s\nStart Noctalia at login: %s\n' \
    "$output" "$swipes" "$rotate" "$scale" "$compact" "$software_cursor" "$start_noctalia"
printf 'Install paths:\n  %s\n  %s\n  %s\n  %s\n' "$main" "$root" "$bar" "$colors"
if "$dry_run"; then exit 0; fi

for tool in python3 noctalia hyprctl wtype; do
    command -v "$tool" >/dev/null || fail "missing $tool. See the README for dependencies."
done
python3 - <<'PYTHON' || fail "install Python GObject bindings, GTK 4 and gtk4-layer-shell introspection"
import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Gtk4LayerShell", "1.0")
PYTHON
if "$rotate"; then
    command -v monitor-sensor >/dev/null || fail "install iio-sensor-proxy for autorotation"
    command -v jq >/dev/null || fail "install jq for autorotation"
fi

stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
rotation_member=""
"$rotate" && rotation_member=', "noctalia_touch_rotation"'
render() {
    sed -e "s/@OUTPUT@/$output/g" -e "s/@SCALE@/$scale/g" \
        -e "s/@ROTATION_MEMBER@/$rotation_member/g" \
        -e "s/@SWIPES@/$swipes/g" -e "s/@AUTOROTATE@/$rotate/g" \
        -e "s/@SOFTWARE_CURSOR@/$software_cursor/g" \
        -e "s/@START_NOCTALIA@/$start_noctalia/g" "$1"
}

render "$src/hypr/tablet.lua" > "$stage/tablet.lua"
render "$src/noctalia/tablet.toml" > "$stage/noctalia-touch.toml"
if "$rotate"; then
    printf '\n' >> "$stage/noctalia-touch.toml"
    render "$src/noctalia/rotation-button.toml" >> "$stage/noctalia-touch.toml"
fi
if "$compact"; then
    printf '\n' >> "$stage/noctalia-touch.toml"
    render "$src/noctalia/compact-top-bar.toml" >> "$stage/noctalia-touch.toml"
fi
for script in floatkb floatwindow autorotate; do
    render "$src/bin/$script" > "$stage/$script"
done
noctalia config validate "$stage/noctalia-touch.toml"
if "$swipes"; then
    command -v pkg-config >/dev/null && pkg-config --exists hyprland ||
        fail "Hyprland development headers are required for swipes. Install them or use --no-swipes."
    compiler=${CXX:-c++}
    command -v "$compiler" >/dev/null || fail "missing C++ compiler. Install one or use --no-swipes."
    read -r -a cflags <<< "$(pkg-config --cflags hyprland)"
    "$compiler" -std=c++23 -O2 -fPIC -shared "${cflags[@]}" "-DTABLET_OUTPUT=\"$output\"" \
        "$src/plugin/tablet-swipes.cpp" -o "$stage/tablet-swipes.so"
fi

mkdir -p -- "$state/noctalia-touch/backups"
backup=$(mktemp -d "$state/noctalia-touch/backups/install.XXXXXXXX")
chmod 700 "$backup"
cp -L -- "$main" "$backup/hyprland.lua"
for path in "$root" "$bar" "$colors" "$rotation_style"; do
    if [[ -e $path || -L $path ]]; then
        cp -a -- "$path" "$backup/"
    fi
done

mkdir -p -- "$root/bin" "$(dirname -- "$colors")"
install -m 644 "$stage/tablet.lua" "$root/tablet.lua"
install -m 755 "$stage/"{floatkb,floatwindow,autorotate} "$root/bin/"
if "$swipes"; then
    # Replacing the file keeps an already mapped plugin valid until the next login.
    install -m 644 "$stage/tablet-swipes.so" "$root/tablet-swipes.so.new"
    mv -f -- "$root/tablet-swipes.so.new" "$root/tablet-swipes.so"
fi
install -m 644 "$src/noctalia/floatwindow-colors.css" "$colors"
install -m 644 "$stage/noctalia-touch.toml" "$bar"
# bin/autorotate writes the rotation button's style when it starts.
"$rotate" || rm -f -- "$rotation_style"
if ! grep -Fxq -- "$begin" "$main"; then
    cat >> "$main" <<'LUA'

-- BEGIN noctalia-touch
dofile((os.getenv("XDG_CONFIG_HOME") or (os.getenv("HOME") .. "/.config")) .. "/hypr/noctalia-touch/tablet.lua")
-- END noctalia-touch
LUA
fi

printf '\nInstalled. Backups: %s\n' "$backup"
printf 'Sign out and back in to activate the settings and load the plugin.\n'
