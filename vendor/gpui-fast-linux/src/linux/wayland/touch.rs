use collections::HashMap;
use gpui::{Pixels, Point, TouchEvent, TouchId, TouchPhase};

pub(super) struct TouchSequence<T> {
    active: HashMap<i32, (T, TouchEvent)>,
    pending: Vec<(T, TouchEvent)>,
    next_id: u64,
}

impl<T> Default for TouchSequence<T> {
    fn default() -> Self {
        Self {
            active: HashMap::default(),
            pending: Vec::new(),
            next_id: 0,
        }
    }
}

impl<T: Clone + PartialEq> TouchSequence<T> {
    pub(super) fn down(&mut self, id: i32, target: T, position: Point<Pixels>) {
        if self.active.contains_key(&id) {
            return;
        }
        // The protocol can reuse a wire ID immediately after Up. GPUI IDs must
        // not be reused for subsequent contacts, even in the same frame.
        let event = TouchEvent {
            id: TouchId(self.next_id),
            phase: TouchPhase::Started,
            position,
            ..TouchEvent::default()
        };
        self.next_id += 1;
        self.active.insert(id, (target.clone(), event.clone()));
        self.pending.push((target, event));
    }

    pub(super) fn motion(&mut self, id: i32, position: Point<Pixels>) {
        if let Some((target, event)) = self.active.get_mut(&id) {
            event.position = position;
            event.phase = TouchPhase::Moved;
            self.pending.push((target.clone(), event.clone()));
        }
    }

    pub(super) fn up(&mut self, id: i32) {
        if let Some((target, mut event)) = self.active.remove(&id) {
            event.phase = TouchPhase::Ended;
            self.pending.push((target, event));
        }
    }

    pub(super) fn cancel(&mut self) -> Vec<(T, TouchEvent)> {
        let mut events = self.frame();
        events.extend(self.active.drain().map(|(_, (target, mut event))| {
            event.phase = TouchPhase::Cancelled;
            (target, event)
        }));
        events
    }

    pub(super) fn frame(&mut self) -> Vec<(T, TouchEvent)> {
        std::mem::take(&mut self.pending)
    }

    pub(super) fn forget(&mut self, target: &T) {
        self.active.retain(|_, (current, _)| current != target);
        self.pending.retain(|(current, _)| current != target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn contacts_keep_their_down_target_and_last_position() {
        let mut touches = TouchSequence::default();
        let start = point(px(1.), px(2.));
        let moved = point(px(3.), px(4.));
        touches.down(-5, "first", start);
        touches.down(0, "second", start);
        touches.motion(-5, moved);
        touches.up(-5);
        let frame = touches.frame();
        let first = frame[0].1.id;
        assert_ne!(first, frame[1].1.id);
        assert_eq!(
            frame
                .iter()
                .map(|(target, event)| (*target, event.phase, event.position))
                .collect::<Vec<_>>(),
            [
                ("first", TouchPhase::Started, start),
                ("second", TouchPhase::Started, start),
                ("first", TouchPhase::Moved, moved),
                ("first", TouchPhase::Ended, moved),
            ]
        );
        assert_eq!(frame[2].1.id, first);
        assert_eq!(frame[3].1.id, first);
        touches.down(-5, "second", start);
        assert_ne!(touches.frame()[0].1.id, first);
    }

    #[test]
    fn cancel_releases_contacts_without_waiting_for_another_frame() {
        let mut touches = TouchSequence::default();
        let start = point(px(1.), px(2.));
        let moved = point(px(3.), px(4.));
        touches.down(0, "window", start);
        let id = touches.frame()[0].1.id;
        touches.motion(0, moved);
        let cancelled = touches.cancel();
        assert_eq!(cancelled[0].1.phase, TouchPhase::Moved);
        assert_eq!(cancelled[1].1.id, id);
        assert_eq!(cancelled[1].1.phase, TouchPhase::Cancelled);
        assert_eq!(cancelled[1].1.position, moved);
        touches.up(0);
        touches.motion(0, moved);
        assert!(touches.frame().is_empty());
    }

    #[test]
    fn forgetting_a_window_does_not_disturb_other_contacts() {
        let mut touches = TouchSequence::default();
        let start = point(px(1.), px(2.));
        touches.down(0, "closed", start);
        touches.down(1, "open", start);
        touches.forget(&"closed");
        touches.up(0);
        touches.up(1);
        assert_eq!(
            touches
                .frame()
                .iter()
                .map(|(target, _)| *target)
                .collect::<Vec<_>>(),
            ["open", "open"]
        );
    }
}
