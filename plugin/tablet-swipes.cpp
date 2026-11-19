#include <cmath>
#include <optional>
#include <unordered_map>
#include <stdexcept>
#include <string_view>

#ifndef TABLET_OUTPUT
#define TABLET_OUTPUT "DSI-1"
#endif

#include <hyprland/src/desktop/view/LayerSurface.hpp>
#include <hyprland/src/event/EventBus.hpp>
#include <hyprland/src/managers/SeatManager.hpp>
#include <hyprland/src/managers/SessionLockManager.hpp>
#include <hyprland/src/output/Monitor.hpp>
#include <hyprland/src/state/MonitorState.hpp>
#include <hyprland/src/plugins/PluginAPI.hpp>

namespace {
enum class Edge { top, bottom, left, right };

struct Swipe {
    int id;
    Edge edge;
    Vector2D start;
    Vector2D latest;
    uint32_t downMs;
    std::string output;
};

std::optional<Swipe> swipe;
CHyprSignalListener down, motion, up, cancel;

// Touches on the floating keyboard (floatkb, a no-keyboard-focus overlay layer) are
// delivered to it directly. Hyprland's own touch handling would refocus the window
// under the finger, which closes on-demand panels such as the Noctalia launcher.
struct KeyboardTouch {
    PHLLSREF layer;
    Vector2D origin;   // surface origin at touch-down, like Hyprland's touchSurfaceOrigin
    PHLMONITORREF monitor;
};
std::unordered_map<int, KeyboardTouch> keyboardTouches;

PHLMONITOR monitorNamed(const std::string& name) {
    for (const auto& monitor : State::monitorState()->monitors())
        if (monitor && monitor->m_name == name)
            return monitor;
    return nullptr;
}

Vector2D layoutPoint(const PHLMONITOR& monitor, Vector2D normalized) {
    return monitor->m_position + normalized * monitor->m_size;
}

// A touch that starts in the edge strip but barely moves is a tap, not a swipe: the
// strip overlaps the bars, so replay it on the layer surface (bar, panel) under it.
PHLLS layerAt(const PHLMONITOR& monitor, Vector2D point) {
    for (const int level : {3, 2}) {   // overlay, then top
        const auto& layers = monitor->m_layerSurfaceLayers[level];
        for (auto it = layers.rbegin(); it != layers.rend(); ++it) {   // topmost first
            const auto layer = it->lock();
            if (layer && layer->mapped() && layer->resource() && layer->m_geometry.containsPoint(point))
                return layer;
        }
    }
    return nullptr;
}

void replayTap(const Swipe& gesture, uint32_t upMs) {
    const auto monitor = monitorNamed(gesture.output);
    if (!monitor)
        return;
    const auto point = layoutPoint(monitor, gesture.start);
    const auto layer = layerAt(monitor, point);
    if (!layer)
        return;
    const auto local = point - layer->m_geometry.pos();
    g_pSeatManager->sendTouchDown(layer->resource(), gesture.downMs, gesture.id, local);
    g_pSeatManager->sendTouchFrame();
    g_pSeatManager->sendTouchUp(upMs, gesture.id);
    g_pSeatManager->sendTouchFrame();
}

PHLLS keyboardAt(const PHLMONITOR& monitor, Vector2D point) {
    for (const auto& ref : monitor->m_layerSurfaceLayers[3]) {   // overlay layer
        const auto layer = ref.lock();
        if (layer && layer->mapped() && layer->m_namespace == "floatkb" &&
            layer->m_geometry.containsPoint(point))
            return layer;
    }
    return nullptr;
}

std::optional<Edge> edgeAt(Vector2D point) {
    constexpr double edge = 0.025;
    constexpr double corner = 0.06;
    if (point.x > corner && point.x < 1 - corner) {
        if (point.y < edge) return Edge::top;
        if (point.y > 1 - edge) return Edge::bottom;
    }
    if (point.y > corner && point.y < 1 - corner) {
        if (point.x < edge) return Edge::left;
        if (point.x > 1 - edge) return Edge::right;
    }
    return std::nullopt;
}

void dispatch(const Swipe& gesture) {
    const auto delta = gesture.latest - gesture.start;
    double inward, cross;
    const char* action;
    switch (gesture.edge) {
        case Edge::top:
            inward = delta.y; cross = delta.x;
            action = "hl.dsp.exec_cmd(\"noctalia msg panel-toggle launcher\")";
            break;
        case Edge::bottom:
            inward = -delta.y; cross = delta.x;
            action = "hl.dsp.exec_cmd(\"noctalia msg window-switcher all\")";
            break;
        case Edge::left:
            inward = delta.x; cross = delta.y;
            action = "hl.dsp.layout(\"focus l\")";
            break;
        case Edge::right:
            inward = -delta.x; cross = delta.y;
            action = "hl.dsp.layout(\"focus r\")";
            break;
        default:
            return;
    }
    if (inward >= 0.045 && inward >= 1.5 * std::abs(cross))
        HyprlandAPI::invokeHyprctlCommand("dispatch", action);
}

bool locked() {
    return !g_pSessionLockManager || g_pSessionLockManager->isSessionLocked();
}
}

APICALL EXPORT std::string PLUGIN_API_VERSION() {
    return HYPRLAND_API_VERSION;
}

APICALL EXPORT PLUGIN_DESCRIPTION_INFO PLUGIN_INIT(HANDLE) {
    if (std::string_view(__hyprland_api_get_hash()) != __hyprland_api_get_client_hash())
        throw std::runtime_error("tablet-swipes requires a rebuild for this Hyprland version");

    auto& touch = Event::bus()->m_events.input.touch;
    down = touch.down.listen([](const ITouch::SDownEvent& event, Event::SCallbackInfo& info) {
        if (swipe && swipe->id == event.touchID)
            swipe.reset();
        if (info.cancelled || locked() || swipe || !event.device ||
            event.device->m_boundOutput != TABLET_OUTPUT)
            return;
        if (const auto edge = edgeAt(event.pos)) {
            swipe = Swipe{event.touchID, *edge, event.pos, event.pos, event.timeMs, event.device->m_boundOutput};
            info.cancelled = true;
            return;
        }
        const auto monitor = monitorNamed(event.device->m_boundOutput);
        if (!monitor)
            return;
        const auto point = layoutPoint(monitor, event.pos);
        const auto layer = keyboardAt(monitor, point);
        if (!layer || !layer->resource())
            return;
        info.cancelled = true;
        const auto origin = layer->m_geometry.pos();
        keyboardTouches[event.touchID] = {layer, origin, monitor};
        g_pSeatManager->sendTouchDown(layer->resource(), event.timeMs, event.touchID, point - origin);
        g_pSeatManager->sendTouchFrame();
    });
    motion = touch.motion.listen([](const ITouch::SMotionEvent& event, Event::SCallbackInfo& info) {
        if (swipe && swipe->id == event.touchID) {
            info.cancelled = true;
            swipe->latest = event.pos;
            return;
        }
        const auto it = keyboardTouches.find(event.touchID);
        if (it == keyboardTouches.end())
            return;
        info.cancelled = true;
        const auto monitor = it->second.monitor.lock();
        if (!monitor || !it->second.layer.lock())
            return;
        g_pSeatManager->sendTouchMotion(event.timeMs, event.touchID,
                                        layoutPoint(monitor, event.pos) - it->second.origin);
        g_pSeatManager->sendTouchFrame();
    });
    up = touch.up.listen([](const ITouch::SUpEvent& event, Event::SCallbackInfo& info) {
        if (keyboardTouches.erase(event.touchID)) {
            info.cancelled = true;
            g_pSeatManager->sendTouchUp(event.timeMs, event.touchID);
            g_pSeatManager->sendTouchFrame();
            return;
        }
        if (!swipe || swipe->id != event.touchID)
            return;
        info.cancelled = true;
        const auto gesture = *swipe;
        swipe.reset();
        if (locked())
            return;
        const auto moved = gesture.latest - gesture.start;
        if (std::abs(moved.x) < 0.012 && std::abs(moved.y) < 0.012)
            replayTap(gesture, event.timeMs);
        else
            dispatch(gesture);
    });
    cancel = touch.cancel.listen([](const ITouch::SCancelEvent& event, Event::SCallbackInfo&) {
        if (swipe && swipe->id == event.touchID)
            swipe.reset();
        if (keyboardTouches.erase(event.touchID))
            g_pSeatManager->sendTouchCancel();
    });
    return {"tablet-swipes", "Tablet touchscreen edge gestures; floatkb touches keep focus", "local", "1.2"};
}

APICALL EXPORT void PLUGIN_EXIT() {
    cancel.reset();
    up.reset();
    motion.reset();
    down.reset();
    swipe.reset();
    keyboardTouches.clear();
}
