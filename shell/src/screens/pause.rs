//! The pause menu and its destructive-choice confirmation — one screen
//! object. Windowless update; the main loop performs the session verbs
//! (resume, watch, restart, main menu, quit) and draws.

use crate::game::SoundKind;
use crate::menu::Menu;
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};

const PAUSE_ITEMS: [&str; 5] = ["Resume", "Watch Replay", "Restart", "Main Menu", "Quit"];

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
    /// Confirmed: rebuild the match.
    Restart,
    /// Confirmed: abandon to the front door.
    MainMenu,
    /// Confirmed: leave the process.
    Quit,
}

/// The pause screen: its menu, plus the armed destructive row while
/// the confirmation dialog is up.
pub struct PauseScreen {
    /// The live menu (pause rows, or the two-row confirm dialog).
    pub menu: Menu,
    /// Which destructive row is awaiting confirmation, if any
    /// (canonical PAUSE_ITEMS index).
    pub confirming: Option<usize>,
    /// Whether the match is decided — only then does Watch Replay
    /// appear. Mid-match playback is a fog-free scout of the enemy;
    /// replays are an end-of-match affair.
    pub finished: bool,
}

fn confirm_menu(choice: usize) -> Menu {
    let verb = PAUSE_ITEMS.get(choice).copied().unwrap_or("Quit");
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
        let items: Vec<String> = PAUSE_ITEMS
            .iter()
            .filter(|&&label| finished || label != "Watch Replay")
            .map(|s| s.to_string())
            .collect();
        Self {
            menu: Menu::new("PAUSED", items),
            confirming: None,
            finished,
        }
    }

    /// Maps a displayed row back to its canonical PAUSE_ITEMS index —
    /// mid-match menus omit the Watch Replay row.
    fn canonical(&self, row: usize) -> usize {
        if !self.finished && row >= 1 {
            row + 1
        } else {
            row
        }
    }

    /// Whether the confirmation dialog is up (for the mode report).
    pub fn confirming(&self) -> bool {
        self.confirming.is_some()
    }

    /// The subtitle for the current face of the screen.
    pub fn subtitle<'a>(&self, scenario_name: &'a str) -> &'a str {
        if self.confirming.is_some() {
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
        if let Some(choice) = self.confirming {
            if escaped || picked == Some(0) {
                self.confirming = None;
                self.menu = Self::open(self.finished).menu;
                // The cursor returns to the armed row, at its DISPLAYED
                // position.
                self.menu
                    .select(if !self.finished { choice - 1 } else { choice });
                return Out::Stay;
            }
            if picked == Some(1) {
                return match choice {
                    2 => Out::Restart,
                    3 => Out::MainMenu,
                    _ => Out::Quit,
                };
            }
            return Out::Stay;
        }
        if picked.is_some() {
            sounds.push((SoundKind::Click, None));
        }
        match picked.map(|row| self.canonical(row)) {
            Some(0) => Out::Resume,
            Some(1) => Out::WatchReplay,
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

    #[test]
    fn destructive_rows_confirm_with_cancel_preselected() {
        let mut p = PauseScreen::open(false);
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Stay, "Restart only arms");
        assert!(p.confirming(), "the dialog is up");
        // Bare Enter declines: Cancel is the preselected row.
        assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
        assert!(!p.confirming(), "Cancel closed the dialog");
        assert_eq!(p.menu.selected, 1, "the cursor returns to the armed row");
        // Armed again, a deliberate second motion confirms.
        drive(&mut p, Key::Enter);
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Restart);
        // Quit sits at the shifted tail mid-match.
        let mut p = PauseScreen::open(false);
        for _ in 0..3 {
            drive(&mut p, Key::Down);
        }
        drive(&mut p, Key::Enter);
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Quit);
    }

    #[test]
    fn escape_resumes_from_the_menu_but_only_cancels_the_dialog() {
        let mut p = PauseScreen::open(false);
        assert_eq!(drive(&mut p, Key::Escape), Out::Resume);
        let mut p = PauseScreen::open(false);
        for _ in 0..3 {
            drive(&mut p, Key::Down);
        }
        drive(&mut p, Key::Enter);
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
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::WatchReplay);
        // Mid-match: no Watch Replay row, and every verb still lands on
        // the right target through the shifted indices.
        let mut p = PauseScreen::open(false);
        assert!(
            !p.menu.items.iter().any(|i| i == "Watch Replay"),
            "mid-match playback would be a fog-free scout of the enemy"
        );
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Stay, "Restart arms");
        assert!(p.confirming());
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Restart);
    }
}
