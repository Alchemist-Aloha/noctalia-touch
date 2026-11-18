-- Installed by noctalia-touch. This file is sourced after the main user config.
local config = os.getenv("XDG_CONFIG_HOME") or (os.getenv("HOME") .. "/.config")
local root = config .. "/hypr/noctalia-touch"
local output = "@OUTPUT@"

local function command(path)
    return "'" .. path:gsub("'", "'\\''") .. "'"
end

local keyboard = command(root .. "/bin/floatkb")
local rotation = command(root .. "/bin/autorotate")
local plugin_path = root .. "/tablet-swipes.so"
local function plugin_loaded()
    for _, plugin in pairs(hl.get_loaded_plugins()) do
        if plugin.name == "tablet-swipes" then return true end
    end
    return false
end

if @SWIPES@ and not plugin_loaded() then
    pcall(hl.plugin.load, plugin_path)
end

hl.config({
    general = {
        layout = "scrolling",
        gaps_in = 4,
        gaps_out = 8,
        resize_on_border = true,
        extend_border_grab_area = 8,
    },
    scrolling = {
        fullscreen_on_one_column = true,
        column_width = 0.8,
        explicit_column_widths = "0.5, 0.8, 1.0",
        wrap_focus = false,
    },
    input = {
        follow_mouse = 0,
        touchdevice = { enabled = true, output = output },
    },
    cursor = {
        hide_on_touch = true,
        no_hardware_cursors = @SOFTWARE_CURSOR@ and 1 or nil,
    },
    -- Fall back to the built-in workspace edge swipes if the plugin fails to load.
    gestures = @SWIPES@ and { workspace_swipe_touch = not plugin_loaded() } or nil,
})

hl.workspace_rule({ workspace = "w[tv1]", gaps_out = 0, gaps_in = 0 })
hl.window_rule({
    name = "noctalia-touch-single-window",
    match = { float = false, workspace = "w[tv1]" },
    border_size = 0,
    rounding = 0,
})

hl.layer_rule({
    name = "noctalia-touch-keyboard-blur",
    match = { namespace = "^floatkb$" },
    blur = true,
    ignore_alpha = 0.1,
})

hl.bind("SUPER + K", hl.dsp.exec_cmd(keyboard .. " --toggle"))

hl.on("hyprland.start", function()
    if @START_NOCTALIA@ then hl.exec_cmd("noctalia") end
    if @AUTOROTATE@ then hl.exec_cmd(rotation) end
end)

if @AUTOROTATE@ then
    hl.bind("SUPER + O", hl.dsp.exec_cmd(rotation .. " --toggle-lock"))
    hl.on("config.reloaded", function()
        hl.exec_cmd(rotation .. " --reapply")
    end)
end
