//! Settings and the Controls remap screen — one screen object with two
//! faces. Windowless update: config edits and binding capture happen
//! here (pure state), while persistence and drawing stay with the
//! caller, which is what lets the 0.9 modifier-capture regression
//! finally live under a headless test.

use crate::action::{Action, BindingMap, Chord};
use crate::config::Config;
use crate::game::SoundKind;
use crate::menu::Menu;
use crate::render;
use macroquad::prelude::{Color, Vec2, color_u8, draw_text, measure_text};
use oxide_protocol::{Key, RawEvent};

/// Which face is up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    /// Volume, scale, camera, motion rows.
    Settings,
    /// Key remapping; `rebinding` is the armed row awaiting its chord.
    Controls {
        /// The action row armed for rebinding, if any.
        rebinding: Option<usize>,
    },
}

/// Where the screen was opened from — leaving returns there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// The front door.
    Home,
    /// The pause menu of a live match; its payload waits intact so the
    /// cursor comes back to the row that opened this screen.
    Pause,
}

/// What a settings frame decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Still tuning.
    Stay,
    /// Back to wherever the screen was opened from (its [`Origin`]).
    Leave,
}

/// A screen-owned status line, drawn by [`SettingsScreen::draw`] above
/// the caller's veil — a toast routed through the game HUD dies under
/// it. Persists until the next action, so it needs no wall clock and
/// the screen stays headless-testable.
pub struct Notice {
    /// The message.
    pub text: String,
    /// A complaint (danger red) rather than a confirmation.
    pub danger: bool,
}

/// A frame's full result: the transition and whether the config
/// changed (the caller persists — screens never touch the disk, which
/// keeps their tests hermetic).
pub struct Update {
    /// Where to go.
    pub out: Out,
    /// The config changed; persist it.
    pub dirty: bool,
}

/// The remappable actions, in display order. Digits and structural keys
/// (Back, Confirm, group slots) stay fixed — their meaning is
/// positional, not preferential.
const REMAPPABLE: [(Action, &str); 24] = [
    (Action::StopOrScrap, "Stop / scrap site"),
    (Action::TrainSlot(0), "Train slot 1"),
    (Action::TrainSlot(1), "Train slot 2"),
    (Action::TogglePause, "Pause"),
    (Action::ToggleBuildPalette, "Build palette"),
    (Action::Patrol, "Patrol"),
    (Action::HomeCamera, "Center home"),
    (Action::ToggleOverlay, "Debug overlay"),
    (Action::PanLeft, "Pan left"),
    (Action::PanRight, "Pan right"),
    (Action::PanUp, "Pan up"),
    (Action::PanDown, "Pan down"),
    (Action::CycleIdleWorker, "Next idle harvester"),
    (Action::JumpToLastAlert, "Jump to last alert"),
    (Action::Salvage, "Salvage building"),
    (Action::Run, "Run (move, no engaging)"),
    (Action::SetBookmark(0), "Set bookmark 1"),
    (Action::RecallBookmark(0), "Recall bookmark 1"),
    (Action::SetBookmark(1), "Set bookmark 2"),
    (Action::RecallBookmark(1), "Recall bookmark 2"),
    (Action::SetBookmark(2), "Set bookmark 3"),
    (Action::RecallBookmark(2), "Recall bookmark 3"),
    (Action::SetBookmark(3), "Set bookmark 4"),
    (Action::RecallBookmark(3), "Recall bookmark 4"),
];

fn settings_menu(config: &Config) -> Menu {
    let pct = |v: f32| format!("{}%", (v * 100.0).round());
    let onoff = |v: bool| if v { "on" } else { "off" };
    Menu::new(
        "SETTINGS",
        vec![
            format!("Master volume: {}", pct(config.volumes.master)),
            format!("Effects volume: {}", pct(config.volumes.effects)),
            format!("UI volume: {}", pct(config.volumes.ui)),
            format!("UI scale: {}", pct(config.ui_scale)),
            format!("Edge pan: {}", onoff(config.camera.edge_pan)),
            format!("Invert zoom: {}", onoff(config.camera.zoom_inverted)),
            format!("Reduced motion: {}", onoff(config.reduced_motion)),
            format!("Colorblind accents: {}", onoff(config.colorblind)),
            "Apply left-handed bindings".to_string(),
            "Controls...".to_string(),
            "Back".to_string(),
        ],
    )
}

/// The "Controls..." row's index in [`settings_menu`] — the exit paths
/// from the Controls face re-select it, and a stale literal here once
/// left the cursor on Colorblind accents after two rows were inserted
/// above (a test pins the label to this index).
const CONTROLS_ROW: usize = 9;

/// Advances one settings row to its next value step. Returns false on
/// rows that navigate instead of cycling.
fn cycle_setting(config: &mut Config, row: usize) -> bool {
    let step = |v: f32| {
        // 0 -> 25 -> 50 -> 75 -> 100 -> 0, tolerant of odd stored values.
        let next = ((v * 4.0).round() as u32 + 1) % 5;
        next as f32 / 4.0
    };
    match row {
        0 => config.volumes.master = step(config.volumes.master),
        1 => config.volumes.effects = step(config.volumes.effects),
        2 => config.volumes.ui = step(config.volumes.ui),
        3 => {
            // 75 -> 100 -> 125 -> 150 -> 75.
            config.ui_scale = match (config.ui_scale * 100.0).round() as u32 {
                75 => 1.0,
                100 => 1.25,
                125 => 1.5,
                _ => 0.75,
            };
            render::set_user_scale(config.ui_scale);
        }
        4 => config.camera.edge_pan = !config.camera.edge_pan,
        5 => config.camera.zoom_inverted = !config.camera.zoom_inverted,
        6 => {
            config.reduced_motion = !config.reduced_motion;
            render::set_reduced_motion(config.reduced_motion);
        }
        7 => {
            config.colorblind = !config.colorblind;
            render::set_colorblind(config.colorblind);
        }
        _ => return false, // preset, Controls..., and Back route in update
    }
    true
}

fn controls_menu(config: &Config) -> Menu {
    let mut items: Vec<String> = REMAPPABLE
        .iter()
        .map(|(action, label)| {
            let chord = config
                .bindings
                .chord_for(*action)
                .map(BindingMap::chord_label)
                .unwrap_or_else(|| "unbound".to_string());
            format!("{label}: {chord}")
        })
        .collect();
    items.push("Reset to defaults".to_string());
    items.push("Back".to_string());
    Menu::new("CONTROLS", items)
}

/// The settings screen (both faces).
pub struct SettingsScreen {
    /// Which face is up.
    pub face: Face,
    /// The face's live menu.
    pub menu: Menu,
    /// Where leaving returns to.
    pub origin: Origin,
    /// The screen's status line, if one is up.
    pub notice: Option<Notice>,
}

impl SettingsScreen {
    /// Opens on the settings rows, returning Home on leave.
    pub fn open(config: &Config) -> Self {
        Self::open_from(config, Origin::Home)
    }

    /// Opens on the settings rows with an explicit return target.
    pub fn open_from(config: &Config, origin: Origin) -> Self {
        Self {
            face: Face::Settings,
            menu: settings_menu(config),
            origin,
            notice: None,
        }
    }

    /// The debug protocol's stable mode name for the current face.
    pub fn mode_name(&self) -> &'static str {
        match self.face {
            Face::Settings => "settings",
            Face::Controls { .. } => "controls",
        }
    }

    /// The face's coaching line.
    pub fn hint(&self) -> &'static str {
        match self.face {
            Face::Settings => "Enter cycles a value - changes stick immediately",
            Face::Controls { rebinding: Some(_) } => {
                "press the new chord (modifiers held count) - Escape cancels"
            }
            Face::Controls { rebinding: None } => {
                "Enter arms a row, then press its new chord - X unbinds"
            }
        }
    }

    fn goto_controls(&mut self, config: &Config, select: usize) {
        self.face = Face::Controls { rebinding: None };
        self.menu = controls_menu(config);
        self.menu.select(select);
        self.notice = None;
    }

    /// Draws the face's menu and the screen-owned notice — the caller
    /// draws the veil first, so both land above it.
    pub fn draw(&self) {
        self.menu.draw(self.hint());
        if let Some(notice) = &self.notice {
            const NOTICE_DANGER: Color = color_u8!(217, 82, 74, 255);
            const NOTICE_PLAIN: Color = color_u8!(232, 228, 216, 200);
            let s = render::ui_scale();
            let size = 16.0 * s;
            let width = measure_text(&notice.text, None, size as u16, 1.0).width;
            draw_text(
                &notice.text,
                (render::viewport().x - width) * 0.5,
                render::viewport().y - 48.0 * s,
                size,
                if notice.danger {
                    NOTICE_DANGER
                } else {
                    NOTICE_PLAIN
                },
            );
        }
    }

    /// Applies a frame's events. `live` is the in-play binding map,
    /// kept in lockstep with the config's; `ctrl0`/`shift0` are the
    /// frame-start modifier truth a chord capture replays from.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
        config: &mut Config,
        live: &mut BindingMap,
        ctrl0: bool,
        shift0: bool,
    ) -> Update {
        let mut update = Update {
            out: Out::Stay,
            dirty: false,
        };
        let escaped = events
            .iter()
            .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
        match self.face {
            Face::Settings => {
                if escaped {
                    update.out = Out::Leave;
                } else if let Some(row) = self.menu.handle(events, mouse) {
                    sounds.push((SoundKind::Click, None));
                    // Any activation is "the next action": the standing
                    // notice has had its say.
                    self.notice = None;
                    if cycle_setting(config, row) {
                        // Apply live, persist, keep the cursor on the
                        // row being tuned.
                        update.dirty = true;
                        let selected = self.menu.selected;
                        self.menu = settings_menu(config);
                        self.menu.select(selected);
                    } else if row == 8 {
                        // The left-handed preset replaces the whole
                        // profile (custom rebinds included — Controls'
                        // Reset row walks back to Classic).
                        config.bindings = BindingMap::left_handed();
                        update.dirty = true;
                        *live = config.bindings.clone();
                        self.notice = Some(Notice {
                            text: "left-handed profile applied".to_string(),
                            danger: false,
                        });
                        let selected = self.menu.selected;
                        self.menu = settings_menu(config);
                        self.menu.select(selected);
                    } else if row == 9 {
                        self.goto_controls(config, 0);
                    } else {
                        update.out = Out::Leave;
                    }
                }
            }
            Face::Controls {
                rebinding: Some(row),
            } => {
                // Armed: the next key IS the answer — raw, before any
                // binding resolution, or the old meaning would fire.
                // The frame's edges replay in order from the frame-
                // start baseline, and the modifiers are read AT the
                // main key's press: batch-final flags would miss a
                // chord whose Ctrl came back up later the same frame.
                let mut walk_ctrl = ctrl0;
                let mut walk_shift = shift0;
                let mut pressed: Option<(Key, bool, bool)> = None;
                for e in events {
                    match e {
                        RawEvent::KeyDown { key: Key::Ctrl } => walk_ctrl = true,
                        RawEvent::KeyUp { key: Key::Ctrl } => walk_ctrl = false,
                        RawEvent::KeyDown { key: Key::Shift } => walk_shift = true,
                        RawEvent::KeyUp { key: Key::Shift } => walk_shift = false,
                        RawEvent::KeyDown { key } if pressed.is_none() => {
                            pressed = Some((*key, walk_ctrl, walk_shift));
                        }
                        _ => {}
                    }
                }
                match pressed {
                    Some((Key::Escape, _, _)) => {
                        self.face = Face::Controls { rebinding: None };
                        self.notice = None;
                    }
                    Some((key, ctrl, shift)) => {
                        let (target, _) = REMAPPABLE[row];
                        let chord = Chord { key, ctrl, shift };
                        if config.bindings.rebind(target, chord) {
                            // Bound again: the unbind tombstone lifts.
                            config.unbound.retain(|a| *a != target);
                            update.dirty = true;
                            *live = config.bindings.clone();
                            self.goto_controls(config, row);
                        } else {
                            // Refused: name the holder, so the player
                            // knows which row to unbind first.
                            let text = match config.bindings.holder(chord).filter(|&a| a != target)
                            {
                                Some(holder) => format!(
                                    "{} is already bound to {}",
                                    BindingMap::chord_label(chord),
                                    holder.label()
                                ),
                                None => "that key already means something".to_string(),
                            };
                            self.notice = Some(Notice { text, danger: true });
                            sounds.push((SoundKind::Denied, None));
                            self.face = Face::Controls { rebinding: None };
                        }
                    }
                    None => {}
                }
            }
            Face::Controls { rebinding: None } => {
                let x_pressed = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::X }));
                if escaped {
                    self.face = Face::Settings;
                    self.menu = settings_menu(config);
                    self.menu.select(CONTROLS_ROW);
                    self.notice = None;
                } else if x_pressed && self.menu.selected < REMAPPABLE.len() {
                    // X on a row unbinds it — outside capture mode, so
                    // the key is free to mean this. The tombstone
                    // records the CHOICE: without it, the next load's
                    // new-verb migration would read the missing row as
                    // an old config and restore the classic chord.
                    let (target, _) = REMAPPABLE[self.menu.selected];
                    config.bindings.unbind(target);
                    if !config.unbound.contains(&target) {
                        config.unbound.push(target);
                    }
                    update.dirty = true;
                    *live = config.bindings.clone();
                    let row = self.menu.selected;
                    self.goto_controls(config, row);
                } else if let Some(row) = self.menu.handle(events, mouse) {
                    sounds.push((SoundKind::Click, None));
                    self.notice = None;
                    if row < REMAPPABLE.len() {
                        self.face = Face::Controls {
                            rebinding: Some(row),
                        };
                    } else if row == REMAPPABLE.len() {
                        // Reset to defaults — tombstones included.
                        config.bindings = BindingMap::classic();
                        config.unbound.clear();
                        update.dirty = true;
                        *live = config.bindings.clone();
                        self.goto_controls(config, row);
                    } else {
                        self.face = Face::Settings;
                        self.menu = settings_menu(config);
                        self.menu.select(CONTROLS_ROW);
                    }
                }
            }
        }
        update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn drive(
        s: &mut SettingsScreen,
        config: &mut Config,
        live: &mut BindingMap,
        events: &[RawEvent],
        ctrl0: bool,
    ) -> Update {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        s.update(events, &mut mouse, &mut sounds, config, live, ctrl0, false)
    }

    fn press(key: Key) -> Vec<RawEvent> {
        vec![RawEvent::KeyDown { key }, RawEvent::KeyUp { key }]
    }

    #[test]
    fn cycling_a_row_edits_the_config_and_reports_dirty() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        for _ in 0..6 {
            drive(&mut s, &mut config, &mut live, &press(Key::Down), false);
        }
        let up = drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        assert!(config.reduced_motion, "row six toggles reduced motion");
        assert!(up.dirty, "the caller is told to persist");
        assert_eq!(s.menu.selected, 6, "the cursor stays on the tuned row");
    }

    #[test]
    fn a_held_modifier_from_another_screen_rides_into_the_captured_chord() {
        // The 0.9 regression, headless at last: Ctrl went down on the
        // Settings screen, so no Ctrl edge appears in THIS frame's
        // events — only the baseline knows. The capture must still
        // record Ctrl+K.
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(&config, 5); // Patrol row
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), true);
        assert_eq!(s.face, Face::Controls { rebinding: Some(5) });
        let up = drive(&mut s, &mut config, &mut live, &press(Key::K), true);
        assert!(up.dirty);
        assert_eq!(
            config.bindings.chord_for(Action::Patrol),
            Some(Chord {
                key: Key::K,
                ctrl: true,
                shift: false
            }),
            "the held Ctrl rides into the chord"
        );
        assert_eq!(
            live.chord_for(Action::Patrol),
            config.bindings.chord_for(Action::Patrol),
            "the live map follows the config"
        );
    }

    #[test]
    fn a_chord_released_within_the_frame_still_reads_its_modifiers() {
        // The whole chord in one batch: Ctrl down, K down, K up, Ctrl
        // up. Batch-final modifier state is false — only reading the
        // modifiers AT the key's press gets this right.
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(&config, 5);
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        let batch = vec![
            RawEvent::KeyDown { key: Key::Ctrl },
            RawEvent::KeyDown { key: Key::K },
            RawEvent::KeyUp { key: Key::K },
            RawEvent::KeyUp { key: Key::Ctrl },
        ];
        drive(&mut s, &mut config, &mut live, &batch, false);
        assert_eq!(
            config.bindings.chord_for(Action::Patrol),
            Some(Chord {
                key: Key::K,
                ctrl: true,
                shift: false
            })
        );
    }

    #[test]
    fn a_conflicting_chord_is_refused_and_the_notice_names_the_holder() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let before = config.bindings.chord_for(Action::Patrol);
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(&config, 5);
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        // M already means Run.
        let up = drive(&mut s, &mut config, &mut live, &press(Key::M), false);
        assert!(!up.dirty);
        let notice = s.notice.as_ref().expect("the refusal reports");
        assert_eq!(notice.text, "M is already bound to Run");
        assert!(notice.danger);
        assert_eq!(config.bindings.chord_for(Action::Patrol), before);
        // Navigation is not an action: the notice waits to be read.
        drive(&mut s, &mut config, &mut live, &press(Key::Down), false);
        assert!(s.notice.is_some(), "arrow keys must not eat the notice");
        // The next action clears it.
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        assert!(s.notice.is_none(), "arming a row is the next action");
    }

    #[test]
    fn a_conflict_with_a_non_remappable_holder_is_still_named() {
        // Enter is Confirm — not on the remap screen, but a reachable
        // collision that once could only say "something".
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(&config, 5);
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        drive(&mut s, &mut config, &mut live, &press(Key::Num1), false);
        assert_eq!(
            s.notice.as_ref().map(|n| n.text.as_str()),
            Some("1 is already bound to Slot 1")
        );
    }

    #[test]
    fn leaving_the_controls_face_clears_the_notice() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(&config, 5);
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        drive(&mut s, &mut config, &mut live, &press(Key::M), false);
        assert!(s.notice.is_some());
        drive(&mut s, &mut config, &mut live, &press(Key::Escape), false);
        assert!(
            matches!(s.face, Face::Settings) && s.notice.is_none(),
            "a face change retires the notice with its context"
        );
    }

    #[test]
    fn the_left_handed_preset_moves_the_verbs_and_reset_walks_home() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        for _ in 0..8 {
            drive(&mut s, &mut config, &mut live, &press(Key::Down), false);
        }
        let up = drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        assert!(up.dirty);
        assert!(
            s.notice.as_ref().is_some_and(|n| !n.danger),
            "the applied preset confirms on the screen's own notice line"
        );
        assert_eq!(
            config.bindings.chord_for(Action::TrainSlot(0)),
            Some(Chord::bare(Key::K)),
            "training moved to the right hand"
        );
        assert_eq!(
            config.bindings.chord_for(Action::PanLeft),
            Some(Chord::bare(Key::Left)),
            "pans stay on the arrows"
        );
        assert!(
            config.bindings.conflicts().is_empty(),
            "the preset must be conflict-free"
        );
        assert_eq!(live.chord_for(Action::Patrol), Some(Chord::bare(Key::O)));
    }

    #[test]
    fn x_unbinds_and_reset_restores_the_classic_map() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(&config, 5);
        let up = drive(&mut s, &mut config, &mut live, &press(Key::X), false);
        assert!(up.dirty);
        assert_eq!(config.bindings.chord_for(Action::Patrol), None);
        // Reset row restores everything.
        s.menu.select(REMAPPABLE.len());
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        assert_eq!(
            config.bindings.chord_for(Action::Patrol),
            BindingMap::classic().chord_for(Action::Patrol)
        );
    }

    #[test]
    fn escaping_controls_returns_the_cursor_to_the_controls_row() {
        // Two rows were once inserted above Controls... while the exit
        // paths kept a stale index: coming back from Controls left the
        // cursor on Colorblind accents, and the next Enter toggled a
        // setting instead of reopening the remap screen.
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut screen = SettingsScreen::open(&config);
        assert_eq!(
            settings_menu(&config).items[CONTROLS_ROW],
            "Controls...",
            "the derived index names the row it claims"
        );
        screen.menu.select(CONTROLS_ROW);
        drive(
            &mut screen,
            &mut config,
            &mut live,
            &press(Key::Enter),
            false,
        );
        assert!(matches!(screen.face, Face::Controls { .. }));
        drive(
            &mut screen,
            &mut config,
            &mut live,
            &press(Key::Escape),
            false,
        );
        assert!(matches!(screen.face, Face::Settings));
        assert_eq!(
            screen.menu.selected, CONTROLS_ROW,
            "the cursor comes back to the row that was activated"
        );
    }
}
