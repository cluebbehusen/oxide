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

/// How many control groups exist. The classic map authors Ctrl+digit
/// only this far: an exact chord to a group dispatch ignores would
/// outrank the bare-digit fallback and swallow palette picks whenever
/// Ctrl is held.
pub const CONTROL_GROUPS: usize = 5;

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
    /// Train the first compatible selected producer's Nth roster slot (0-based).
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
    /// Select (and center on) the next idle own harvester.
    CycleIdleWorker,
    /// Center the camera where trouble last landed.
    JumpToLastAlert,
    /// Arm salvage: the next click on an own built building sends the
    /// selected harvesters to strip it for a partial refund.
    Salvage,
    /// Arm run: the next ground click sends the selection walking
    /// WITHOUT firing — the strict disengage and recall verb.
    Run,
    /// Arm attack-move: the next ground click sends the selection
    /// fighting and chasing along the route.
    AttackMove,
    /// Remember the camera position in slot 0-3.
    SetBookmark(u8),
    /// Return the camera to a remembered slot.
    RecallBookmark(u8),
    /// Arm the unit weld: the next click on a damaged own ground unit
    /// sends the selected harvesters to weld it back up (billed per hp
    /// against the machine's cost).
    RepairUnit,
}

impl Action {
    /// The player-facing name, e.g. in the remap screen's conflict
    /// notice ("M is already bound to Run"). Exhaustive on purpose:
    /// non-remappable holders (Confirm, the digits) are reachable
    /// conflicts, and every one must be nameable.
    pub fn label(self) -> String {
        match self {
            Action::PanLeft => "Pan left".to_string(),
            Action::PanRight => "Pan right".to_string(),
            Action::PanUp => "Pan up".to_string(),
            Action::PanDown => "Pan down".to_string(),
            Action::StopOrScrap => "Stop".to_string(),
            Action::TrainSlot(n) => format!("Train slot {}", n + 1),
            Action::TogglePause => "Pause".to_string(),
            Action::ToggleBuildPalette => "Build palette".to_string(),
            Action::Patrol => "Patrol".to_string(),
            Action::ToggleOverlay => "Debug overlay".to_string(),
            Action::Back => "Back".to_string(),
            Action::HomeCamera => "Center home".to_string(),
            Action::Slot(n) => format!("Slot {n}"),
            Action::AssignGroup(n) => format!("Assign group {n}"),
            Action::Confirm => "Confirm".to_string(),
            Action::CycleIdleWorker => "Next idle harvester".to_string(),
            Action::JumpToLastAlert => "Jump to last alert".to_string(),
            Action::Salvage => "Salvage".to_string(),
            Action::Run => "Run".to_string(),
            Action::AttackMove => "Attack-move".to_string(),
            Action::SetBookmark(n) => format!("Set bookmark {}", n + 1),
            Action::RecallBookmark(n) => format!("Recall bookmark {}", n + 1),
            Action::RepairUnit => "Weld unit".to_string(),
        }
    }
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
            Binding {
                chord: Chord::bare(Key::N),
                action: Action::CycleIdleWorker,
            },
            Binding {
                chord: Chord::bare(Key::A),
                action: Action::JumpToLastAlert,
            },
            Binding {
                chord: Chord::bare(Key::V),
                action: Action::Salvage,
            },
            Binding {
                chord: Chord::bare(Key::M),
                action: Action::Run,
            },
            Binding {
                chord: Chord::bare(Key::F),
                action: Action::AttackMove,
            },
            Binding {
                chord: Chord::bare(Key::W),
                action: Action::RepairUnit,
            },
        ];
        for (i, key) in [Key::F5, Key::F6, Key::F7, Key::F8].into_iter().enumerate() {
            bindings.push(Binding {
                chord: Chord::ctrl(key),
                action: Action::SetBookmark(i as u8),
            });
            bindings.push(Binding {
                chord: Chord::bare(key),
                action: Action::RecallBookmark(i as u8),
            });
        }
        for (i, key) in digits.into_iter().enumerate() {
            let n = (i + 1) as u8;
            bindings.push(Binding {
                chord: Chord::bare(key),
                action: Action::Slot(n),
            });
            if (n as usize) <= CONTROL_GROUPS {
                bindings.push(Binding {
                    chord: Chord::ctrl(key),
                    action: Action::AssignGroup(n),
                });
            }
        }
        Self { bindings }
    }

    /// The left-handed profile: every verb mirrored onto the right
    /// hand (mouse in the left), pans staying on the arrows. Same
    /// grammar, other hemisphere.
    pub fn left_handed() -> Self {
        let mut map = Self::classic();
        for (action, key) in [
            (Action::TrainSlot(0), Key::K),
            (Action::TrainSlot(1), Key::L),
            (Action::StopOrScrap, Key::M),
            (Action::ToggleBuildPalette, Key::N),
            (Action::Patrol, Key::O),
            (Action::CycleIdleWorker, Key::U),
            (Action::JumpToLastAlert, Key::I),
            (Action::TogglePause, Key::P),
            // Every gameplay verb crosses over — Salvage shipped after
            // this preset and once stayed marooned on classic's V.
            (Action::Salvage, Key::J),
            // Classic's M belongs to StopOrScrap over here; Run takes
            // the freed right-index H (TrainSlot 1 moved to K).
            (Action::Run, Key::H),
            // The explicit fighting march sits beside Run.
            (Action::AttackMove, Key::G),
            // Weld crosses to the right hand's remaining top-row key.
            (Action::RepairUnit, Key::Y),
        ] {
            // Order matters: unbind the target key's old meaning first
            // so the rebind never reports a conflict.
            if let Some(holder) = map
                .bindings
                .iter()
                .find(|b| b.chord == Chord::bare(key) && b.action != action)
                .map(|b| b.action)
            {
                map.unbind(holder);
            }
            map.rebind(action, Chord::bare(key));
        }
        map
    }

    /// Resolves a pressed key under the current modifier truth. Graded
    /// matching: exact chord, then same-Ctrl-ignoring-Shift, then bare —
    /// so Ctrl+Shift+1 still assigns a group (Shift often lingers from
    /// queueing orders) and a held modifier never mutes an unmodified
    /// binding.
    pub fn resolve(&self, key: Key, ctrl: bool, shift: bool) -> Option<Action> {
        let rows = || self.bindings.iter().filter(move |b| b.chord.key == key);
        rows()
            .find(|b| b.chord.ctrl == ctrl && b.chord.shift == shift)
            .or_else(|| rows().find(|b| b.chord.ctrl == ctrl && !b.chord.shift))
            .or_else(|| rows().find(|b| !b.chord.ctrl && !b.chord.shift))
            .map(|b| b.action)
    }

    /// The chord bound to an action, for labels and the remap screen.
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
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Rebinds an action's chord. Refused (false) when the chord would
    /// collide with a different action's exact chord — the remap screen
    /// reports the conflict instead of silently shadowing a binding.
    /// Removes an action's binding entirely (the row reads "unbound").
    pub fn unbind(&mut self, action: Action) {
        self.bindings.retain(|b| b.action != action);
    }

    /// The action currently holding an exact chord, if any — the
    /// non-mutating twin of [`Self::rebind`]'s refusal rule, so a
    /// refused remap can name what it collided with.
    pub fn holder(&self, chord: Chord) -> Option<Action> {
        self.bindings
            .iter()
            .find(|b| b.chord == chord)
            .map(|b| b.action)
    }

    pub fn rebind(&mut self, action: Action, chord: Chord) -> bool {
        if self
            .bindings
            .iter()
            .any(|b| b.chord == chord && b.action != action)
        {
            return false;
        }
        match self.bindings.iter_mut().find(|b| b.action == action) {
            Some(binding) => binding.chord = chord,
            None => self.bindings.push(Binding { chord, action }),
        }
        true
    }

    /// Human label for a chord, e.g. "Ctrl+3" or "Space".
    pub fn chord_label(chord: Chord) -> String {
        let key = match chord.key {
            Key::Num1 => "1",
            Key::Num2 => "2",
            Key::Num3 => "3",
            Key::Num4 => "4",
            Key::Num5 => "5",
            Key::Num6 => "6",
            Key::Num7 => "7",
            Key::Num8 => "8",
            Key::Num9 => "9",
            other => return Self::modifier_prefix(chord) + &format!("{other:?}"),
        };
        Self::modifier_prefix(chord) + key
    }

    fn modifier_prefix(chord: Chord) -> String {
        let mut out = String::new();
        if chord.ctrl {
            out.push_str("Ctrl+");
        }
        if chord.shift {
            out.push_str("Shift+");
        }
        out
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

    /// Whether Ctrl is held (the type strip's remove-this-kind click).
    pub fn ctrl_held(&self) -> bool {
        self.ctrl
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
    fn the_left_handed_preset_crosses_every_gameplay_verb_over() {
        // The preset's guarantee: verbs live on the right hand. Salvage
        // shipped after the preset and once stayed marooned on V.
        let map = BindingMap::left_handed();
        assert_eq!(
            map.chord_for(Action::Salvage),
            Some(Chord::bare(Key::J)),
            "salvage crossed over with the rest"
        );
        assert_eq!(
            map.chord_for(Action::StopOrScrap),
            Some(Chord::bare(Key::M))
        );
        assert_eq!(
            map.chord_for(Action::AttackMove),
            Some(Chord::bare(Key::G)),
            "attack-move crosses beside run"
        );
    }

    #[test]
    fn rebinding_refuses_collisions_and_moves_the_chord() {
        let mut map = BindingMap::classic();
        assert!(
            !map.rebind(Action::StopOrScrap, Chord::bare(Key::H)),
            "H belongs to train slot 0; shadowing must be refused"
        );
        assert!(map.rebind(Action::StopOrScrap, Chord::ctrl(Key::X)));
        assert_eq!(map.resolve(Key::X, true, false), Some(Action::StopOrScrap));
        assert_eq!(
            map.resolve(Key::X, false, false),
            None,
            "the old chord is gone, not shadowed"
        );
    }

    #[test]
    fn ctrl_digits_past_the_group_count_fall_through_to_slots() {
        let map = BindingMap::classic();
        assert!(
            map.bindings().iter().all(|b| match b.action {
                Action::AssignGroup(n) => (n as usize) <= CONTROL_GROUPS,
                _ => true,
            }),
            "no chord may point at a group dispatch ignores"
        );
        assert_eq!(
            map.resolve(Key::Num7, true, false),
            Some(Action::Slot(7)),
            "ctrl+7 reaches the palette, not a phantom group"
        );
    }

    #[test]
    fn shift_never_flips_an_assign_into_a_recall() {
        // Shift lingers after queueing orders; Ctrl+Shift+digit must
        // still mean assign, as the classic layout always had it.
        let map = BindingMap::classic();
        assert_eq!(
            map.resolve(Key::Num4, true, true),
            Some(Action::AssignGroup(4))
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
            Key::F,
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
