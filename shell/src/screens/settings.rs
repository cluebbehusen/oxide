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
use macroquad::prelude::{Vec2, draw_text, measure_text};
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

/// What a settings frame decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Still tuning.
    Stay,
    /// Reveal local diagnostic records in the file manager.
    OpenDiagnostics,
    /// Export a consistent local report in the background.
    ExportDiagnostics,
    /// Back to wherever the screen was opened from — the coordinator
    /// holds the displaced screen and restores it wholesale.
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

/// Every keyboard action is exposed, including contextual cards and navigation.
fn control_sections() -> Vec<(&'static str, Vec<Action>)> {
    use Action::*;
    let mut sections = vec![
        (
            "Camera",
            vec![
                PanUp,
                PanLeft,
                PanDown,
                PanRight,
                HomeCamera,
                CycleIdleWorker,
                JumpToLastAlert,
            ],
        ),
        (
            "Orders",
            vec![
                StopOrScrap,
                Run,
                AttackMove,
                Patrol,
                Salvage,
                RepairUnit,
                ReturnCargo,
                Unload,
            ],
        ),
        (
            "Buildings",
            vec![ToggleBuildPalette, Upgrade, SetRally, ClearRally],
        ),
        ("Production", (0..6).map(TrainSlot).collect()),
        (
            "Construction categories",
            (0..4).map(BuildCategory).collect(),
        ),
    ];
    for (name, kinds) in crate::action::BUILD_CATEGORIES {
        sections.push((name, kinds.iter().copied().map(Build).collect()));
    }
    sections.extend([
        (
            "Control groups",
            (1..=5).flat_map(|n| [Slot(n), AssignGroup(n)]).collect(),
        ),
        (
            "Camera bookmarks",
            (0..4)
                .flat_map(|n| [RecallBookmark(n), SetBookmark(n)])
                .collect(),
        ),
        ("Match", vec![TogglePause, ToggleOverlay]),
        (
            "Replay",
            vec![
                ReplayPause,
                ReplayBack,
                ReplayForward,
                ReplayStart,
                ReplayEnd,
                ReplayStats,
            ],
        ),
        ("Replay speeds", (0..8).map(ReplaySpeed).collect()),
        (
            "Menus",
            vec![
                Back,
                Confirm,
                MenuUp,
                MenuDown,
                MenuLeft,
                MenuRight,
                MenuPageUp,
                MenuPageDown,
                MenuHome,
                MenuEnd,
                DeleteSave,
            ],
        ),
    ]);
    sections
}
fn control_rows() -> Vec<Option<Action>> {
    control_sections()
        .into_iter()
        .flat_map(|(_, actions)| std::iter::once(None).chain(actions.into_iter().map(Some)))
        .collect()
}

/// One settings row. Rows are values, not indices: the label, the cycle
/// step, and the activation route all key off the row itself, so inserting a
/// row cannot leave a stale index pointing at its neighbour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    MasterVolume,
    EffectsVolume,
    UiVolume,
    MusicVolume,
    UiScale,
    EdgePan,
    InvertZoom,
    ReducedMotion,
    Colorblind,
    PerformanceDisplay,
    MarkerTiming,
    MarkerSize,
    LeftHandedPreset,
    Controls,
    Diagnostics,
    OpenDiagnostics,
    ExportDiagnostics,
}

impl Row {
    /// Every row, in the order the menu shows them.
    const ALL: [Row; 17] = [
        Row::MasterVolume,
        Row::EffectsVolume,
        Row::UiVolume,
        Row::MusicVolume,
        Row::UiScale,
        Row::EdgePan,
        Row::InvertZoom,
        Row::ReducedMotion,
        Row::Colorblind,
        Row::PerformanceDisplay,
        Row::MarkerTiming,
        Row::MarkerSize,
        Row::LeftHandedPreset,
        Row::Controls,
        Row::Diagnostics,
        Row::OpenDiagnostics,
        Row::ExportDiagnostics,
    ];

    /// Where this row sits in this build's menu.
    fn index(self) -> usize {
        rows(crate::platform::TOUCH_ONLY)
            .iter()
            .position(|row| *row == self)
            .expect("the row is offered on this build")
    }

    /// Rows a touch-only build cannot use: key rebinding needs a
    /// keyboard, edge pan needs a hovering pointer, and there is no file
    /// manager to open a folder in.
    fn needs_desktop(self) -> bool {
        matches!(
            self,
            Row::EdgePan | Row::LeftHandedPreset | Row::Controls | Row::OpenDiagnostics
        )
    }

    fn label(self, config: &Config) -> String {
        let pct = |v: f32| format!("{}%", (v * 100.0).round());
        let onoff = |v: bool| if v { "on" } else { "off" };
        match self {
            Row::MasterVolume => format!("Master volume: {}", pct(config.volumes.master)),
            Row::EffectsVolume => format!("Effects volume: {}", pct(config.volumes.effects)),
            Row::UiVolume => format!("UI volume: {}", pct(config.volumes.ui)),
            Row::MusicVolume => format!("Music volume: {}", pct(config.volumes.music)),
            Row::UiScale => format!("UI scale: {}", pct(config.ui_scale)),
            Row::EdgePan => format!("Edge pan: {}", onoff(config.camera.edge_pan)),
            Row::InvertZoom => format!("Invert zoom: {}", onoff(config.camera.zoom_inverted)),
            Row::ReducedMotion => format!("Reduced motion: {}", onoff(config.reduced_motion)),
            Row::Colorblind => format!("Colorblind accents: {}", onoff(config.colorblind)),
            Row::PerformanceDisplay => format!(
                "Performance display: {}",
                config.performance_display.label()
            ),
            Row::MarkerTiming => {
                format!("Show strategic markers: {}", config.markers.timing_label())
            }
            Row::MarkerSize => format!("Strategic marker size: {}", pct(config.markers.scale)),
            Row::LeftHandedPreset => "Apply left-handed bindings".to_string(),
            Row::Controls => "Controls...".to_string(),
            Row::Diagnostics => format!("Diagnostics: {}", onoff(config.diagnostics)),
            Row::OpenDiagnostics => "Open diagnostics folder".to_string(),
            Row::ExportDiagnostics => "Export diagnostic report".to_string(),
        }
    }
}

/// The settings face's coaching line.
fn settings_hint(touch_only: bool) -> &'static str {
    if touch_only {
        "tap a row to change it - changes stick immediately"
    } else {
        "{confirm} cycles a value - changes stick immediately"
    }
}

/// The rows this build offers, in menu order.
fn rows(touch_only: bool) -> Vec<Row> {
    Row::ALL
        .into_iter()
        .filter(|row| !(touch_only && row.needs_desktop()))
        .collect()
}

fn settings_menu(config: &Config) -> Menu {
    Menu::new(
        "SETTINGS",
        rows(crate::platform::TOUCH_ONLY)
            .iter()
            .map(|row| row.label(config))
            .collect(),
    )
}

/// Advances one settings row to its next value step. Returns false on
/// rows that navigate instead of cycling.
fn cycle_setting(config: &mut Config, row: Row) -> bool {
    let step = |v: f32| {
        // 0 -> 25 -> 50 -> 75 -> 100 -> 0, tolerant of odd stored values.
        let next = ((v * 4.0).round() as u32 + 1) % 5;
        next as f32 / 4.0
    };
    // 75 -> 100 -> 125 -> 150 -> 75.
    let scale_step = |v: f32| match (v * 100.0).round() as u32 {
        75 => 1.0,
        100 => 1.25,
        125 => 1.5,
        _ => 0.75,
    };
    match row {
        Row::MasterVolume => config.volumes.master = step(config.volumes.master),
        Row::EffectsVolume => config.volumes.effects = step(config.volumes.effects),
        Row::UiVolume => config.volumes.ui = step(config.volumes.ui),
        Row::MusicVolume => config.volumes.music = step(config.volumes.music),
        Row::UiScale => {
            config.ui_scale = scale_step(config.ui_scale);
            render::set_user_scale(config.ui_scale);
        }
        Row::EdgePan => config.camera.edge_pan = !config.camera.edge_pan,
        Row::InvertZoom => config.camera.zoom_inverted = !config.camera.zoom_inverted,
        Row::ReducedMotion => {
            config.reduced_motion = !config.reduced_motion;
            render::set_reduced_motion(config.reduced_motion);
        }
        Row::Colorblind => {
            config.colorblind = !config.colorblind;
            render::set_colorblind(config.colorblind);
        }
        Row::PerformanceDisplay => {
            config.performance_display = config.performance_display.next();
        }
        Row::Diagnostics => config.diagnostics = !config.diagnostics,
        Row::MarkerTiming => {
            config.markers.cycle_timing();
            crate::strategic_markers::set_prefs(config.markers);
        }
        Row::MarkerSize => {
            config.markers.scale = scale_step(config.markers.scale);
            crate::strategic_markers::set_prefs(config.markers);
        }
        Row::LeftHandedPreset | Row::Controls | Row::OpenDiagnostics | Row::ExportDiagnostics => {
            return false;
        }
    }
    true
}

fn controls_menu(config: &Config, selected_slot: usize) -> Menu {
    let mut items = Vec::new();
    let mut headers = Vec::new();
    for (section, actions) in control_sections() {
        headers.push(items.len());
        items.push(section.to_uppercase());
        for action in actions {
            let label = |slot| {
                let value = config
                    .bindings
                    .chord_at(action, slot)
                    .map(BindingMap::chord_label)
                    .unwrap_or_else(|| "unbound".into());
                if slot == selected_slot {
                    format!("[{value}]")
                } else {
                    value
                }
            };
            items.push(format!("{}: {} | {}", action.label(), label(0), label(1)));
        }
    }
    items.push("Reset all to defaults".into());
    Menu::with_headers("CONTROLS", items, headers)
}

/// The settings screen (both faces).
pub struct SettingsScreen {
    /// Which face is up.
    pub face: Face,
    /// The face's live menu.
    pub menu: Menu,
    /// The screen's status line, if one is up.
    pub notice: Option<Notice>,
    binding_slot: usize,
    back: crate::button::BackButton,
}

impl SettingsScreen {
    /// Opens on the settings rows. Where leaving lands is the
    /// coordinator's business — it keeps the displaced screen.
    pub fn open(config: &Config) -> Self {
        Self {
            face: Face::Settings,
            menu: settings_menu(config),
            notice: None,
            binding_slot: 0,
            back: crate::button::BackButton::default(),
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
            Face::Settings => settings_hint(crate::platform::TOUCH_ONLY),
            Face::Controls { rebinding: Some(_) } => {
                "press the new chord (modifiers held count) - Escape cancels"
            }
            Face::Controls { rebinding: None } => {
                if self.binding_slot == 0 {
                    "PRIMARY selected | Left/Right chooses column | Enter remaps | X clears | Esc back"
                } else {
                    "SECONDARY selected | Left/Right chooses column | Enter remaps | X clears | Esc back"
                }
            }
        }
    }

    /// Returns from Controls to the settings rows, on the Controls row.
    fn leave_controls(&mut self, config: &Config) {
        self.face = Face::Settings;
        self.menu = settings_menu(config);
        self.menu.select(Row::Controls.index());
        self.notice = None;
    }

    fn goto_controls(&mut self, config: &Config, select: usize) {
        self.face = Face::Controls { rebinding: None };
        self.menu = controls_menu(config, self.binding_slot);
        self.menu.select(select);
        self.notice = None;
    }

    /// Draws the face's menu and the screen-owned notice — the caller
    /// draws the veil first, so both land above it.
    pub fn draw(&self) {
        self.menu.draw(self.hint());
        if let Some(notice) = &self.notice {
            let s = render::ui_scale();
            let size = 16.0 * s;
            let width = measure_text(&notice.text, None, size as u16, 1.0).width;
            draw_text(
                &notice.text,
                (render::viewport().x - width) * 0.5,
                render::viewport().y - 48.0 * s,
                size,
                if notice.danger {
                    crate::theme::TEXT_DANGER
                } else {
                    crate::theme::TEXT_BODY
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
        // The BACK button steps out exactly as Escape does on each face,
        // capture included: it cancels a rebind without binding anything.
        let (back, routed) = self.back.route(events);
        let events = routed.as_slice();
        if back {
            sounds.push((SoundKind::Click, None));
            match self.face {
                Face::Settings => update.out = Out::Leave,
                Face::Controls { rebinding: Some(_) } => {
                    self.face = Face::Controls { rebinding: None };
                    self.notice = None;
                }
                Face::Controls { rebinding: None } => self.leave_controls(config),
            }
            return update;
        }
        let escaped = events
            .iter()
            .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
        match self.face {
            Face::Settings => {
                if escaped {
                    update.out = Out::Leave;
                } else if let Some(index) = self.menu.handle(events, mouse) {
                    let row = rows(crate::platform::TOUCH_ONLY)[index];
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
                    } else if row == Row::LeftHandedPreset {
                        // The left-handed preset replaces the whole
                        // profile (custom rebinds included — Controls'
                        // Reset row walks back to Classic).
                        config.bindings = BindingMap::left_handed();
                        config.unbound.clear();
                        update.dirty = true;
                        *live = config.bindings.clone();
                        self.notice = Some(Notice {
                            text: "left-handed profile applied".to_string(),
                            danger: false,
                        });
                        let selected = self.menu.selected;
                        self.menu = settings_menu(config);
                        self.menu.select(selected);
                    } else if row == Row::OpenDiagnostics {
                        update.out = Out::OpenDiagnostics;
                    } else if row == Row::ExportDiagnostics {
                        update.out = Out::ExportDiagnostics;
                    } else if row == Row::Controls {
                        self.goto_controls(config, 0);
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
                    Some((Key::Escape, false, false))
                        if control_rows()[row] != Some(Action::Back) =>
                    {
                        self.face = Face::Controls { rebinding: None };
                        self.notice = None;
                    }
                    Some((key, ctrl, shift)) => {
                        let Some(target) = control_rows()[row] else {
                            return update;
                        };
                        let chord = Chord { key, ctrl, shift };
                        if config
                            .bindings
                            .rebind_slot(target, self.binding_slot, chord)
                        {
                            // Bound again: the unbind tombstone lifts.
                            config.unbound.retain(|a| *a != target);
                            update.dirty = true;
                            *live = config.bindings.clone();
                            self.goto_controls(config, row);
                        } else {
                            // Refused: name the holder, so the player
                            // knows which row to unbind first.
                            let text =
                                match config.bindings.conflict(target, self.binding_slot, chord) {
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
                if let Some(key) = events.iter().find_map(|event| match event {
                    RawEvent::KeyDown { key }
                        if matches!(key, Key::Left | Key::Right | Key::Tab) =>
                    {
                        Some(*key)
                    }
                    _ => None,
                }) {
                    self.binding_slot = match key {
                        Key::Left => 0,
                        Key::Right => 1,
                        _ => 1 - self.binding_slot,
                    };
                    let row = self.menu.selected;
                    self.goto_controls(config, row);
                } else if escaped {
                    self.leave_controls(config);
                } else if x_pressed
                    && control_rows()
                        .get(self.menu.selected)
                        .is_some_and(Option::is_some)
                {
                    // X on a row unbinds it — outside capture mode, so
                    // the key is free to mean this. The tombstone
                    // records the CHOICE: without it, the next load's
                    // new-verb migration would read the missing row as
                    // an old config and restore the classic chord.
                    let target = control_rows()[self.menu.selected].expect("action row");
                    config.bindings.unbind_slot(target, self.binding_slot);
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
                    if events.iter().any(|e| {
                        matches!(
                            e,
                            RawEvent::MouseDown { .. }
                                | RawEvent::TouchDown { .. }
                                | RawEvent::MouseUp { .. }
                                | RawEvent::TouchUp { .. }
                        )
                    }) && let Some(rect) = self.menu.item_rect(row)
                    {
                        self.binding_slot = usize::from(mouse.x >= rect.x + rect.w * 0.8);
                    }
                    if control_rows().get(row).is_some_and(Option::is_some) {
                        self.face = Face::Controls {
                            rebinding: Some(row),
                        };
                    } else if row == control_rows().len() {
                        // Reset to defaults — tombstones included.
                        config.bindings = BindingMap::classic();
                        config.unbound.clear();
                        update.dirty = true;
                        *live = config.bindings.clone();
                        self.goto_controls(config, row);
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

    #[test]
    fn the_settings_hint_speaks_touch_on_touch_only_builds() {
        assert!(settings_hint(false).contains("{confirm}"));
        crate::platform::assert_touch_copy(settings_hint(true));
    }

    #[test]
    fn a_touch_only_build_hides_rows_it_cannot_use() {
        let touch = rows(true);
        for hidden in [
            Row::EdgePan,
            Row::LeftHandedPreset,
            Row::Controls,
            Row::OpenDiagnostics,
        ] {
            assert!(!touch.contains(&hidden), "{hidden:?} needs a desktop");
        }
        assert_eq!(touch.len(), Row::ALL.len() - 4);
        assert_eq!(rows(false), Row::ALL.to_vec(), "desktop keeps every row");
    }

    #[test]
    fn marker_settings_apply_live_and_remain_touch_reachable_in_small_windows() {
        crate::render::set_viewport(640.0, 400.0);
        crate::render::set_user_scale(1.5);
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut screen = SettingsScreen::open(&config);
        screen.menu.select(Row::MarkerTiming.index());
        for (label, midpoint) in [("Earlier", 26.0), ("Later", 14.0), ("Standard", 20.0)] {
            let update = drive(
                &mut screen,
                &mut config,
                &mut live,
                &press(Key::Enter),
                false,
            );
            assert!(update.dirty);
            assert!(screen.menu.items[Row::MarkerTiming.index()].ends_with(label));
            assert_eq!(crate::strategic_markers::marker_alpha(midpoint), 0.5);
        }
        screen.menu.select(Row::MarkerSize.index());
        for size in [1.25, 1.5, 0.75, 1.0] {
            let rect = screen
                .menu
                .item_rect(Row::MarkerSize.index())
                .expect("selected size row visible");
            let (x, y) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
            let update = drive(
                &mut screen,
                &mut config,
                &mut live,
                &[
                    RawEvent::TouchDown { id: 1, x, y },
                    RawEvent::TouchUp { id: 1, x, y },
                ],
                false,
            );
            assert!(update.dirty);
            assert_eq!(config.markers.scale, size);
            assert_eq!(crate::strategic_markers::prefs(), config.markers);
        }
        crate::render::set_viewport(1280.0, 800.0);
        crate::render::set_user_scale(1.0);
    }

    #[test]
    fn the_back_button_steps_out_one_level_like_escape() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut screen = SettingsScreen::open(&config);
        assert!(!screen.menu.items.iter().any(|item| item == "Back"));
        let back = crate::button::press_back(true);

        screen.goto_controls(&config, 1);
        screen.face = Face::Controls { rebinding: Some(1) };
        let update = drive(&mut screen, &mut config, &mut live, &back, false);
        assert_eq!(
            screen.face,
            Face::Controls { rebinding: None },
            "cancels capture"
        );
        assert!(!update.dirty, "a cancelled capture binds nothing");
        assert_eq!(config.bindings, BindingMap::classic());

        drive(&mut screen, &mut config, &mut live, &back, false);
        assert_eq!(screen.face, Face::Settings);
        assert_eq!(screen.menu.selected, Row::Controls.index());

        let update = drive(&mut screen, &mut config, &mut live, &back, false);
        assert_eq!(update.out, Out::Leave);
    }

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
        for _ in 0..Row::ReducedMotion.index() {
            drive(&mut s, &mut config, &mut live, &press(Key::Down), false);
        }
        let up = drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        assert!(config.reduced_motion);
        assert!(up.dirty, "the caller is told to persist");
        assert_eq!(
            s.menu.selected,
            Row::ReducedMotion.index(),
            "the cursor stays on the tuned row"
        );
    }

    #[test]
    fn music_volume_is_a_live_persisted_settings_row() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut screen = SettingsScreen::open(&config);
        let music = Row::MusicVolume.index();
        for _ in 0..music {
            drive(
                &mut screen,
                &mut config,
                &mut live,
                &press(Key::Down),
                false,
            );
        }
        let update = drive(
            &mut screen,
            &mut config,
            &mut live,
            &press(Key::Enter),
            false,
        );
        assert!(update.dirty);
        assert_eq!(config.volumes.music, 0.0);
        assert_eq!(screen.menu.selected, music);
        assert_eq!(screen.menu.items[music], "Music volume: 0%");
    }

    #[test]
    fn music_volume_is_touch_reachable() {
        crate::render::set_viewport(1280.0, 800.0);
        crate::render::set_user_scale(1.0);
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut screen = SettingsScreen::open(&config);
        let row = screen.menu.item_rect(3).expect("music row is visible");
        let x = row.x + row.w * 0.5;
        let y = row.y + row.h * 0.5;
        // Another layout test must not move the row between measurement and input.
        std::thread::spawn(|| crate::render::set_user_scale(0.75))
            .join()
            .unwrap();
        let update = drive(
            &mut screen,
            &mut config,
            &mut live,
            &[
                RawEvent::TouchDown { id: 11, x, y },
                RawEvent::TouchUp { id: 11, x, y },
            ],
            false,
        );
        assert!(update.dirty);
        assert_eq!(config.volumes.music, 0.0);
        assert_eq!(screen.menu.selected, 3);
    }

    #[test]
    fn performance_cycles_with_keyboard_mouse_and_touch_in_small_windows() {
        use crate::config::PerformanceDisplay;
        use oxide_protocol::MouseButton;
        for scale in [0.75, 1.0, 1.25, 1.5] {
            crate::render::set_viewport(640.0, 400.0);
            crate::render::set_user_scale(scale);
            let mut config = Config::default();
            let mut live = config.bindings.clone();
            let mut screen = SettingsScreen::open(&config);
            for _ in 0..Row::PerformanceDisplay.index() {
                drive(
                    &mut screen,
                    &mut config,
                    &mut live,
                    &press(Key::Down),
                    false,
                );
            }
            for mode in [
                PerformanceDisplay::Fps,
                PerformanceDisplay::Detailed,
                PerformanceDisplay::Off,
            ] {
                let rect = screen
                    .menu
                    .item_rect(Row::PerformanceDisplay.index())
                    .expect("selected row visible");
                let (x, y) = (rect.center().x, rect.center().y);
                let events = match mode {
                    PerformanceDisplay::Fps => press(Key::Enter),
                    PerformanceDisplay::Detailed => vec![
                        RawEvent::MouseDown {
                            button: MouseButton::Left,
                            x,
                            y,
                        },
                        RawEvent::MouseUp {
                            button: MouseButton::Left,
                            x,
                            y,
                        },
                    ],
                    PerformanceDisplay::Off => vec![
                        RawEvent::TouchDown { id: 1, x, y },
                        RawEvent::TouchUp { id: 1, x, y },
                    ],
                };
                let update = drive(&mut screen, &mut config, &mut live, &events, false);
                assert!(update.dirty);
                assert_eq!(config.performance_display, mode);
                assert_eq!(screen.menu.selected, Row::PerformanceDisplay.index());
                assert_eq!(
                    screen.menu.items[Row::PerformanceDisplay.index()],
                    format!("Performance display: {}", mode.label())
                );
            }
            assert_eq!(
                screen.menu.items[Row::LeftHandedPreset.index()],
                "Apply left-handed bindings"
            );
            assert_eq!(screen.menu.items[Row::Controls.index()], "Controls...");
        }
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
        s.goto_controls(
            &config,
            control_rows()
                .iter()
                .position(|a| *a == Some(Action::Patrol))
                .unwrap(),
        ); // Patrol row
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), true);
        assert!(matches!(s.face, Face::Controls { rebinding: Some(_) }));
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
        s.goto_controls(
            &config,
            control_rows()
                .iter()
                .position(|a| *a == Some(Action::Patrol))
                .unwrap(),
        );
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
        s.goto_controls(
            &config,
            control_rows()
                .iter()
                .position(|a| *a == Some(Action::Patrol))
                .unwrap(),
        );
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        // M already means Run.
        let up = drive(&mut s, &mut config, &mut live, &press(Key::M), false);
        assert!(!up.dirty);
        let notice = s.notice.as_ref().expect("the refusal reports");
        assert_eq!(
            notice.text,
            "M is already bound to Run (move without engaging)"
        );
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
        s.goto_controls(
            &config,
            control_rows()
                .iter()
                .position(|a| *a == Some(Action::Patrol))
                .unwrap(),
        );
        drive(&mut s, &mut config, &mut live, &press(Key::Enter), false);
        drive(&mut s, &mut config, &mut live, &press(Key::Num1), false);
        assert_eq!(
            s.notice.as_ref().map(|n| n.text.as_str()),
            Some("1 is already bound to Recall group 1")
        );
    }

    #[test]
    fn leaving_the_controls_face_clears_the_notice() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(
            &config,
            control_rows()
                .iter()
                .position(|a| *a == Some(Action::Patrol))
                .unwrap(),
        );
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
        for _ in 0..Row::LeftHandedPreset.index() {
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
            Some(Chord::bare(Key::O)),
            "training moved to the right hand"
        );
        assert_eq!(
            config.bindings.chord_at(Action::PanLeft, 1),
            Some(Chord::bare(Key::Left)),
            "pans stay on the arrows"
        );
        assert!(
            config.bindings.conflicts().is_empty(),
            "the preset must be conflict-free"
        );
        assert_eq!(live.chord_for(Action::Patrol), Some(Chord::bare(Key::Y)));
    }

    #[test]
    fn x_unbinds_and_reset_restores_the_classic_map() {
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut s = SettingsScreen::open(&config);
        s.goto_controls(
            &config,
            control_rows()
                .iter()
                .position(|a| *a == Some(Action::Patrol))
                .unwrap(),
        );
        let up = drive(&mut s, &mut config, &mut live, &press(Key::X), false);
        assert!(up.dirty);
        assert_eq!(config.bindings.chord_for(Action::Patrol), None);
        // Reset row restores everything.
        s.menu.select(control_rows().len());
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
            settings_menu(&config).items[Row::Controls.index()],
            "Controls...",
            "the derived index names the row it claims"
        );
        screen.menu.select(Row::Controls.index());
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
            screen.menu.selected,
            Row::Controls.index(),
            "the cursor comes back to the row that was activated"
        );
    }
    #[test]
    fn every_default_action_is_editable_and_secondary_edit_does_not_replace_primary() {
        let mut config = Config::default();
        let rows = control_rows();
        for binding in config.bindings.bindings() {
            assert!(rows.contains(&Some(binding.action)), "{:?}", binding.action);
        }
        let row = rows.iter().position(|a| *a == Some(Action::PanUp)).unwrap();
        let mut live = config.bindings.clone();
        let mut screen = SettingsScreen::open(&config);
        screen.goto_controls(&config, row);
        drive(
            &mut screen,
            &mut config,
            &mut live,
            &press(Key::Right),
            false,
        );
        drive(
            &mut screen,
            &mut config,
            &mut live,
            &press(Key::Enter),
            false,
        );
        assert!(drive(&mut screen, &mut config, &mut live, &press(Key::I), false).dirty);
        assert_eq!(
            config.bindings.chord_at(Action::PanUp, 0),
            Some(Chord::bare(Key::W))
        );
        assert_eq!(
            config.bindings.chord_at(Action::PanUp, 1),
            Some(Chord::bare(Key::I))
        );
        drive(&mut screen, &mut config, &mut live, &press(Key::X), false);
        assert_eq!(config.bindings.chord_at(Action::PanUp, 1), None);
        assert_eq!(
            config.bindings.chord_at(Action::PanUp, 0),
            Some(Chord::bare(Key::W))
        );
    }
    #[test]
    fn clicking_the_secondary_column_selects_it_even_when_release_is_a_later_frame() {
        use oxide_protocol::MouseButton;
        let mut config = Config::default();
        let mut live = config.bindings.clone();
        let mut screen = SettingsScreen::open(&config);
        let row = control_rows()
            .iter()
            .position(|a| *a == Some(Action::PanUp))
            .unwrap();
        screen.goto_controls(&config, row);
        let rect = screen.menu.item_rect(row).unwrap();
        let (x, y) = (rect.x + rect.w * 0.9, rect.y + rect.h * 0.5);
        drive(
            &mut screen,
            &mut config,
            &mut live,
            &[RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            }],
            false,
        );
        assert!(matches!(screen.face, Face::Controls { rebinding: None }));
        drive(
            &mut screen,
            &mut config,
            &mut live,
            &[RawEvent::MouseUp {
                button: MouseButton::Left,
                x,
                y,
            }],
            false,
        );
        assert_eq!(screen.binding_slot, 1);
        assert!(drive(&mut screen, &mut config, &mut live, &press(Key::I), false).dirty);
        assert_eq!(live.chord_at(Action::PanUp, 0), Some(Chord::bare(Key::W)));
        assert_eq!(live.chord_at(Action::PanUp, 1), Some(Chord::bare(Key::I)));
    }

    #[test]
    fn diagnostics_is_opt_in_and_keeps_existing_settings_rows_stable() {
        let mut config = Config::default();
        assert!(!config.diagnostics);
        assert_eq!(
            settings_menu(&config).items[Row::Diagnostics.index()],
            "Diagnostics: off"
        );
        assert!(cycle_setting(&mut config, Row::Diagnostics));
        assert!(config.diagnostics);
        assert_eq!(
            settings_menu(&config).items[Row::Diagnostics.index()],
            "Diagnostics: on"
        );
        assert_eq!(
            settings_menu(&config).items[Row::OpenDiagnostics.index()],
            "Open diagnostics folder"
        );
        assert_eq!(
            settings_menu(&config).items[Row::ExportDiagnostics.index()],
            "Export diagnostic report"
        );
        assert_eq!(
            settings_menu(&config).items[Row::Controls.index()],
            "Controls..."
        );
        let mut old = serde_json::to_value(&config).unwrap();
        old.as_object_mut().unwrap().remove("diagnostics");
        assert!(!serde_json::from_value::<Config>(old).unwrap().diagnostics);
    }
}
