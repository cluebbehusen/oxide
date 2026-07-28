//! The pause menu and its destructive-choice confirmation — one screen
//! object. Windowless update; the main loop performs the session verbs
//! (resume, watch, settings, restart, main menu, quit) and draws.

use crate::game::SoundKind;
use crate::menu::Menu;
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};

/// One pause row. The row set is conditional (Watch Replay only once
/// the match is decided), so rows are values, not indices — the
/// confirm step and the cursor return key off the row itself, and a
/// new conditional row costs one variant, not an index-shift audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Back to the match.
    Resume,
    /// Watch the session so far (decided matches only).
    WatchReplay,
    /// Tune settings over the paused match.
    Settings,
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
            Row::WatchReplay => "Watch Replay",
            Row::Settings => "Settings",
            Row::Restart => "Restart",
            Row::MainMenu => "Main Menu",
            Row::Quit => "Quit",
        }
    }
}

/// The rows the current match state offers, in display order.
fn rows(finished: bool) -> Vec<Row> {
    let mut rows = vec![Row::Resume];
    if finished {
        rows.push(Row::WatchReplay);
    }
    rows.extend([Row::Settings, Row::Restart, Row::MainMenu, Row::Quit]);
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

/// What a pause frame decided. Destructive verbs only ever emerge
/// after the confirmation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// Still paused.
    Stay,
    /// Back to the match.
    Resume,
    /// Watch the session so far.
    WatchReplay,
    /// Open Settings over the paused match; this screen waits intact.
    Settings,
    /// Confirmed: rebuild the match.
    Restart,
    /// Confirmed: abandon to the front door.
    MainMenu,
    /// Confirmed: leave the process.
    Quit,
    /// Try the failed save again, then perform the verb if it lands.
    RetrySave(LeaveVerb),
    /// Perform the verb without a save — the row that guarantees a
    /// player on a permanently full disk can always leave.
    LeaveUnsaved(LeaveVerb),
    /// Back to the front door (a Home-origin failure dialog cancelled).
    Home,
}

/// The pause screen: its menu, plus the armed destructive row while
/// the confirmation dialog is up.
pub struct PauseScreen {
    /// The live menu (pause rows, the two-row confirm dialog, or the
    /// save-failure dialog).
    pub menu: Menu,
    /// The displayed rows, in menu order.
    rows: Vec<Row>,
    /// Which destructive row is awaiting confirmation, if any.
    confirming: Option<Row>,
    /// The save-failure dialog, if a leave verb's autosave refused.
    save_failed: Option<SaveFailed>,
    /// Whether the match is decided — only then does Watch Replay
    /// appear. Mid-match playback is a fog-free scout of the enemy;
    /// replays are an end-of-match affair.
    pub finished: bool,
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

fn confirm_menu(row: Row) -> Menu {
    let verb = row.label();
    // Cancel sits first and preselected: confirming destruction takes
    // a deliberate second motion, never a double-tap.
    Menu::new(
        format!("{}?", verb.to_uppercase()),
        vec!["Cancel".to_string(), verb.to_string()],
    )
}

impl PauseScreen {
    /// Opens on the pause rows.
    pub fn open(finished: bool) -> Self {
        let rows = rows(finished);
        let items: Vec<String> = rows.iter().map(|r| r.label().to_string()).collect();
        Self {
            menu: Menu::new("PAUSED", items),
            rows,
            confirming: None,
            save_failed: None,
            finished,
        }
    }

    /// Opens straight onto the save-failure dialog: a leave verb's
    /// autosave refused, and exiting silently would be data loss. The
    /// safe Cancel row sits preselected (the destructive-confirm house
    /// pattern); Leave without saving is always reachable, so a full
    /// disk can never trap the player in the game.
    pub fn open_save_failed(
        line: String,
        verb: LeaveVerb,
        finished: bool,
        cancel_home: bool,
    ) -> Self {
        let mut screen = Self::open(finished);
        let mut menu = Menu::new(
            "COULD NOT SAVE",
            vec![
                "Retry".to_string(),
                "Cancel".to_string(),
                "Leave without saving".to_string(),
            ],
        );
        menu.select(1);
        screen.menu = menu;
        screen.save_failed = Some(SaveFailed {
            verb,
            line,
            cancel_home,
        });
        screen
    }

    /// Whether the confirmation dialog is up (for the mode report).
    pub fn confirming(&self) -> bool {
        self.confirming.is_some()
    }

    /// Whether the save-failure dialog is up (for the mode report).
    pub fn saving_failed(&self) -> bool {
        self.save_failed.is_some()
    }

    /// Refreshes the failure sentence after a retry failed again —
    /// the dialog stays up, the reason stays current.
    pub fn set_save_failure_line(&mut self, line: String) {
        if let Some(dialog) = self.save_failed.as_mut() {
            dialog.line = line;
        }
    }

    /// The subtitle for the current face of the screen.
    pub fn subtitle<'a>(&'a self, scenario_name: &'a str) -> &'a str {
        if let Some(dialog) = &self.save_failed {
            &dialog.line
        } else if self.confirming.is_some() {
            "this throws the current match away"
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
        let picked = self.menu.handle(events, mouse);
        if let Some(dialog) = &self.save_failed {
            let (verb, cancel_home) = (dialog.verb, dialog.cancel_home);
            match picked {
                Some(0) => return Out::RetrySave(verb),
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
            self.menu = Self::open(self.finished).menu;
            let display = self.rows.iter().position(|&r| r == row).unwrap_or(0);
            self.menu.select(display);
            return Out::Stay;
        }
        if let Some(row) = self.confirming {
            if escaped || picked == Some(0) {
                self.confirming = None;
                self.menu = Self::open(self.finished).menu;
                // The cursor returns to the armed row.
                let display = self.rows.iter().position(|&r| r == row).unwrap_or(0);
                self.menu.select(display);
                return Out::Stay;
            }
            if picked == Some(1) {
                return match row {
                    Row::Restart => Out::Restart,
                    Row::MainMenu => Out::MainMenu,
                    _ => Out::Quit,
                };
            }
            return Out::Stay;
        }
        if picked.is_some() {
            sounds.push((SoundKind::Click, None));
        }
        match picked.map(|i| self.rows[i]) {
            Some(Row::Resume) => Out::Resume,
            Some(Row::WatchReplay) => Out::WatchReplay,
            Some(Row::Settings) => Out::Settings,
            Some(destructive) => {
                // Restart, Main Menu, and Quit all throw away a live
                // match — each asks first.
                self.confirming = Some(destructive);
                self.menu = confirm_menu(destructive);
                Out::Stay
            }
            None if escaped => Out::Resume,
            None => Out::Stay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn drive(p: &mut PauseScreen, key: Key) -> Out {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        p.update(
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
            &mut mouse,
            &mut sounds,
        )
    }

    /// Moves the cursor to the labeled row and activates it — by
    /// label, never by raw Down counts, so the tests survive row-set
    /// changes the way the index math never did.
    fn activate(p: &mut PauseScreen, label: &str) -> Out {
        let target = p
            .menu
            .items
            .iter()
            .position(|i| i == label)
            .unwrap_or_else(|| panic!("no row labeled {label} in {:?}", p.menu.items));
        while p.menu.selected < target {
            drive(p, Key::Down);
        }
        while p.menu.selected > target {
            drive(p, Key::Up);
        }
        drive(p, Key::Enter)
    }

    #[test]
    fn destructive_rows_confirm_with_cancel_preselected() {
        let mut p = PauseScreen::open(false);
        assert_eq!(activate(&mut p, "Restart"), Out::Stay, "Restart only arms");
        assert!(p.confirming(), "the dialog is up");
        // Bare Enter declines: Cancel is the preselected row.
        assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
        assert!(!p.confirming(), "Cancel closed the dialog");
        assert_eq!(
            p.menu.items[p.menu.selected], "Restart",
            "the cursor returns to the armed row"
        );
        // Armed again, a deliberate second motion confirms.
        drive(&mut p, Key::Enter);
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Restart);
        // Quit confirms the same way.
        let mut p = PauseScreen::open(false);
        activate(&mut p, "Quit");
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Quit);
    }

    #[test]
    fn escape_resumes_from_the_menu_but_only_cancels_the_dialog() {
        let mut p = PauseScreen::open(false);
        assert_eq!(drive(&mut p, Key::Escape), Out::Resume);
        let mut p = PauseScreen::open(false);
        activate(&mut p, "Main Menu");
        assert!(p.confirming());
        assert_eq!(
            drive(&mut p, Key::Escape),
            Out::Stay,
            "Escape in the dialog cancels, never resumes past it"
        );
        assert!(!p.confirming());
    }

    #[test]
    fn resume_needs_no_confirmation_and_watch_exists_only_after_the_end() {
        let mut p = PauseScreen::open(true);
        assert_eq!(drive(&mut p, Key::Enter), Out::Resume);
        let mut p = PauseScreen::open(true);
        assert_eq!(activate(&mut p, "Watch Replay"), Out::WatchReplay);
        // Mid-match: no Watch Replay row, and every verb still lands on
        // the right target.
        let mut p = PauseScreen::open(false);
        assert!(
            !p.menu.items.iter().any(|i| i == "Watch Replay"),
            "mid-match playback would be a fog-free scout of the enemy"
        );
        assert_eq!(activate(&mut p, "Restart"), Out::Stay, "Restart arms");
        assert!(p.confirming());
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Restart);
    }

    #[test]
    fn the_save_failure_dialog_preselects_cancel_and_returns_to_the_verb() {
        let mut p = PauseScreen::open_save_failed(
            "could not save: the disk refused the file".to_string(),
            LeaveVerb::Quit,
            false,
            false,
        );
        assert!(p.saving_failed());
        assert!(p.subtitle("map").is_ascii(), "the menu font is Latin-1");
        // Bare Enter declines: Cancel is the preselected row, so a
        // reflexive double-tap never leaves unsaved.
        assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
        assert!(!p.saving_failed(), "Cancel closed the dialog");
        assert_eq!(
            p.menu.items[p.menu.selected], "Quit",
            "the cursor returns to the verb that raised the dialog"
        );
    }

    #[test]
    fn retry_and_leave_unsaved_carry_the_pending_verb() {
        let mut p =
            PauseScreen::open_save_failed("x".to_string(), LeaveVerb::MainMenu, false, false);
        assert_eq!(
            activate(&mut p, "Retry"),
            Out::RetrySave(LeaveVerb::MainMenu)
        );
        assert!(p.saving_failed(), "the dialog waits on the retry's verdict");
        assert_eq!(
            activate(&mut p, "Leave without saving"),
            Out::LeaveUnsaved(LeaveVerb::MainMenu),
            "a full disk can never trap the player"
        );
    }

    #[test]
    fn escape_cancels_the_save_failure_dialog_never_the_leave() {
        let mut p = PauseScreen::open_save_failed("x".to_string(), LeaveVerb::Quit, false, false);
        assert_eq!(drive(&mut p, Key::Escape), Out::Stay);
        assert!(!p.saving_failed());
        // A Home-origin dialog cancels back to the front door instead
        // of a pause menu the player never opened.
        let mut p = PauseScreen::open_save_failed("x".to_string(), LeaveVerb::Quit, false, true);
        assert_eq!(drive(&mut p, Key::Escape), Out::Home);
    }

    #[test]
    fn settings_opens_without_confirmation_on_both_faces() {
        // Settings destroys nothing — it must never arm the dialog.
        for finished in [false, true] {
            let mut p = PauseScreen::open(finished);
            assert_eq!(activate(&mut p, "Settings"), Out::Settings);
            assert!(!p.confirming(), "Settings is not a destructive row");
        }
    }
}
