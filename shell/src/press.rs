//! The pointer gesture every menu surface shares: a press arms the zone
//! under it, and only a release on that same zone commits. Dragging away
//! cancels, and the first finger down owns a touch gesture until it lifts.
//!
//! A screen supplies its own hit test and calls in with whichever zone the
//! pointer is over; this type only remembers what was armed.

/// Armed pointer state over a screen's hit zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press<Z> {
    mouse: Option<Z>,
    touch: Option<(u64, Z)>,
}

impl<Z> Default for Press<Z> {
    fn default() -> Self {
        Self {
            mouse: None,
            touch: None,
        }
    }
}

impl<Z: Copy + PartialEq> Press<Z> {
    /// Arms the zone under a mouse press, or disarms when it hit nothing.
    pub fn mouse_down(&mut self, zone: Option<Z>) {
        self.mouse = zone;
    }

    /// Resolves the mouse press: the armed zone, if the release landed on it.
    pub fn mouse_up(&mut self, zone: Option<Z>) -> Option<Z> {
        self.mouse.take().filter(|armed| zone == Some(*armed))
    }

    /// Whether a new finger may start a gesture. A touch that armed nothing
    /// owns nothing, so the next finger is free to try.
    pub fn touch_free(&self) -> bool {
        self.touch.is_none()
    }

    /// Arms the zone under a fresh touch. Call only while [`Self::touch_free`].
    pub fn touch_down(&mut self, id: u64, zone: Option<Z>) {
        self.touch = zone.map(|zone| (id, zone));
    }

    /// Whether `id` is the finger that armed the current touch gesture.
    pub fn owns(&self, id: u64) -> bool {
        self.touch.is_some_and(|(finger, _)| finger == id)
    }

    /// Resolves the touch gesture: the armed zone, if the owning finger
    /// lifted on it. Call only for the finger that [`Self::owns`] it.
    pub fn touch_up(&mut self, zone: Option<Z>) -> Option<Z> {
        let (_, armed) = self.touch.take()?;
        (zone == Some(armed)).then_some(armed)
    }

    /// Drops whatever is armed.
    pub fn cancel(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub fn armed_touch(&self) -> Option<(u64, Z)> {
        self.touch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mouse_press_commits_only_when_released_on_the_armed_zone() {
        let mut press = Press::default();
        press.mouse_down(Some(2));
        assert_eq!(press.mouse_up(Some(2)), Some(2));

        press.mouse_down(Some(2));
        assert_eq!(press.mouse_up(Some(3)), None);
        assert_eq!(press.mouse_up(Some(2)), None, "a cancelled press is spent");

        press.mouse_down(None);
        assert_eq!(press.mouse_up(Some(2)), None);
    }

    #[test]
    fn a_touch_gesture_belongs_to_the_finger_that_armed_it() {
        let mut press = Press::default();
        assert!(press.touch_free());
        press.touch_down(7, Some(1));
        assert!(!press.touch_free());
        assert!(press.owns(7));
        assert!(!press.owns(8));

        assert_eq!(press.touch_up(Some(0)), None, "lifting elsewhere cancels");
        assert!(press.touch_free());

        press.touch_down(9, Some(0));
        assert_eq!(press.touch_up(Some(0)), Some(0));
    }

    #[test]
    fn a_touch_that_hits_nothing_leaves_the_gesture_free() {
        let mut press = Press::<usize>::default();
        press.touch_down(7, None);
        assert!(press.touch_free());
        assert!(!press.owns(7));
    }

    #[test]
    fn cancel_disarms_mouse_and_touch() {
        let mut press = Press::default();
        press.mouse_down(Some(4));
        press.touch_down(7, Some(4));
        press.cancel();
        assert_eq!(press.mouse_up(Some(4)), None);
        assert!(press.touch_free());
    }
}
