//! The pause menu and its confirmation dialogs — one screen object.
//! Windowless update; the main loop performs the session verbs
//! (resume, watch, settings, restart, main menu, quit) and draws.

use crate::game::SoundKind;
use crate::menu::{Label, Menu};
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
    /// The confirmation this row asks before acting, if it asks one.
    fn confirmable(self) -> Option<Confirmable> {
        match self {
            Row::Surrender => Some(Confirmable::Surrender),
            Row::Restart => Some(Confirmable::Restart),
            Row::MainMenu => Some(Confirmable::MainMenu),
            Row::Quit => Some(Confirmable::Quit),
            Row::Resume | Row::SaveGame | Row::WatchReplay | Row::Settings | Row::Roster => None,
        }
    }

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

/// A pause verb that asks before carrying out its consequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmable {
    Surrender,
    Restart,
    MainMenu,
    Quit,
}

impl Confirmable {
    /// The pause row that asked.
    fn row(self) -> Row {
        match self {
            Confirmable::Surrender => Row::Surrender,
            Confirmable::Restart => Row::Restart,
            Confirmable::MainMenu => Row::MainMenu,
            Confirmable::Quit => Row::Quit,
        }
    }

    /// What confirming does.
    fn out(self) -> Out {
        match self {
            Confirmable::Surrender => Out::Surrender,
            Confirmable::Restart => Out::Restart,
            Confirmable::MainMenu => Out::MainMenu,
            Confirmable::Quit => Out::Quit,
        }
    }

    /// The dialog's subtitle: what confirming costs.
    fn consequence(self) -> &'static str {
        match self {
            Confirmable::Surrender => "this concedes the match",
            Confirmable::Restart => "progress is discarded and the match starts over",
            Confirmable::MainMenu => "the match is saved before returning home",
            Confirmable::Quit => "the match is saved before quitting",
        }
    }
}

/// What a row of any pause face stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// A pause row.
    Row(Row),
    /// Back out of a dialog without acting.
    Cancel,
    /// Carry out the verb a confirmation dialog asks about.
    Confirm,
    /// Try a failed save again.
    Retry,
    /// Leave without the save that failed.
    LeaveUnsaved,
    /// The save-name field's one row.
    Field,
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

/// Which face of the pause screen is up.
enum Face {
    /// The pause rows.
    Rows,
    /// A verb asking for confirmation.
    Confirm(Confirmable),
    /// A leave verb's autosave refused.
    SaveFailed(SaveFailed),
    /// The save-name field has focus. Only here do Text events mean
    /// anything; letters stay semantic everywhere else.
    Naming(TextField),
}

/// The pause screen: its rows and whichever face is up.
pub struct PauseScreen {
    /// The live menu (pause rows, the two-row confirm dialog, the
    /// save-failure dialog, or the name field's row).
    pub menu: Menu<Choice>,
    /// The displayed rows, in menu order.
    rows: Vec<Row>,
    face: Face,
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

fn pause_menu(rows: &[Row], lan: bool) -> Menu<Choice> {
    Menu::rows(
        if lan { "MENU" } else { "PAUSED" },
        rows.iter()
            .map(|row| (Label::from(row.label()), Choice::Row(*row))),
    )
}

fn confirm_menu(verb: Confirmable) -> Menu<Choice> {
    let label = verb.row().label();
    // Cancel sits first and preselected: a consequential choice takes a
    // deliberate second motion, never a double-tap.
    Menu::rows(
        format!("{}?", label.to_uppercase()),
        [
            (Label::from("Cancel"), Choice::Cancel),
            (Label::from(label), Choice::Confirm),
        ],
    )
}

impl PauseScreen {
    /// Opens on the pause rows.
    pub fn open(finished: bool, can_surrender: bool) -> Self {
        let rows = rows(finished, can_surrender, !crate::platform::TOUCH_ONLY);
        Self {
            menu: pause_menu(&rows, false),
            rows,
            face: Face::Rows,
            notice: None,
            lan: false,
        }
    }

    /// Consumes a request to raise the on-screen keyboard again.
    pub fn take_keyboard_request(&mut self) -> bool {
        match &mut self.face {
            Face::Naming(field) => field.take_keyboard_request(),
            Face::Rows | Face::Confirm(_) | Face::SaveFailed(_) => false,
        }
    }

    /// Draws the menu while a save this screen asked for runs: the same
    /// dialog, dimmed, saying so, so leaving never flashes another screen.
    pub fn draw_saving(&self, line: &str) {
        self.menu.draw_busy(line);
    }

    /// Draws the current face: the menu, or the name field with its
    /// Save and Cancel buttons.
    pub fn draw(&self, scenario_name: &str, mouse: Vec2) {
        match &self.face {
            Face::Naming(field) => field.draw(
                naming_hint(crate::platform::TOUCH_ONLY),
                crate::hints::fade(crate::theme::TEXT_SECONDARY),
                mouse,
            ),
            Face::Rows | Face::Confirm(_) | Face::SaveFailed(_) => {
                self.menu.draw(self.subtitle(scenario_name));
            }
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
        self.show_notice(notice);
        self
    }

    /// Shows `notice` as the subtitle until the next activation.
    pub fn show_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    /// Longest save name the field accepts — what the shelf row can
    /// show without eliding.
    pub const NAME_MAX: usize = 26;

    /// Opens the name field over the pause menu, prefilled with the
    /// caller's suggestion so Enter-Enter saves without typing.
    pub fn begin_naming(&mut self, suggested: &str) {
        let field = TextField::new("SAVE GAME", "SAVE", suggested, Self::NAME_MAX);
        self.menu = field.menu(Choice::Field);
        self.face = Face::Naming(field);
    }

    /// Reports the save verdict and returns to the pause rows, cursor
    /// back on Save Game, the verdict as the subtitle.
    pub fn end_naming(&mut self, notice: String) {
        self.back_to_rows(Row::SaveGame);
        self.notice = Some(notice);
    }

    /// Returns to the pause rows with the cursor on `row`.
    fn back_to_rows(&mut self, row: Row) {
        self.face = Face::Rows;
        self.menu = pause_menu(&self.rows, self.lan);
        self.menu.select_where(|choice| *choice == Choice::Row(row));
    }

    /// Opens straight onto the save-failure dialog: a leave verb's
    /// autosave refused, and exiting silently would be data loss. The
    /// safe Cancel row sits preselected; Leave without saving is always
    /// reachable, so a full disk can never trap the player in the game.
    pub fn with_save_failed(mut self, line: String, verb: LeaveVerb, cancel_home: bool) -> Self {
        let mut menu = Menu::rows(
            "COULD NOT SAVE",
            [
                (Label::from("Retry"), Choice::Retry),
                (Label::from("Cancel"), Choice::Cancel),
                (Label::from("Leave without saving"), Choice::LeaveUnsaved),
            ],
        );
        menu.select(1);
        self.menu = menu;
        self.face = Face::SaveFailed(SaveFailed {
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

    /// Whether the save-failure dialog is up.
    pub fn saving_failed(&self) -> bool {
        matches!(self.face, Face::SaveFailed(_))
    }

    /// Whether the save-name field has focus.
    pub fn naming(&self) -> bool {
        matches!(self.face, Face::Naming(_))
    }

    /// The debug protocol's stable mode name for the current face.
    pub fn mode_name(&self) -> &'static str {
        match self.face {
            Face::Rows => "pause_menu",
            Face::Confirm(_) => "confirm_pause",
            Face::SaveFailed(_) => "save_failed",
            Face::Naming(_) => "save_name",
        }
    }

    /// The subtitle for the current face of the screen.
    pub fn subtitle<'a>(&'a self, scenario_name: &'a str) -> &'a str {
        match &self.face {
            Face::SaveFailed(dialog) => &dialog.line,
            Face::Naming(_) => naming_hint(crate::platform::TOUCH_ONLY),
            Face::Confirm(verb) => verb.consequence(),
            Face::Rows => self.notice.as_deref().unwrap_or(scenario_name),
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
        if let Face::Naming(field) = &mut self.face {
            match field.update(events, sounds) {
                Edit::Commit(name) => return Out::Save(name),
                Edit::Stay => self.menu = field.menu(Choice::Field),
                Edit::Cancel => self.back_to_rows(Row::SaveGame),
            }
            return Out::Stay;
        }
        let picked = self.menu.handle(events, mouse).map(|picked| picked.value);
        match &self.face {
            Face::SaveFailed(dialog) => {
                let (verb, cancel_home) = (dialog.verb, dialog.cancel_home);
                match picked {
                    Some(Choice::Retry) => return Out::RetrySave(verb, cancel_home),
                    Some(Choice::LeaveUnsaved) => return Out::LeaveUnsaved(verb),
                    Some(_) => {}
                    None if escaped => {}
                    None => return Out::Stay,
                }
                // Cancel (or Escape — never the leave verb): back to the
                // rows, cursor on the verb that raised the dialog, or back
                // to the front door that asked.
                if cancel_home {
                    self.face = Face::Rows;
                    return Out::Home;
                }
                self.back_to_rows(match verb {
                    LeaveVerb::MainMenu => Row::MainMenu,
                    LeaveVerb::Quit => Row::Quit,
                });
                Out::Stay
            }
            Face::Confirm(verb) => {
                let verb = *verb;
                if escaped || picked == Some(Choice::Cancel) {
                    // The cursor returns to the armed row.
                    self.back_to_rows(verb.row());
                    return Out::Stay;
                }
                if picked == Some(Choice::Confirm) {
                    return verb.out();
                }
                Out::Stay
            }
            Face::Rows | Face::Naming(_) => {
                let Some(Choice::Row(row)) = picked else {
                    return if escaped { Out::Resume } else { Out::Stay };
                };
                sounds.push((SoundKind::Click, None));
                self.notice = None;
                if let Some(verb) = row.confirmable() {
                    // Surrender, Restart, Main Menu, and Quit each ask
                    // before carrying out their distinct consequence.
                    self.face = Face::Confirm(verb);
                    self.menu = confirm_menu(verb);
                    return Out::Stay;
                }
                match row {
                    Row::Resume => Out::Resume,
                    Row::SaveGame => Out::SaveGame,
                    Row::WatchReplay => Out::WatchReplay,
                    Row::Settings => Out::Settings,
                    Row::Roster => Out::Roster,
                    Row::Surrender | Row::Restart | Row::MainMenu | Row::Quit => Out::Stay,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
