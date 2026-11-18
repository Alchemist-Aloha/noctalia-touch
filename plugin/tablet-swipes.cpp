#include <cmath>
#include <optional>
#include <stdexcept>
#include <string_view>

#ifndef TABLET_OUTPUT
#define TABLET_OUTPUT "DSI-1"
#endif

#include <hyprland/src/event/EventBus.hpp>
#include <hyprland/src/managers/SessionLockManager.hpp>
#include <hyprland/src/plugins/PluginAPI.hpp>

namespace {
enum class Edge { top, bottom, left, right };

struct Swipe {
    int id;
    Edge edge;
    Vector2D start;
    Vector2D latest;
};

std::optional<Swipe> swipe;
CHyprSignalListener down, motion, up, cancel;

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
            swipe = Swipe{event.touchID, *edge, event.pos, event.pos};
            info.cancelled = true;
        }
    });
    motion = touch.motion.listen([](const ITouch::SMotionEvent& event, Event::SCallbackInfo& info) {
        if (swipe && swipe->id == event.touchID) {
            info.cancelled = true;
            swipe->latest = event.pos;
        }
    });
    up = touch.up.listen([](const ITouch::SUpEvent& event, Event::SCallbackInfo& info) {
        if (!swipe || swipe->id != event.touchID)
            return;
        info.cancelled = true;
        const auto gesture = *swipe;
        swipe.reset();
        if (!locked())
            dispatch(gesture);
    });
    cancel = touch.cancel.listen([](const ITouch::SCancelEvent& event, Event::SCallbackInfo&) {
        if (swipe && swipe->id == event.touchID)
            swipe.reset();
    });
    return {"tablet-swipes", "Tablet touchscreen edge gestures", "local", "1.0"};
}

APICALL EXPORT void PLUGIN_EXIT() {
    cancel.reset();
    up.reset();
    motion.reset();
    down.reset();
    swipe.reset();
}
