//! The pause menu and its confirmation dialogs — one screen object.
//! Windowless update; the main loop performs the session verbs
//! (resume, watch, settings, restart, main menu, quit) and draws.

use crate::game::SoundKind;
use crate::menu::Menu;
use crate::text_field::{Edit, TextField};
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};

/// The name field's coaching line; a touch-only build has no keys to
/// name.
fn naming_hint(touch_only: bool) -> &'static str {
    if touch_only {
        "type a name"
    } else {
        "type a name | Enter saves | Esc cancels"
    }
}

/// One pause row. The row set is conditional (Watch Replay only once
/// the match is decided), so rows are values, not indices: the confirm
/// step and the cursor return key off the row itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Back to the match.
    Resume,
    /// Write a named save while the match is still running
    /// (non-destructive; never confirms).
    SaveGame,
    /// Watch the session so far (decided matches only).
    WatchReplay,
    /// Tune settings over the paused match.
    Settings,
    /// Read the codex over the paused match.
    Roster,
    /// Concede the human's seat (confirms; mid-match only — a decided
    /// match has nothing left to give up).
    Surrender,
    /// Rebuild the match (confirms).
    Restart,
    /// Abandon to the front door (confirms).
    MainMenu,
    /// Leave the process (confirms).
    Quit,
}

impl Row {
    fn label(self) -> &'static str {
        match self {
            Row::Resume => "Resume",
            Row::SaveGame => "Save Game",
            Row::WatchReplay => "Watch Replay",
            Row::Settings => "Settings",
            Row::Roster => "Roster",
            Row::Surrender => "Surrender",
            Row::Restart => "Restart",
            Row::MainMenu => "Main Menu",
            Row::Quit => "Quit",
        }
    }
}

/// The rows the current match state offers, in display order. Watch
/// Replay belongs to decided matches (mid-match playback would be a
/// fog-free scout of the enemy); Save Game and Surrender to
/// running ones, with Surrender further limited to a seat that still
/// has a voice — a resigned or eliminated spectator is shown no verb
/// the sim would only reject. Quit is left out where the platform, not
/// the app, closes apps.
fn rows(finished: bool, can_surrender: bool, quit: bool) -> Vec<Row> {
    let mut rows = vec![Row::Resume];
    if finished {
        rows.push(Row::WatchReplay);
    } else {
        rows.push(Row::SaveGame);
    }
    rows.push(Row::Settings);
    rows.push(Row::Roster);
    if !finished && can_surrender {
        rows.push(Row::Surrender);
    }
    rows.extend([Row::Restart, Row::MainMenu]);
    if quit {
        rows.push(Row::Quit);
    }
    rows
}

/// The verb a save-failure dialog is holding open: what the player
/// was leaving toward when the autosave refused to land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaveVerb {
    /// Abandon to the front door.
    MainMenu,
    /// Leave the process.
    Quit,
}

/// What a pause frame decided. Confirmed verbs only ever emerge after
/// the confirmation step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// Still paused.
    Stay,
    /// Back to the match.
    Resume,
    /// Save Game was picked; the caller supplies the suggested name via
    /// [`PauseScreen::begin_naming`] (only the session knows its map
    /// and tick).
    SaveGame,
    /// The name field committed: write the save under this name. The
    /// caller reports the verdict through [`PauseScreen::end_naming`].
    Save(String),
    /// Watch the session so far.
    WatchReplay,
    /// Open Settings over the paused match; this screen waits intact.
    Settings,
    /// Open the codex over the paused match; this screen waits intact.
    Roster,
    /// Confirmed: concede the human's seat. The caller issues the sim
    /// command and returns to the match — the result (or the team
    /// game's concede overlay) arrives with the next tick.
    Surrender,
    /// Confirmed: rebuild the match.
    Restart,
    /// Confirmed: abandon to the front door.
    MainMenu,
    /// Confirmed: leave the process.
    Quit,
    /// Try the failed save again, then perform the verb if it lands.
    RetrySave(LeaveVerb, bool),
    /// Perform the verb without a save — the row that guarantees a
    /// player on a permanently full disk can always leave.
    LeaveUnsaved(LeaveVerb),
    /// Back to the front door (a Home-origin failure dialog cancelled).
    Home,
}

/// The pause screen: its menu, plus the armed row while a confirmation
/// dialog is up.
pub struct PauseScreen {
    /// The live menu (pause rows, the two-row confirm dialog, or the
    /// save-failure dialog).
    pub menu: Menu,
    /// The displayed rows, in menu order.
    rows: Vec<Row>,
    /// Which row is awaiting confirmation, if any.
    confirming: Option<Row>,
    /// The save-failure dialog, if a leave verb's autosave refused.
    save_failed: Option<SaveFailed>,
    /// The save-name field while it has focus. Only here do Text events
    /// mean anything; letters stay semantic everywhere else.
    naming: Option<TextField>,
    /// A one-line verdict from the last explicit save (success or
    /// failure), shown as the subtitle until the next activation.
    notice: Option<String>,
    /// Whether this is the menu over a LAN match (see
    /// [`PauseScreen::for_lan_match`]).
    lan: bool,
}

/// The state a save-failure dialog holds open.
struct SaveFailed {
    /// The verb waiting on the save.
    verb: LeaveVerb,
    /// The player-facing failure sentence (the dialog's subtitle).
    line: String,
    /// Whether Cancel returns to the front door (the dialog was raised
    /// from Home or a window close outside a match) instead of the
    /// pause rows.
    cancel_home: bool,
}

fn pause_menu(rows: &[Row], lan: bool) -> Menu {
    Menu::new(
        if lan { "MENU" } else { "PAUSED" },
        rows.iter().map(|row| row.label().to_string()).collect(),
    )
}

fn confirm_menu(row: Row) -> Menu {
    let verb = row.label();
    // Cancel sits first and preselected: a consequential choice takes a
    // deliberate second motion, never a double-tap.
    Menu::new(
        format!("{}?", verb.to_uppercase()),
        vec!["Cancel".to_string(), verb.to_string()],
    )
}

impl PauseScreen {
    /// Opens on the pause rows.
    pub fn open(finished: bool, can_surrender: bool) -> Self {
        let rows = rows(finished, can_surrender, !crate::platform::TOUCH_ONLY);
        Self {
            menu: pause_menu(&rows, false),
            rows,
            confirming: None,
            save_failed: None,
            naming: None,
            notice: None,
            lan: false,
        }
    }

    /// Consumes a request to raise the on-screen keyboard again.
    pub fn take_keyboard_request(&mut self) -> bool {
        self.naming
            .as_mut()
            .is_some_and(TextField::take_keyboard_request)
    }

    /// Draws the menu while a save this screen asked for runs: the same
    /// dialog, dimmed, saying so, so leaving never flashes another screen.
    pub fn draw_saving(&self, line: &str) {
        self.menu.draw_busy(line);
    }

    /// Draws the current face: the menu, or the name field with its
    /// Save and Cancel buttons.
    pub fn draw(&self, scenario_name: &str, mouse: Vec2) {
        match &self.naming {
            Some(field) => field.draw(
                naming_hint(crate::platform::TOUCH_ONLY),
                crate::hints::fade(crate::theme::TEXT_SECONDARY),
                mouse,
            ),
            None => self.menu.draw(self.subtitle(scenario_name)),
        }
    }

    /// The menu over a LAN match, which keeps running underneath: no
    /// saving and no restart.
    pub fn for_lan_match(mut self) -> Self {
        self.rows
            .retain(|row| !matches!(row, Row::SaveGame | Row::Restart));
        self.lan = true;
        self.menu = pause_menu(&self.rows, true);
        self
    }

    /// Shows `notice` as the subtitle until the next activation.
    pub fn with_notice(mut self, notice: impl Into<String>) -> Self {
        self.notice = Some(notice.into());
        self
    }

    /// Longest save name the field accepts — what the shelf row can
    /// show without eliding.
    pub const NAME_MAX: usize = 26;

    /// Opens the name field over the pause menu, prefilled with the
    /// caller's suggestion so Enter-Enter saves without typing.
    pub fn begin_naming(&mut self, suggested: &str) {
        let field = TextField::new("SAVE GAME", "SAVE", suggested, Self::NAME_MAX);
        self.menu = field.menu();
        self.naming = Some(field);
    }

    /// Reports the save verdict and returns to the pause rows, cursor
    /// back on Save Game, the verdict as the subtitle.
    pub fn end_naming(&mut self, notice: String) {
        self.naming = None;
        self.menu = pause_menu(&self.rows, self.lan);
        let display = self
            .rows
            .iter()
            .position(|&r| r == Row::SaveGame)
            .unwrap_or(0);
        self.menu.select(display);
        self.notice = Some(notice);
    }

    /// Opens straight onto the save-failure dialog: a leave verb's
    /// autosave refused, and exiting silently would be data loss. The
    /// safe Cancel row sits preselected; Leave without saving is always
    /// reachable, so a full disk can never trap the player in the game.
    pub fn with_save_failed(mut self, line: String, verb: LeaveVerb, cancel_home: bool) -> Self {
        let mut menu = Menu::new(
            "COULD NOT SAVE",
            vec![
                "Retry".to_string(),
                "Cancel".to_string(),
                "Leave without saving".to_string(),
            ],
        );
        menu.select(1);
        self.menu = menu;
        self.save_failed = Some(SaveFailed {
            verb,
            line,
            cancel_home,
        });
        self
    }

    /// Whether the rows were built for a decided match.
    pub fn decided(&self) -> bool {
        self.rows.contains(&Row::WatchReplay)
    }

    /// Whether the confirmation dialog is up (for the mode report).
    pub fn confirming(&self) -> bool {
        self.confirming.is_some()
    }

    /// Whether the save-failure dialog is up (for the mode report).
    pub fn saving_failed(&self) -> bool {
        self.save_failed.is_some()
    }

    /// Whether the save-name field has focus (for the mode report).
    pub fn naming(&self) -> bool {
        self.naming.is_some()
    }

    /// The subtitle for the current face of the screen.
    pub fn subtitle<'a>(&'a self, scenario_name: &'a str) -> &'a str {
        if let Some(dialog) = &self.save_failed {
            &dialog.line
        } else if self.naming.is_some() {
            naming_hint(crate::platform::TOUCH_ONLY)
        } else if let Some(row) = self.confirming {
            match row {
                Row::Surrender => "this concedes the match",
                Row::Restart => "progress is discarded and the match starts over",
                Row::MainMenu => "the match is saved before returning home",
                Row::Quit => "the match is saved before quitting",
                Row::Resume | Row::SaveGame | Row::WatchReplay | Row::Settings | Row::Roster => {
                    scenario_name
                }
            }
        } else if let Some(notice) = &self.notice {
            notice
        } else {
            scenario_name
        }
    }

    /// Applies a frame's events.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Out {
        let escaped = events
            .iter()
            .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
        if let Some(field) = self.naming.as_mut() {
            match field.update(events, sounds) {
                Edit::Commit(name) => return Out::Save(name),
                Edit::Stay => self.menu = field.menu(),
                Edit::Cancel => {
                    self.naming = None;
                    self.menu = pause_menu(&self.rows, self.lan);
                    let display = self
                        .rows
                        .iter()
                        .position(|&r| r == Row::SaveGame)
                        .unwrap_or(0);
                    self.menu.select(display);
                }
            }
            return Out::Stay;
        }
        let picked = self.menu.handle(events, mouse);
        if let Some(dialog) = &self.save_failed {
            let (verb, cancel_home) = (dialog.verb, dialog.cancel_home);
            match picked {
                Some(0) => return Out::RetrySave(verb, cancel_home),
                Some(2) => return Out::LeaveUnsaved(verb),
                Some(_) => {}
                None if escaped => {}
                None => return Out::Stay,
            }
            // Cancel (or Escape — never the leave verb): back to the
            // rows, cursor on the verb that raised the dialog, or back
            // to the front door that asked.
            self.save_failed = None;
            if cancel_home {
                return Out::Home;
            }
            let row = match verb {
                LeaveVerb::MainMenu => Row::MainMenu,
                LeaveVerb::Quit => Row::Quit,
            };
            self.menu = pause_menu(&self.rows, self.lan);
            let display = self.rows.iter().position(|&r| r == row).unwrap_or(0);
            self.menu.select(display);
            return Out::Stay;
        }
        if let Some(row) = self.confirming {
            if escaped || picked == Some(0) {
                self.confirming = None;
                self.menu = pause_menu(&self.rows, self.lan);
                // The cursor returns to the armed row.
                let display = self.rows.iter().position(|&r| r == row).unwrap_or(0);
                self.menu.select(display);
                return Out::Stay;
            }
            if picked == Some(1) {
                return match row {
                    Row::Surrender => Out::Surrender,
                    Row::Restart => Out::Restart,
                    Row::MainMenu => Out::MainMenu,
                    _ => Out::Quit,
                };
            }
            return Out::Stay;
        }
        if picked.is_some() {
            sounds.push((SoundKind::Click, None));
            self.notice = None;
        }
        match picked.map(|i| self.rows[i]) {
            Some(Row::Resume) => Out::Resume,
            Some(Row::SaveGame) => Out::SaveGame,
            Some(Row::WatchReplay) => Out::WatchReplay,
            Some(Row::Settings) => Out::Settings,
            Some(Row::Roster) => Out::Roster,
            Some(confirmed) => {
                // Surrender, Restart, Main Menu, and Quit each ask
                // before carrying out their distinct consequence.
                self.confirming = Some(confirmed);
                self.menu = confirm_menu(confirmed);
                Out::Stay
            }
            None if escaped => Out::Resume,
            None => Out::Stay,
        }
    }
}

#[cfg(test)]
mod tests;
