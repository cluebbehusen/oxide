//! Contextual keyboard actions, shared by input, hints, settings, and persistence.

use oxide_protocol::Key;
use oxide_sim::BuildingKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const CONTROL_GROUPS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Empty,
    Units,
    Workers,
    Buildings,
    Production,
    Construction,
    BuildCategory(u8),
    Playback,
    FinalMap,
    Menu,
}

impl Context {
    fn bit(self) -> u16 {
        match self {
            Self::Empty => 1,
            Self::Units => 2,
            Self::Workers => 4096,
            Self::Buildings => 4,
            Self::Production => 8,
            Self::Construction => 16,
            Self::BuildCategory(n) => 32 << n.min(3),
            Self::Playback => 512,
            Self::FinalMap => 1024,
            Self::Menu => 2048,
        }
    }
}
const UNITS: u16 = 2 | 4096;
const LIVE: u16 = 511 | 4096;
const WORLD: u16 = LIVE | 512 | 1024;

/// Categories and card order are shared by rendering and keyboard dispatch.
pub const BUILD_CATEGORIES: [(&str, &[BuildingKind]); 4] = [
    (
        "Economy",
        &[BuildingKind::Reclaimer, BuildingKind::Extractor],
    ),
    (
        "Production",
        &[
            BuildingKind::Fabricator,
            BuildingKind::Foundry,
            BuildingKind::Airworks,
            BuildingKind::Crucible,
        ],
    ),
    (
        "Defense",
        &[
            BuildingKind::Turret,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
            BuildingKind::Barricade,
        ],
    ),
    (
        "Utility",
        &[
            BuildingKind::Array,
            BuildingKind::RepairBay,
            BuildingKind::ScuttleCharge,
        ],
    ),
];
pub fn building_category(kind: BuildingKind) -> u8 {
    BUILD_CATEGORIES
        .iter()
        .position(|(_, kinds)| kinds.contains(&kind))
        .unwrap_or(0) as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
    StopOrScrap,
    TrainSlot(u8),
    TogglePause,
    ToggleBuildPalette,
    Patrol,
    ToggleOverlay,
    Back,
    HomeCamera,
    /// One-based group recall; the serialized name preserves existing config rows.
    Slot(u8),
    AssignGroup(u8),
    Confirm,
    CycleIdleWorker,
    JumpToLastAlert,
    Salvage,
    Run,
    AttackMove,
    SetBookmark(u8),
    RecallBookmark(u8),
    RepairUnit,
    BuildCategory(u8),
    Build(BuildingKind),
    Upgrade,
    SetRally,
    ClearRally,
    Unload,
    ReplayPause,
    ReplayBack,
    ReplayForward,
    ReplayStart,
    ReplayEnd,
    ReplaySpeed(u8),
    ReplayStats,
    MenuUp,
    MenuDown,
    MenuLeft,
    MenuRight,
    MenuPageUp,
    MenuPageDown,
    MenuHome,
    MenuEnd,
    DeleteSave,
    ReturnCargo,
}
impl Action {
    pub fn label(self) -> String {
        match self {
            Self::PanLeft => "Pan left".into(),
            Self::PanRight => "Pan right".into(),
            Self::PanUp => "Pan up".into(),
            Self::PanDown => "Pan down".into(),
            Self::StopOrScrap => "Stop / scrap site / clear focus".into(),
            Self::TrainSlot(n) => format!("Production slot {}", n + 1),
            Self::TogglePause => "Pause match".into(),
            Self::ToggleBuildPalette => "Construction".into(),
            Self::Patrol => "Patrol".into(),
            Self::ToggleOverlay => "Debug overlay".into(),
            Self::Back => "Back / cancel".into(),
            Self::HomeCamera => "Center home".into(),
            Self::Slot(n) => format!("Recall group {n}"),
            Self::AssignGroup(n) => format!("Assign group {n}"),
            Self::Confirm => "Confirm".into(),
            Self::CycleIdleWorker => "Next idle Harvester".into(),
            Self::JumpToLastAlert => "Jump to last alert".into(),
            Self::Salvage => "Salvage building".into(),
            Self::Run => "Run (move without engaging)".into(),
            Self::AttackMove => "Attack-move".into(),
            Self::SetBookmark(n) => format!("Set camera bookmark {}", n + 1),
            Self::RecallBookmark(n) => format!("Recall camera bookmark {}", n + 1),
            Self::RepairUnit => "Weld unit".into(),
            Self::BuildCategory(n) => format!("{} category", BUILD_CATEGORIES[n as usize].0),
            Self::Build(kind) => format!("Build {}", crate::typography::entity_name(kind.name())),
            Self::Upgrade => "Upgrade building".into(),
            Self::SetRally => "Set rally".into(),
            Self::ClearRally => "Clear rally".into(),
            Self::Unload => "Unload here".into(),
            Self::ReturnCargo => "Return cargo".into(),
            Self::ReplayPause => "Pause replay".into(),
            Self::ReplayBack => "Seek back 25 seconds".into(),
            Self::ReplayForward => "Seek forward 25 seconds".into(),
            Self::ReplayStart => "Replay start".into(),
            Self::ReplayEnd => "Replay end".into(),
            Self::ReplaySpeed(n) => format!("Replay speed {}x", 0.5 * 2_f32.powi(i32::from(n))),
            Self::ReplayStats => "Replay statistics".into(),
            Self::MenuUp => "Menu up".into(),
            Self::MenuDown => "Menu down".into(),
            Self::MenuLeft => "Menu left".into(),
            Self::MenuRight => "Menu right".into(),
            Self::MenuPageUp => "Menu page up".into(),
            Self::MenuPageDown => "Menu page down".into(),
            Self::MenuHome => "Menu first item".into(),
            Self::MenuEnd => "Menu last item".into(),
            Self::DeleteSave => "Delete saved replay".into(),
        }
    }
    fn contexts(self) -> u16 {
        match self {
            Self::PanLeft | Self::PanRight | Self::PanUp | Self::PanDown => WORLD,
            Self::Back => WORLD | 2048,
            Self::TrainSlot(_) | Self::SetRally | Self::ClearRally => 8,
            Self::Upgrade => 4 | 8,
            Self::StopOrScrap => UNITS | 4,
            Self::Patrol => UNITS,
            Self::Salvage | Self::Run | Self::AttackMove | Self::RepairUnit => UNITS | 496,
            Self::Unload => 2,
            Self::ReturnCargo => 4096 | 496,
            Self::BuildCategory(_) => 16,
            Self::Build(kind) => Context::BuildCategory(building_category(kind)).bit(),
            Self::ReplayPause
            | Self::ReplayBack
            | Self::ReplayForward
            | Self::ReplayStart
            | Self::ReplayEnd
            | Self::ReplaySpeed(_)
            | Self::ReplayStats => 512,
            Self::Confirm
            | Self::MenuUp
            | Self::MenuDown
            | Self::MenuLeft
            | Self::MenuRight
            | Self::MenuPageUp
            | Self::MenuPageDown
            | Self::MenuHome
            | Self::MenuEnd
            | Self::DeleteSave => 2048,
            _ => LIVE,
        }
    }
    pub fn available(self, context: Context) -> bool {
        self.contexts() & context.bit() != 0
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.contexts() & other.contexts() != 0
    }
    pub fn sane(self) -> bool {
        match self {
            Self::Slot(n) | Self::AssignGroup(n) => (1..=5).contains(&n),
            Self::TrainSlot(n) => n < 6,
            Self::SetBookmark(n) | Self::RecallBookmark(n) | Self::BuildCategory(n) => n < 4,
            Self::ReplaySpeed(n) => n < 8,
            Self::Build(kind) => BUILD_CATEGORIES
                .iter()
                .any(|(_, kinds)| kinds.contains(&kind)),
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chord {
    pub key: Key,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
}
impl Chord {
    pub fn bare(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
        }
    }
    pub fn ctrl(key: Key) -> Self {
        Self {
            key,
            ctrl: true,
            shift: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub chord: Chord,
    pub action: Action,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingMap {
    bindings: Vec<Binding>,
    #[serde(default)]
    secondary: Vec<Binding>,
    #[serde(default)]
    revision: u8,
}
impl BindingMap {
    pub fn legacy() -> Self {
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
        Self {
            bindings,
            revision: 0,
            secondary: Vec::new(),
        }
    }

    /// The left-handed profile: every verb mirrored onto the right
    /// hand (mouse in the left), pans staying on the arrows. Same
    /// grammar, other hemisphere.
    pub fn legacy_left_handed() -> Self {
        let mut map = Self::legacy();
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

    pub fn classic() -> Self {
        use Action::*;
        let mut map = Self {
            bindings: Vec::new(),
            secondary: Vec::new(),
            revision: 3,
        };
        let defaults = [
            (PanUp, Key::W),
            (PanLeft, Key::A),
            (PanDown, Key::S),
            (PanRight, Key::D),
            (StopOrScrap, Key::X),
            (TogglePause, Key::P),
            (ToggleBuildPalette, Key::B),
            (Patrol, Key::R),
            (ToggleOverlay, Key::F1),
            (Back, Key::Escape),
            (HomeCamera, Key::Space),
            (Confirm, Key::Enter),
            (CycleIdleWorker, Key::N),
            (JumpToLastAlert, Key::Tab),
            (Salvage, Key::V),
            (Run, Key::M),
            (AttackMove, Key::F),
            (RepairUnit, Key::C),
            (Upgrade, Key::U),
            (SetRally, Key::Y),
            (Unload, Key::U),
            (ReturnCargo, Key::U),
            (ReplayPause, Key::Space),
            (ReplayBack, Key::PageUp),
            (ReplayForward, Key::PageDown),
            (ReplayStart, Key::Home),
            (ReplayEnd, Key::End),
            (ReplayStats, Key::Tab),
            (MenuUp, Key::Up),
            (MenuDown, Key::Down),
            (MenuLeft, Key::Left),
            (MenuRight, Key::Right),
            (MenuPageUp, Key::PageUp),
            (MenuPageDown, Key::PageDown),
            (MenuHome, Key::Home),
            (MenuEnd, Key::End),
            (DeleteSave, Key::X),
        ];
        for (action, key) in defaults {
            assert!(map.rebind(action, Chord::bare(key)));
        }
        assert!(map.rebind(
            ClearRally,
            Chord {
                key: Key::Y,
                ctrl: false,
                shift: true
            }
        ));
        for (action, key) in [
            (PanUp, Key::Up),
            (PanLeft, Key::Left),
            (PanDown, Key::Down),
            (PanRight, Key::Right),
        ] {
            assert!(map.rebind_slot(action, 1, Chord::bare(key)));
        }
        for (n, key) in [Key::Q, Key::E, Key::R, Key::T, Key::Z, Key::X]
            .into_iter()
            .enumerate()
        {
            assert!(map.rebind(TrainSlot(n as u8), Chord::bare(key)));
            if n < 4 {
                assert!(map.rebind(BuildCategory(n as u8), Chord::bare(key)));
                for (_, kinds) in BUILD_CATEGORIES {
                    if let Some(kind) = kinds.get(n) {
                        assert!(map.rebind(Build(*kind), Chord::bare(key)));
                    }
                }
            }
        }
        for (n, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
        ]
        .into_iter()
        .enumerate()
        {
            assert!(map.rebind(ReplaySpeed(n as u8), Chord::bare(key)));
            if n < CONTROL_GROUPS {
                assert!(map.rebind(Slot(n as u8 + 1), Chord::bare(key)));
                assert!(map.rebind(AssignGroup(n as u8 + 1), Chord::ctrl(key)));
            }
        }
        for (n, key) in [Key::F5, Key::F6, Key::F7, Key::F8].into_iter().enumerate() {
            assert!(map.rebind(SetBookmark(n as u8), Chord::ctrl(key)));
            assert!(map.rebind(RecallBookmark(n as u8), Chord::bare(key)));
        }
        map
    }
    pub fn left_handed() -> Self {
        let mut map = Self::classic();
        // A permutation preserves all contextual conflicts and both binding slots.
        for binding in map.bindings.iter_mut().chain(&mut map.secondary) {
            binding.chord.key = match binding.chord.key {
                Key::W => Key::I,
                Key::I => Key::W,
                Key::A => Key::J,
                Key::J => Key::A,
                Key::S => Key::K,
                Key::K => Key::S,
                Key::D => Key::L,
                Key::L => Key::D,
                Key::Q => Key::O,
                Key::O => Key::Q,
                Key::E => Key::U,
                Key::U => Key::E,
                Key::R => Key::Y,
                Key::Y => Key::R,
                Key::Z => Key::N,
                Key::N => Key::Z,
                Key::X => Key::M,
                Key::M => Key::X,
                Key::C => Key::H,
                Key::H => Key::C,
                Key::V => Key::G,
                Key::G => Key::V,
                other => other,
            };
        }
        map
    }
    fn rows(&self) -> impl Iterator<Item = &Binding> {
        self.bindings.iter().chain(&self.secondary)
    }
    pub fn resolve_in(
        &self,
        key: Key,
        ctrl: bool,
        shift: bool,
        context: Context,
    ) -> Option<Action> {
        self.resolve_where(key, ctrl, shift, |a| a.available(context))
    }
    fn resolve_where(
        &self,
        key: Key,
        ctrl: bool,
        shift: bool,
        accepts: impl Fn(Action) -> bool,
    ) -> Option<Action> {
        let rows = || {
            self.rows()
                .filter(|b| b.chord.key == key && accepts(b.action))
        };
        rows()
            .find(|b| b.chord.ctrl == ctrl && b.chord.shift == shift)
            .or_else(|| rows().find(|b| b.chord.ctrl == ctrl && !b.chord.shift))
            .or_else(|| rows().find(|b| !b.chord.ctrl && !b.chord.shift))
            .map(|b| b.action)
    }
    #[cfg(test)]
    pub fn resolve(&self, key: Key, ctrl: bool, shift: bool) -> Option<Action> {
        self.resolve_where(key, ctrl, shift, |_| true)
    }
    /// Existing menus consume canonical navigation events after this binding boundary.
    pub fn menu_events(
        &self,
        events: &[oxide_protocol::RawEvent],
        mut ctrl: bool,
        mut shift: bool,
    ) -> Vec<oxide_protocol::RawEvent> {
        use oxide_protocol::RawEvent;
        events
            .iter()
            .filter_map(|event| {
                let key = match event {
                    RawEvent::KeyDown { key: Key::Ctrl } => {
                        ctrl = true;
                        return None;
                    }
                    RawEvent::KeyUp { key: Key::Ctrl } => {
                        ctrl = false;
                        return None;
                    }
                    RawEvent::KeyDown { key: Key::Shift } => {
                        shift = true;
                        return None;
                    }
                    RawEvent::KeyUp { key: Key::Shift } => {
                        shift = false;
                        return None;
                    }
                    RawEvent::KeyDown { key } | RawEvent::KeyUp { key } => *key,
                    _ => return Some(*event),
                };
                let action = self.resolve_in(key, ctrl, shift, Context::Menu)?;
                let key = match action {
                    Action::Back => Key::Escape,
                    Action::Confirm => Key::Enter,
                    Action::MenuUp => Key::Up,
                    Action::MenuDown => Key::Down,
                    Action::MenuLeft => Key::Left,
                    Action::MenuRight => Key::Right,
                    Action::MenuPageUp => Key::PageUp,
                    Action::MenuPageDown => Key::PageDown,
                    Action::MenuHome => Key::Home,
                    Action::MenuEnd => Key::End,
                    Action::DeleteSave => Key::X,
                    _ => return None,
                };
                Some(if matches!(event, RawEvent::KeyDown { .. }) {
                    RawEvent::KeyDown { key }
                } else {
                    RawEvent::KeyUp { key }
                })
            })
            .collect()
    }
    pub fn chord_for(&self, action: Action) -> Option<Chord> {
        self.chord_at(action, 0)
            .or_else(|| self.chord_at(action, 1))
    }
    pub fn chord_at(&self, action: Action, slot: usize) -> Option<Chord> {
        let rows = if slot == 0 {
            &self.bindings
        } else {
            &self.secondary
        };
        rows.iter().find(|b| b.action == action).map(|b| b.chord)
    }
    pub fn label(&self, action: Action) -> String {
        self.chord_for(action)
            .map(Self::chord_label)
            .unwrap_or_else(|| "unbound".into())
    }
    pub fn labels(&self, action: Action) -> String {
        [0, 1]
            .into_iter()
            .filter_map(|slot| self.chord_at(action, slot))
            .map(Self::chord_label)
            .collect::<Vec<_>>()
            .join(" / ")
    }
    #[cfg(test)]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
    pub fn unbind(&mut self, action: Action) {
        self.bindings.retain(|b| b.action != action);
        self.secondary.retain(|b| b.action != action);
    }
    pub fn unbind_slot(&mut self, action: Action, slot: usize) {
        let rows = if slot == 0 {
            &mut self.bindings
        } else {
            &mut self.secondary
        };
        rows.retain(|b| b.action != action);
    }
    pub fn conflict(&self, action: Action, slot: usize, chord: Chord) -> Option<Action> {
        self.rows()
            .find(|b| b.chord == chord && b.action != action && action.overlaps(b.action))
            .map(|b| b.action)
            .or_else(|| (self.chord_at(action, 1 - slot) == Some(chord)).then_some(action))
    }
    pub fn rebind(&mut self, action: Action, chord: Chord) -> bool {
        self.rebind_slot(action, 0, chord)
    }
    pub fn rebind_slot(&mut self, action: Action, slot: usize, chord: Chord) -> bool {
        if slot > 1
            || matches!(chord.key, Key::Ctrl | Key::Shift)
            || self.conflict(action, slot, chord).is_some()
        {
            return false;
        }
        let rows = if slot == 0 {
            &mut self.bindings
        } else {
            &mut self.secondary
        };
        match rows.iter_mut().find(|b| b.action == action) {
            Some(b) => b.chord = chord,
            None => rows.push(Binding { action, chord }),
        }
        true
    }
    #[cfg(test)]
    pub(crate) fn conflicts(&self) -> Vec<(Binding, Binding)> {
        let rows: Vec<_> = self.rows().copied().collect();
        let mut result = Vec::new();
        for (i, a) in rows.iter().enumerate() {
            for b in rows.iter().skip(i + 1) {
                if a.chord == b.chord && a.action.overlaps(b.action) {
                    result.push((*a, *b));
                }
            }
        }
        result
    }
    pub fn valid(&self) -> bool {
        let mut seen = Self {
            bindings: Vec::new(),
            secondary: Vec::new(),
            revision: 3,
        };
        for (slot, rows) in [(0, &self.bindings), (1, &self.secondary)] {
            for b in rows {
                if !b.action.sane()
                    || seen.chord_at(b.action, slot).is_some()
                    || !seen.rebind_slot(b.action, slot, b.chord)
                {
                    return false;
                }
            }
        }
        true
    }
    /// Upgrade old defaults while keeping deliberate remaps and unbound actions.
    pub fn migrate(&mut self, unbound: &[Action]) {
        if self.revision >= 3 {
            return;
        }
        if self.revision == 2 {
            for (mut previous, old_key) in
                [(Self::classic(), Key::O), (Self::left_handed(), Key::Q)]
            {
                previous.rebind(Action::ReturnCargo, Chord::bare(old_key));
                previous.revision = 2;
                if *self == previous && !unbound.contains(&Action::ReturnCargo) {
                    let key = if old_key == Key::O { Key::U } else { Key::E };
                    self.rebind(Action::ReturnCargo, Chord::bare(key));
                    break;
                }
            }
            self.revision = 3;
            return;
        }
        if self.revision == 1 {
            let mut left = Self::left_handed();
            left.unbind(Action::ReturnCargo);
            left.revision = 1;
            let key = if *self == left { Key::E } else { Key::U };
            if !unbound.contains(&Action::ReturnCargo)
                && self.chord_for(Action::ReturnCargo).is_none()
            {
                let _ = self.rebind(Action::ReturnCargo, Chord::bare(key));
            }
            self.revision = 3;
            return;
        }
        let legacy = Self::legacy();
        let left = Self::legacy_left_handed();
        let base = if *self == left { &left } else { &legacy };
        let defaults = if *self == left {
            Self::left_handed()
        } else {
            Self::classic()
        };
        let custom: Vec<_> = self
            .bindings
            .iter()
            .filter(|b| b.action.sane() && base.chord_for(b.action) != Some(b.chord))
            .copied()
            .collect();
        let mut migrated = Self {
            bindings: Vec::new(),
            secondary: Vec::new(),
            revision: 3,
        };
        for b in custom {
            let _ = migrated.rebind(b.action, b.chord);
        }
        for (slot, rows) in [(0, &defaults.bindings), (1, &defaults.secondary)] {
            for b in rows {
                if !unbound.contains(&b.action) && migrated.chord_at(b.action, slot).is_none() {
                    let _ = migrated.rebind_slot(b.action, slot, b.chord);
                }
            }
        }
        *self = migrated;
    }
    pub fn chord_label(chord: Chord) -> String {
        let key = match chord.key {
            Key::Num1 => "1".into(),
            Key::Num2 => "2".into(),
            Key::Num3 => "3".into(),
            Key::Num4 => "4".into(),
            Key::Num5 => "5".into(),
            Key::Num6 => "6".into(),
            Key::Num7 => "7".into(),
            Key::Num8 => "8".into(),
            Key::Num9 => "9".into(),
            other => format!("{other:?}"),
        };
        format!(
            "{}{}{key}",
            if chord.ctrl { "Ctrl+" } else { "" },
            if chord.shift { "Shift+" } else { "" }
        )
    }
}
impl Default for BindingMap {
    fn default() -> Self {
        Self::classic()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionEvent {
    Pressed(Action),
    Released(Action),
}
#[derive(Debug, Default)]
pub struct ActionResolver {
    ctrl: bool,
    shift: bool,
    active: HashMap<Key, Action>,
}
impl ActionResolver {
    pub fn key_edge_in(
        &mut self,
        map: &BindingMap,
        key: Key,
        down: bool,
        context: Context,
    ) -> Option<ActionEvent> {
        self.edge(map, key, down, |a| a.available(context))
    }
    #[cfg(test)]
    pub fn key_edge(&mut self, map: &BindingMap, key: Key, down: bool) -> Option<ActionEvent> {
        self.edge(map, key, down, |_| true)
    }
    fn edge(
        &mut self,
        map: &BindingMap,
        key: Key,
        down: bool,
        accepts: impl Fn(Action) -> bool,
    ) -> Option<ActionEvent> {
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
            if self.active.contains_key(&key) {
                return None;
            }
            let action = map.resolve_where(key, self.ctrl, self.shift, accepts)?;
            self.active.insert(key, action);
            Some(ActionEvent::Pressed(action))
        } else {
            self.active.remove(&key).map(ActionEvent::Released)
        }
    }
    pub fn is_held(&self, action: Action) -> bool {
        self.active.values().any(|&a| a == action)
    }
    pub fn shift_held(&self) -> bool {
        self.shift
    }
    pub fn ctrl_held(&self) -> bool {
        self.ctrl
    }
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
        let map = BindingMap::legacy();
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
        let map = BindingMap::legacy_left_handed();
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
        let mut map = BindingMap::legacy();
        assert!(
            !map.rebind(Action::StopOrScrap, Chord::bare(Key::M)),
            "M belongs to Run in the unit context"
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
        let map = BindingMap::legacy();
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
        let map = BindingMap::legacy();
        assert_eq!(
            map.resolve(Key::Num4, true, true),
            Some(Action::AssignGroup(4))
        );
    }

    #[test]
    fn releases_pair_with_what_the_press_meant() {
        let map = BindingMap::legacy();
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
        let map = BindingMap::legacy();
        let mut r = ActionResolver::default();
        r.key_edge(&map, Key::Left, true);
        assert!(r.is_held(Action::PanLeft));
        r.key_edge(&map, Key::Left, false);
        assert!(!r.is_held(Action::PanLeft));
    }

    #[test]
    fn the_classic_profile_has_no_conflicts_and_covers_the_old_map() {
        let map = BindingMap::legacy();
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
        let map = BindingMap::legacy();
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
    #[test]
    fn contexts_allow_panel_reuse_but_reserve_camera_and_group_keys() {
        let mut map = BindingMap::classic();
        assert!(map.valid());
        assert!(BindingMap::left_handed().valid());
        assert_eq!(
            map.resolve_in(Key::R, false, false, Context::Units),
            Some(Action::Patrol)
        );
        assert_eq!(
            map.resolve_in(Key::R, false, false, Context::Production),
            Some(Action::TrainSlot(2))
        );
        assert_eq!(
            map.resolve_in(Key::R, false, false, Context::Construction),
            Some(Action::BuildCategory(2))
        );
        assert!(!map.rebind(Action::Build(BuildingKind::Turret), Chord::bare(Key::W)));
        assert!(!map.rebind(Action::TrainSlot(0), Chord::bare(Key::Num1)));
        assert!(map.rebind(Action::Build(BuildingKind::Turret), Chord::ctrl(Key::G)));
        assert_eq!(
            map.resolve_in(Key::G, true, true, Context::BuildCategory(2)),
            Some(Action::Build(BuildingKind::Turret))
        );
        assert_eq!(
            map.resolve_in(Key::G, true, true, Context::BuildCategory(0)),
            None
        );
    }

    #[test]
    fn releasing_one_pan_alias_or_changing_context_does_not_release_the_other() {
        let map = BindingMap::classic();
        let mut resolver = ActionResolver::default();
        resolver.key_edge_in(&map, Key::W, true, Context::Units);
        resolver.key_edge_in(&map, Key::Up, true, Context::Units);
        resolver.key_edge_in(&map, Key::Shift, true, Context::Construction);
        resolver.key_edge_in(&map, Key::W, false, Context::Construction);
        assert!(resolver.is_held(Action::PanUp));
        resolver.key_edge_in(&map, Key::Up, false, Context::Construction);
        assert!(!resolver.is_held(Action::PanUp));
        resolver.key_edge_in(&map, Key::W, true, Context::Playback);
        resolver.clear();
        assert!(!resolver.is_held(Action::PanUp));
    }

    #[test]
    fn old_defaults_migrate_without_stealing_custom_chords_or_reviving_unbindings() {
        let mut map = BindingMap::legacy();
        map.migrate(&[]);
        assert_eq!(map, BindingMap::classic());
        let mut map = BindingMap::legacy();
        assert!(map.rebind(Action::Patrol, Chord::bare(Key::D)));
        map.unbind(Action::Salvage);
        map.migrate(&[Action::Salvage]);
        assert_eq!(map.chord_for(Action::Patrol), Some(Chord::bare(Key::D)));
        assert_eq!(map.chord_at(Action::PanRight, 0), None);
        assert_eq!(
            map.chord_at(Action::PanRight, 1),
            Some(Chord::bare(Key::Right))
        );
        assert_eq!(map.chord_for(Action::Salvage), None);
        assert!(map.valid());
        let saved = serde_json::to_string(&map).unwrap();
        let mut loaded: BindingMap = serde_json::from_str(&saved).unwrap();
        loaded.migrate(&[]);
        assert_eq!(loaded, map);
    }

    #[test]
    fn malformed_secondary_rows_and_unknown_slots_are_rejected() {
        let mut map = BindingMap::classic();
        map.secondary.push(Binding {
            action: Action::Patrol,
            chord: Chord::bare(Key::W),
        });
        assert!(!map.valid());
        let mut map = BindingMap::classic();
        map.bindings.push(Binding {
            action: Action::BuildCategory(9),
            chord: Chord::bare(Key::G),
        });
        assert!(!map.valid());
    }

    #[test]
    fn rebound_menu_keys_emit_only_their_contextual_navigation() {
        use oxide_protocol::RawEvent;
        let mut map = BindingMap::classic();
        assert!(map.rebind(Action::Confirm, Chord::bare(Key::Q)));
        let events = map.menu_events(
            &[
                RawEvent::KeyDown { key: Key::Q },
                RawEvent::KeyDown { key: Key::W },
                RawEvent::Text { ch: 'q' },
            ],
            false,
            false,
        );
        assert_eq!(
            events,
            vec![
                RawEvent::KeyDown { key: Key::Enter },
                RawEvent::Text { ch: 'q' }
            ]
        );
    }
}

#[cfg(test)]
mod cargo_binding_tests {
    use super::*;

    #[test]
    fn cargo_shortcuts_share_keys_only_in_distinct_selection_contexts() {
        for (mut map, key) in [
            (BindingMap::classic(), Key::U),
            (BindingMap::left_handed(), Key::E),
        ] {
            for (context, action) in [
                (Context::Workers, Action::ReturnCargo),
                (Context::Construction, Action::ReturnCargo),
                (Context::Units, Action::Unload),
                (Context::Buildings, Action::Upgrade),
                (Context::Production, Action::Upgrade),
            ] {
                assert_eq!(map.resolve_in(key, false, false, context), Some(action));
            }
            assert!(map.valid());
            assert!(map.conflicts().is_empty());
            assert!(!map.rebind(Action::Run, Chord::bare(key)));
            assert!(map.rebind(Action::ReturnCargo, Chord::ctrl(Key::O)));
            assert_eq!(map.chord_for(Action::Unload), Some(Chord::bare(key)));
        }
    }

    #[test]
    fn shared_cargo_key_migrates_previous_presets_but_keeps_custom_bindings() {
        for (mut map, old_key, new_key) in [
            (BindingMap::classic(), Key::O, Key::U),
            (BindingMap::left_handed(), Key::Q, Key::E),
        ] {
            assert!(map.rebind(Action::ReturnCargo, Chord::bare(old_key)));
            map.revision = 2;
            let mut custom = map.clone();
            assert!(custom.rebind(Action::ReturnCargo, Chord::ctrl(Key::O)));
            custom.migrate(&[]);
            assert_eq!(
                custom.chord_for(Action::ReturnCargo),
                Some(Chord::ctrl(Key::O))
            );
            let mut unbound = map.clone();
            unbound.unbind(Action::ReturnCargo);
            unbound.migrate(&[Action::ReturnCargo]);
            assert_eq!(unbound.chord_for(Action::ReturnCargo), None);
            map.migrate(&[]);
            assert_eq!(
                map.chord_for(Action::ReturnCargo),
                Some(Chord::bare(new_key))
            );
            assert!(map.valid());
        }
    }

    #[test]
    fn return_cargo_migration_keeps_custom_keys_and_unbindings() {
        for mut map in [BindingMap::classic(), BindingMap::left_handed()] {
            let chord = map.chord_for(Action::ReturnCargo).unwrap();
            map.unbind(Action::ReturnCargo);
            map.revision = 1;
            let mut unbound = map.clone();
            unbound.migrate(&[Action::ReturnCargo]);
            assert_eq!(unbound.chord_for(Action::ReturnCargo), None);
            map.migrate(&[]);
            assert_eq!(map.chord_for(Action::ReturnCargo), Some(chord));
            assert!(map.valid());
        }
        let mut map = BindingMap::classic();
        map.unbind(Action::ReturnCargo);
        map.revision = 1;
        map.unbind(Action::Unload);
        map.unbind(Action::Upgrade);
        assert!(map.rebind(Action::Run, Chord::bare(Key::U)));
        map.migrate(&[]);
        assert_eq!(map.chord_for(Action::Run), Some(Chord::bare(Key::U)));
        assert_eq!(map.chord_for(Action::ReturnCargo), None);
    }
}
