//! Semantic actions: what the player *means*, decoupled from which key
//! says it.
//!
//! `poll_events` stays the only hardware reader; everything downstream
//! of a `RawEvent` resolves through a [`BindingMap`] into [`Action`]s.
//! Menus, remapping, tooltips, the HUD's key labels, and the automation
//! harness all read the same table — a binding changed in settings
//! updates every one of them, because there is only one of it.

use oxide_protocol::Key;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Something the player can mean. Payload-free variants bind directly;
/// numbered variants bind per index (digits, group slots).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    /// Continuous camera pan (held).
    PanLeft,
    /// Continuous camera pan (held).
    PanRight,
    /// Continuous camera pan (held).
    PanUp,
    /// Continuous camera pan (held).
    PanDown,
    /// Halt selected units, or scrap a selected unfinished site.
    StopOrScrap,
    /// Train the selected factory's Nth roster slot (0-based).
    TrainSlot(u8),
    /// Toggle the sim clock.
    TogglePause,
    /// Open or close the build palette.
    ToggleBuildPalette,
    /// Arm a patrol route / send the armed circuit.
    Patrol,
    /// Toggle the debug overlay.
    ToggleOverlay,
    /// Unwind one layer: palette, placement, patrol, selection — then
    /// (in the screen stack's hands) the pause menu.
    Back,
    /// Center the camera on the home Foundry.
    HomeCamera,
    /// Contextual digit 1-9: build palette pick, factory produce slot,
    /// or control-group recall — context decides, the key just counts.
    Slot(u8),
    /// Assign the selection to control group N.
    AssignGroup(u8),
    /// Menu confirm.
    Confirm,
}

/// A physical chord: one key plus the modifier truth that must hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord {
    /// The main key.
    pub key: Key,
    /// Whether Ctrl must be held.
    #[serde(default)]
    pub ctrl: bool,
    /// Whether Shift must be held.
    #[serde(default)]
    pub shift: bool,
}

impl Chord {
    /// A bare, modifier-free chord.
    pub fn bare(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
        }
    }

    /// The same key with Ctrl required.
    pub fn ctrl(key: Key) -> Self {
        Self {
            key,
            ctrl: true,
            shift: false,
        }
    }
}

/// One binding row. Bindings are data — the settings screen edits them,
/// the config file persists them, labels render from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// The chord that fires it.
    pub chord: Chord,
    /// What it means.
    pub action: Action,
}

/// The resolution table.
///
/// Matching is exact-chord first, then a bare-chord fallback: `ctrl+1`
/// resolves to its own binding when one exists, while `ctrl+H` (no
/// exact row) still trains — held modifiers never mute an unmodified
/// binding, which is how the classic layout always behaved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingMap {
    bindings: Vec<Binding>,
}

impl BindingMap {
    /// The "Oxide Classic" profile: every pre-0.9 shortcut, unchanged.
    pub fn classic() -> Self {
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        let mut bindings = vec![
            Binding {
                chord: Chord::bare(Key::Left),
                action: Action::PanLeft,
            },
            Binding {
                chord: Chord::bare(Key::Right),
                action: Action::PanRight,
            },
            Binding {
                chord: Chord::bare(Key::Up),
                action: Action::PanUp,
            },
            Binding {
                chord: Chord::bare(Key::Down),
                action: Action::PanDown,
            },
            Binding {
                chord: Chord::bare(Key::X),
                action: Action::StopOrScrap,
            },
            Binding {
                chord: Chord::bare(Key::H),
                action: Action::TrainSlot(0),
            },
            Binding {
                chord: Chord::bare(Key::S),
                action: Action::TrainSlot(1),
            },
            Binding {
                chord: Chord::bare(Key::P),
                action: Action::TogglePause,
            },
            Binding {
                chord: Chord::bare(Key::B),
                action: Action::ToggleBuildPalette,
            },
            Binding {
                chord: Chord::bare(Key::R),
                action: Action::Patrol,
            },
            Binding {
                chord: Chord::bare(Key::F1),
                action: Action::ToggleOverlay,
            },
            Binding {
                chord: Chord::bare(Key::Escape),
                action: Action::Back,
            },
            Binding {
                chord: Chord::bare(Key::Space),
                action: Action::HomeCamera,
            },
            Binding {
                chord: Chord::bare(Key::Enter),
                action: Action::Confirm,
            },
        ];
        for (i, key) in digits.into_iter().enumerate() {
            let n = (i + 1) as u8;
            bindings.push(Binding {
                chord: Chord::bare(key),
                action: Action::Slot(n),
            });
            bindings.push(Binding {
                chord: Chord::ctrl(key),
                action: Action::AssignGroup(n),
            });
        }
        Self { bindings }
    }

    /// Resolves a pressed key under the current modifier truth.
    pub fn resolve(&self, key: Key, ctrl: bool, shift: bool) -> Option<Action> {
        let exact = self
            .bindings
            .iter()
            .find(|b| b.chord.key == key && b.chord.ctrl == ctrl && b.chord.shift == shift);
        if let Some(b) = exact {
            return Some(b.action);
        }
        self.bindings
            .iter()
            .find(|b| b.chord.key == key && !b.chord.ctrl && !b.chord.shift)
            .map(|b| b.action)
    }

    /// The chord bound to an action, for labels and the remap screen.
    // Consumed by the Phase D settings screens; tests exercise it now.
    #[allow(dead_code)]
    pub fn chord_for(&self, action: Action) -> Option<Chord> {
        self.bindings
            .iter()
            .find(|b| b.action == action)
            .map(|b| b.chord)
    }

    /// Every pair of bindings sharing an exact chord — a conflict makes
    /// one row unreachable, and the settings screen must say so.
    // Consumed by the Phase D settings screens; tests exercise it now.
    #[allow(dead_code)]
    pub fn conflicts(&self) -> Vec<(Binding, Binding)> {
        let mut out = Vec::new();
        for (i, a) in self.bindings.iter().enumerate() {
            for b in self.bindings.iter().skip(i + 1) {
                if a.chord == b.chord {
                    out.push((*a, *b));
                }
            }
        }
        out
    }

    /// All bindings, for rendering the remap screen.
    // Consumed by the Phase D settings screens; tests exercise it now.
    #[allow(dead_code)]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
}

impl Default for BindingMap {
    fn default() -> Self {
        Self::classic()
    }
}

/// The edge a key event produced, with releases paired to what the
/// *press* meant — a modifier released mid-hold must not orphan a pan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionEvent {
    /// The chord fired.
    Pressed(Action),
    /// The chord's key came back up.
    Released(Action),
}

/// Stateful resolver: tracks modifier truth and pairs each release
/// with the action its press resolved to.
#[derive(Debug, Default)]
pub struct ActionResolver {
    ctrl: bool,
    shift: bool,
    active: HashMap<Key, Action>,
}

impl ActionResolver {
    /// Feeds one key edge; returns the semantic edge, if any.
    pub fn key_edge(&mut self, map: &BindingMap, key: Key, down: bool) -> Option<ActionEvent> {
        match key {
            Key::Ctrl => {
                self.ctrl = down;
                return None;
            }
            Key::Shift => {
                self.shift = down;
                return None;
            }
            _ => {}
        }
        if down {
            let action = map.resolve(key, self.ctrl, self.shift)?;
            self.active.insert(key, action);
            Some(ActionEvent::Pressed(action))
        } else {
            self.active.remove(&key).map(ActionEvent::Released)
        }
    }

    /// Whether an action's chord is currently held (continuous pans).
    pub fn is_held(&self, action: Action) -> bool {
        self.active.values().any(|&a| a == action)
    }

    /// Whether Shift is held (queue-order semantics live on clicks).
    pub fn shift_held(&self) -> bool {
        self.shift
    }

    /// Drops all held state — mode transitions eat release events, and
    /// stale holds otherwise pan forever.
    pub fn clear(&mut self) {
        self.ctrl = false;
        self.shift = false;
        self.active.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_chords_beat_bare_ones_and_bare_survives_modifiers() {
        let map = BindingMap::classic();
        assert_eq!(
            map.resolve(Key::Num1, true, false),
            Some(Action::AssignGroup(1)),
            "ctrl+1 is its own meaning"
        );
        assert_eq!(map.resolve(Key::Num1, false, false), Some(Action::Slot(1)));
        assert_eq!(
            map.resolve(Key::H, true, false),
            Some(Action::TrainSlot(0)),
            "a held modifier never mutes an unmodified binding"
        );
    }

    #[test]
    fn releases_pair_with_what_the_press_meant() {
        let map = BindingMap::classic();
        let mut r = ActionResolver::default();
        assert_eq!(
            r.key_edge(&map, Key::Ctrl, true),
            None,
            "modifiers are truth, not actions"
        );
        assert_eq!(
            r.key_edge(&map, Key::Num2, true),
            Some(ActionEvent::Pressed(Action::AssignGroup(2)))
        );
        // Ctrl comes up before the digit: the release still closes the
        // assign, not a phantom recall.
        r.key_edge(&map, Key::Ctrl, false);
        assert_eq!(
            r.key_edge(&map, Key::Num2, false),
            Some(ActionEvent::Released(Action::AssignGroup(2)))
        );
    }

    #[test]
    fn held_pans_read_back_until_released() {
        let map = BindingMap::classic();
        let mut r = ActionResolver::default();
        r.key_edge(&map, Key::Left, true);
        assert!(r.is_held(Action::PanLeft));
        r.key_edge(&map, Key::Left, false);
        assert!(!r.is_held(Action::PanLeft));
    }

    #[test]
    fn the_classic_profile_has_no_conflicts_and_covers_the_old_map() {
        let map = BindingMap::classic();
        assert!(map.conflicts().is_empty());
        // Every key the old hardcoded switchboard answered resolves to
        // something; a silent hole would be a lost shortcut.
        for key in [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::X,
            Key::H,
            Key::S,
            Key::P,
            Key::B,
            Key::R,
            Key::F1,
            Key::Escape,
            Key::Space,
            Key::Enter,
            Key::Num1,
            Key::Num9,
        ] {
            assert!(
                map.resolve(key, false, false).is_some(),
                "{key:?} lost its meaning"
            );
        }
    }

    #[test]
    fn clear_forgets_holds_and_modifiers() {
        let map = BindingMap::classic();
        let mut r = ActionResolver::default();
        r.key_edge(&map, Key::Ctrl, true);
        r.key_edge(&map, Key::Up, true);
        r.clear();
        assert!(!r.is_held(Action::PanUp));
        assert_eq!(
            r.key_edge(&map, Key::Num3, true),
            Some(ActionEvent::Pressed(Action::Slot(3))),
            "ctrl must not survive a clear"
        );
    }
}
