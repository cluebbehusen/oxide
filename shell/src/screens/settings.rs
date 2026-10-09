//! Settings and the Controls remap screen: one screen object with two
//! faces. Config edits and binding capture are pure state updated here;
//! persistence and drawing stay with the caller, so binding capture runs
//! under headless tests.

use crate::action::{Action, BindingMap, Chord};
use crate::config::Config;
use crate::game::SoundKind;
use crate::menu::Menu;
use crate::numeric;
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
/// the caller's veil, where a HUD toast would be hidden. It persists
/// until the next action, so it needs no wall clock.
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
    use Action::{
        AssignGroup, Back, Build, BuildCategory, ClearRally, Confirm, CycleIdleWorker, DeleteSave,
        HomeCamera, Hunt, JumpToLastAlert, MenuDown, MenuEnd, MenuHome, MenuLeft, MenuPageDown,
        MenuPageUp, MenuRight, MenuUp, PanDown, PanLeft, PanRight, PanUp, Patrol, RecallBookmark,
        RepairUnit, ReplayBack, ReplayEnd, ReplayForward, ReplayPause, ReplaySpeed, ReplayStart,
        ReplayStats, ReturnCargo, Run, Salvage, SetBookmark, SetRally, Slot, StopOrScrap,
        ToggleBuildPalette, ToggleOverlay, TogglePause, TrainSlot, Unload, Upgrade,
    };
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
                Hunt,
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
        (
            "Production",
            (0..crate::action::TRAIN_SLOTS).map(TrainSlot).collect(),
        ),
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
    ControlGroups,
    MarkerTiming,
    MarkerSize,
    LeftHandedPreset,
    Controls,
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
        Row::ControlGroups,
        Row::MarkerTiming,
        Row::MarkerSize,
        Row::LeftHandedPreset,
        Row::Controls,
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
            Row::ControlGroups => format!("Control groups: {}", onoff(config.control_groups)),
            Row::MarkerTiming => {
                format!("Show strategic markers: {}", config.markers.timing_label())
            }
            Row::MarkerSize => format!("Strategic marker size: {}", pct(config.markers.scale)),
            Row::LeftHandedPreset => "Apply left-handed bindings".to_string(),
            Row::Controls => "Controls...".to_string(),
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
        let next = (numeric::to_u32((v * 4.0).round()) + 1) % 5;
        next as f32 / 4.0
    };
    // 75 -> 100 -> 125 -> 150 -> 75.
    let scale_step = |v: f32| match numeric::to_u32((v * 100.0).round()) {
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
        Row::ControlGroups => {
            config.control_groups = !config.control_groups;
            render::set_control_groups(config.control_groups);
        }
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
                    .map_or_else(|| "unbound".into(), BindingMap::chord_label);
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

    /// The face's subtitle: the rebind prompt while a capture is armed,
    /// since it is the only sign the next key is being listened for.
    fn subtitle(&self) -> &'static str {
        match self.face {
            Face::Controls { rebinding: Some(_) } => {
                "press the new chord (modifiers held count) - Escape cancels"
            }
            _ => "",
        }
    }

    /// The face's coaching line, in the footer.
    fn coaching(&self) -> Option<&'static str> {
        match self.face {
            Face::Settings => Some(settings_hint(crate::platform::TOUCH_ONLY)),
            Face::Controls { rebinding: Some(_) } => None,
            Face::Controls { rebinding: None } => Some(if self.binding_slot == 0 {
                "PRIMARY selected | Left/Right chooses column | Enter remaps | X clears | Esc back"
            } else {
                "SECONDARY selected | Left/Right chooses column | Enter remaps | X clears | Esc back"
            }),
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
        self.menu
            .draw_with_coaching(self.subtitle(), self.coaching());
        if let Some(notice) = &self.notice {
            let s = render::ui_scale();
            let size = 16.0 * s;
            let width = measure_text(&notice.text, None, numeric::font_size(size), 1.0).width;
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
                    // Any activation clears the standing notice.
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
                // Armed: the next key is the answer, read raw before any
                // binding resolution so its current meaning does not fire.
                // The frame's edges replay in order from the frame-start
                // baseline, and the modifiers are read at the main key's
                // press: batch-final flags would miss a chord whose Ctrl
                // came back up later the same frame.
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
                    // X on a row unbinds it (outside capture mode, so the
                    // key is free to mean this).
                    let target = control_rows()[self.menu.selected].expect("action row");
                    config.bindings.unbind_slot(target, self.binding_slot);
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
                        // Reset to defaults.
                        config.bindings = BindingMap::classic();
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
mod tests;
