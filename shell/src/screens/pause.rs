//! The pause menu and its confirmation dialogs — one screen object.
//! Windowless update; the main loop performs the session verbs
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
/// Replay belongs to decided matches; Save Game and Surrender to
/// running ones, with Surrender further limited to a seat that still
/// has a voice — a resigned or eliminated spectator is shown no verb
/// the sim would only reject.
fn rows(finished: bool, can_surrender: bool) -> Vec<Row> {
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
    rows.extend([Row::Restart, Row::MainMenu, Row::Quit]);
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
    RetrySave(LeaveVerb),
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
    /// The save-name buffer while the name field has focus. Only here
    /// do Text events mean anything; letters stay semantic everywhere
    /// else.
    naming: Option<String>,
    /// A one-line verdict from the last explicit save (success or
    /// failure), shown as the subtitle until the next activation.
    notice: Option<String>,
    /// Whether the match is decided — only then does Watch Replay
    /// appear. Mid-match playback is a fog-free scout of the enemy;
    /// replays are an end-of-match affair.
    pub finished: bool,
    /// Whether the human's seat can still concede (alive and not
    /// already resigned) — the Surrender row's other gate.
    can_surrender: bool,
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
        let rows = rows(finished, can_surrender);
        let items: Vec<String> = rows.iter().map(|r| r.label().to_string()).collect();
        Self {
            menu: Menu::new("PAUSED", items),
            rows,
            confirming: None,
            save_failed: None,
            naming: None,
            notice: None,
            finished,
            can_surrender,
        }
    }

    /// Longest save name the field accepts — what the shelf row can
    /// show without eliding.
    pub const NAME_MAX: usize = 26;

    /// Opens the name field over the pause menu, prefilled with the
    /// caller's suggestion so Enter-Enter saves without typing (the
    /// Start-preselected doctrine).
    pub fn begin_naming(&mut self, suggested: String) {
        // The field itself only ever grows typed ASCII; the suggestion
        // holds to the same ASCII UI alphabet
        // so a map name cannot smuggle glyphs past the ingest filter.
        let mut value: String = suggested.chars().take(Self::NAME_MAX).collect();
        value.retain(|c| c.is_ascii() || c == '\u{b7}');
        self.menu = Self::name_menu(&value);
        self.naming = Some(value);
    }

    /// Reports the save verdict and returns to the pause rows, cursor
    /// back on Save Game, the verdict as the subtitle.
    pub fn end_naming(&mut self, notice: String) {
        self.naming = None;
        self.menu = Self::open(self.finished, self.can_surrender).menu;
        let display = self
            .rows
            .iter()
            .position(|&r| r == Row::SaveGame)
            .unwrap_or(0);
        self.menu.select(display);
        self.notice = Some(notice);
    }

    /// The name field's face: one editable row under the SAVE GAME
    /// title. The caret is a static underscore — never a blink, so
    /// reduced motion holds and the shots suite stays deterministic.
    fn name_menu(value: &str) -> Menu {
        Menu::new("SAVE GAME", vec![format!("{value}_")])
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
        can_surrender: bool,
        cancel_home: bool,
    ) -> Self {
        let mut screen = Self::open(finished, can_surrender);
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

    /// Whether the save-name field has focus (for the mode report).
    pub fn naming(&self) -> bool {
        self.naming.is_some()
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
        } else if self.naming.is_some() {
            "type a name | Enter saves | Esc cancels"
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
        if let Some(value) = self.naming.as_mut() {
            // The name field owns the frame: typed characters edit,
            // Backspace deletes, Enter commits, Escape abandons. The
            // menu widget is display only here — its navigation would
            // fight the caret.
            let mut edited = false;
            for event in events {
                match *event {
                    RawEvent::Text { ch } => {
                        if value.chars().count() < Self::NAME_MAX {
                            value.push(ch);
                            edited = true;
                        }
                    }
                    RawEvent::KeyDown {
                        key: Key::Backspace,
                    } => {
                        value.pop();
                        edited = true;
                    }
                    _ => {}
                }
            }
            let committed = events
                .iter()
                .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Enter }));
            if committed {
                let name = value.trim().to_string();
                if name.is_empty() {
                    sounds.push((SoundKind::Denied, None));
                } else {
                    return Out::Save(name);
                }
            }
            if escaped {
                self.naming = None;
                self.menu = Self::open(self.finished, self.can_surrender).menu;
                let display = self
                    .rows
                    .iter()
                    .position(|&r| r == Row::SaveGame)
                    .unwrap_or(0);
                self.menu.select(display);
                return Out::Stay;
            }
            if edited {
                let display = Self::name_menu(value);
                self.menu = display;
            }
            return Out::Stay;
        }
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
            self.menu = Self::open(self.finished, self.can_surrender).menu;
            let display = self.rows.iter().position(|&r| r == row).unwrap_or(0);
            self.menu.select(display);
            return Out::Stay;
        }
        if let Some(row) = self.confirming {
            if escaped || picked == Some(0) {
                self.confirming = None;
                self.menu = Self::open(self.finished, self.can_surrender).menu;
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
    fn consequential_rows_confirm_with_cancel_preselected() {
        let mut p = PauseScreen::open(false, true);
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
        let mut p = PauseScreen::open(false, true);
        activate(&mut p, "Quit");
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Quit);
    }

    #[test]
    fn confirmation_copy_names_the_real_consequence() {
        for (label, consequence) in [
            ("Restart", "progress is discarded and the match starts over"),
            ("Main Menu", "the match is saved before returning home"),
            ("Quit", "the match is saved before quitting"),
        ] {
            let mut p = PauseScreen::open(false, true);
            assert_eq!(activate(&mut p, label), Out::Stay, "{label} only arms");
            assert_eq!(p.subtitle("map"), consequence, "{label} consequence");
        }
    }

    #[test]
    fn escape_resumes_from_the_menu_but_only_cancels_the_dialog() {
        let mut p = PauseScreen::open(false, true);
        assert_eq!(drive(&mut p, Key::Escape), Out::Resume);
        let mut p = PauseScreen::open(false, true);
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
        let mut p = PauseScreen::open(true, false);
        assert!(
            !p.menu.items.iter().any(|i| i == "Save Game"),
            "a finished match is a replay, not a resumable named save"
        );
        assert_eq!(drive(&mut p, Key::Enter), Out::Resume);
        let mut p = PauseScreen::open(true, false);
        assert_eq!(activate(&mut p, "Watch Replay"), Out::WatchReplay);
        // Mid-match: no Watch Replay row, and every verb still lands on
        // the right target.
        let mut p = PauseScreen::open(false, true);
        assert!(
            !p.menu.items.iter().any(|i| i == "Watch Replay"),
            "mid-match playback would be a fog-free scout of the enemy"
        );
        assert!(
            p.menu.items.iter().any(|i| i == "Save Game"),
            "a running match can be saved by name"
        );
        assert_eq!(activate(&mut p, "Restart"), Out::Stay, "Restart arms");
        assert!(p.confirming());
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Restart);
    }

    #[test]
    fn the_save_failure_dialog_preselects_cancel_and_returns_to_the_verb() {
        let mut p = PauseScreen::open_save_failed(
            "could not save: unable to write the save file".to_string(),
            LeaveVerb::Quit,
            false,
            true,
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
            PauseScreen::open_save_failed("x".to_string(), LeaveVerb::MainMenu, false, true, false);
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
        let mut p =
            PauseScreen::open_save_failed("x".to_string(), LeaveVerb::Quit, false, true, false);
        assert_eq!(drive(&mut p, Key::Escape), Out::Stay);
        assert!(!p.saving_failed());
        // A Home-origin dialog cancels back to the front door instead
        // of a pause menu the player never opened.
        let mut p =
            PauseScreen::open_save_failed("x".to_string(), LeaveVerb::Quit, false, true, true);
        assert_eq!(drive(&mut p, Key::Escape), Out::Home);
    }

    fn type_text(p: &mut PauseScreen, text: &str) {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        let events: Vec<RawEvent> = text.chars().map(|ch| RawEvent::Text { ch }).collect();
        p.update(&events, &mut mouse, &mut sounds);
    }

    #[test]
    fn save_game_never_confirms_and_bare_enter_saves_the_suggestion() {
        let mut p = PauseScreen::open(false, true);
        assert_eq!(activate(&mut p, "Save Game"), Out::SaveGame);
        assert!(!p.confirming(), "saving destroys nothing — no dialog");
        p.begin_naming("skirmish | t100".to_string());
        assert!(p.naming());
        assert_eq!(
            p.menu.items[0], "skirmish | t100_",
            "prefilled, with a static caret"
        );
        // The Start-preselected doctrine: Enter alone commits the
        // suggested name without any typing.
        assert_eq!(
            drive(&mut p, Key::Enter),
            Out::Save("skirmish | t100".to_string())
        );
    }

    #[test]
    fn the_name_field_edits_with_text_and_backspace_and_escape_cancels() {
        let mut p = PauseScreen::open(false, true);
        p.begin_naming(String::new());
        type_text(&mut p, "abc");
        assert_eq!(drive(&mut p, Key::Backspace), Out::Stay);
        assert_eq!(p.menu.items[0], "ab_");
        // Letter KEYS are not text: only Text events edit the buffer,
        // so an injected semantic H cannot type.
        drive(&mut p, Key::H);
        assert_eq!(p.menu.items[0], "ab_");
        assert_eq!(drive(&mut p, Key::Escape), Out::Stay, "Escape abandons");
        assert!(!p.naming());
        assert_eq!(
            p.menu.items[p.menu.selected], "Save Game",
            "the cursor returns to the verb"
        );
        // An empty name refuses to commit instead of writing a blank.
        p.begin_naming(String::new());
        assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
        assert!(p.naming(), "the field waits for a real name");
    }

    #[test]
    fn repeated_backspace_edges_clear_the_name_field_in_one_frame() {
        let mut p = PauseScreen::open(false, true);
        p.begin_naming("oxide".to_string());
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        let repeats = [
            RawEvent::KeyDown {
                key: Key::Backspace,
            },
            RawEvent::KeyDown {
                key: Key::Backspace,
            },
            RawEvent::KeyDown {
                key: Key::Backspace,
            },
            RawEvent::KeyDown {
                key: Key::Backspace,
            },
            RawEvent::KeyDown {
                key: Key::Backspace,
            },
        ];
        assert_eq!(p.update(&repeats, &mut mouse, &mut sounds), Out::Stay);
        assert_eq!(p.menu.items[0], "_");
    }

    #[test]
    fn the_name_field_caps_its_length_and_the_verdict_shows_until_the_next_pick() {
        let mut p = PauseScreen::open(false, true);
        p.begin_naming("x".repeat(40));
        let shown = p.menu.items[0].clone();
        assert_eq!(
            shown.chars().count(),
            PauseScreen::NAME_MAX + 1,
            "cap+caret"
        );
        type_text(&mut p, "y");
        assert_eq!(p.menu.items[0], shown, "a full field refuses more");
        drive(&mut p, Key::Enter);
        p.end_naming("saved: x".to_string());
        assert!(!p.naming());
        assert_eq!(p.subtitle("map"), "saved: x", "the verdict is the subtitle");
        assert_eq!(p.menu.items[p.menu.selected], "Save Game");
        assert_eq!(activate(&mut p, "Resume"), Out::Resume);
        assert_eq!(p.subtitle("map"), "map", "an activation clears the verdict");
    }

    #[test]
    fn surrender_exists_mid_match_only_and_confirms_with_cancel_preselected() {
        let mut p = PauseScreen::open(false, true);
        assert_eq!(activate(&mut p, "Surrender"), Out::Stay, "Surrender arms");
        assert!(p.confirming(), "conceding asks first");
        assert_eq!(
            p.subtitle("map"),
            "this concedes the match",
            "the dialog names the real consequence, not a thrown-away match"
        );
        // Bare Enter declines: Cancel is the preselected row.
        assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
        assert!(!p.confirming(), "Cancel closed the dialog");
        assert_eq!(
            p.menu.items[p.menu.selected], "Surrender",
            "the cursor returns to the armed row"
        );
        // A deliberate second motion concedes.
        drive(&mut p, Key::Enter);
        drive(&mut p, Key::Down);
        assert_eq!(drive(&mut p, Key::Enter), Out::Surrender);
        // A decided match has nothing left to give up.
        let p = PauseScreen::open(true, false);
        assert!(
            !p.menu.items.iter().any(|i| i == "Surrender"),
            "a decided match offers Watch Replay, not concession"
        );
        // A seat with no voice (resigned or eliminated) gets no verb
        // the sim would only reject.
        let p = PauseScreen::open(false, false);
        assert!(
            !p.menu.items.iter().any(|i| i == "Surrender"),
            "a spectating seat cannot concede twice"
        );
    }

    #[test]
    fn settings_opens_without_confirmation_on_both_faces() {
        // Settings destroys nothing — it must never arm the dialog.
        for finished in [false, true] {
            let mut p = PauseScreen::open(finished, true);
            assert_eq!(activate(&mut p, "Settings"), Out::Settings);
            assert!(!p.confirming(), "Settings is not a destructive row");
        }
    }
}
