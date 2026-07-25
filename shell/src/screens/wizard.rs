//! The New Match wizard: map, then either the 1v1 quick flow
//! (difficulty, personality, faction) or — on team maps — the match
//! setup screen with per-seat dials and seat picking. One screen object
//! owning its step, its menus, and the draft they edit.
//!
//! Two front-ends, one back: whatever the flow, every answer lands in
//! the draft's PER-SEAT vector, and `launch()` reads only that.

use crate::game::SoundKind;
use crate::menu::{Menu, ScenarioEntry, discover_scenarios};
use anyhow::{Context, Result};
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};
use oxide_sim::Scenario;
use std::path::PathBuf;

/// One AI seat's dials in the draft.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeatPlan {
    /// Difficulty row (indexes `Level::LADDER`).
    pub level_choice: usize,
    /// Personality row (feeds [`personality_knob`]).
    pub personality_choice: usize,
}

impl Default for SeatPlan {
    fn default() -> Self {
        Self {
            level_choice: 1, // Medium is the fair default
            personality_choice: 0,
        }
    }
}

/// Everything New Match has chosen so far. The draft outlives every
/// screen transition: backing from any step to the map list and
/// forward again re-offers each earlier answer instead of forgetting
/// it.
#[derive(Default)]
pub struct NewMatchDraft {
    /// The loaded map, once picked.
    pub scenario: Option<Box<Scenario>>,
    /// The picked map's path (`None` = the embedded skirmish) — keyed
    /// by PATH so the browser's section sort can never move the
    /// remembered highlight onto a different map.
    pub scenario_path: Option<PathBuf>,
    /// Which chair the human takes (index into the scenario's players).
    pub seat_choice: usize,
    /// One plan per seat, aligned with the scenario's player list and
    /// re-derived (and clamped) whenever the scenario is assigned —
    /// Back from an 8-seat map to a 2-seat map must not leave a stale
    /// seat 7. The human's own row is inert at launch.
    pub seats: Vec<SeatPlan>,
    /// Faction row for the 1v1 quick flow (Ferrous / Cupric /
    /// surprise). Team maps have no faction question: picking a seat
    /// IS picking its authored faction.
    pub faction_choice: usize,
}

impl NewMatchDraft {
    /// Installs a picked map: the per-seat vector re-derives at the new
    /// map's width (existing dials survive where seats overlap) and the
    /// seat choice clamps onto the board.
    pub fn set_scenario(&mut self, scenario: Scenario, path: Option<PathBuf>) {
        let count = scenario.players.len();
        self.seats.resize(count, SeatPlan::default());
        self.seats.truncate(count);
        self.seat_choice = self.seat_choice.min(count.saturating_sub(1));
        self.scenario = Some(Box::new(scenario));
        self.scenario_path = path;
    }

    /// Whether the picked map runs the per-seat setup screen (anything
    /// beyond a duel) instead of the 1v1 quick flow.
    pub fn team_map(&self) -> bool {
        self.scenario.as_ref().is_some_and(|s| s.players.len() > 2)
    }
}

const DIFFICULTY_ITEMS: [&str; 4] = ["Easy", "Medium", "Hard", "Expert"];
const PERSONALITY_ITEMS: [&str; 4] = ["Surprise me", "Turtle", "Balanced", "Aggressive"];
const FACTION_ITEMS: [&str; 3] = ["Ferrous", "Cupric", "Surprise me"];

/// The personality dial a wizard row means; `None` lets the scenario
/// seed deal one.
pub fn personality_knob(choice: usize) -> Option<u32> {
    match choice {
        1 => Some(100), // Turtle
        2 => Some(500), // Balanced
        3 => Some(900), // Aggressive
        _ => None,      // Surprise me
    }
}

/// Which wizard question is on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// Scenario picker (the map list).
    Map,
    /// Difficulty picker (1v1 quick flow).
    Difficulty,
    /// Personality picker (1v1 quick flow).
    Personality,
    /// Faction picker — the quick flow's last question; answering
    /// launches.
    Faction,
    /// Team-map match setup: one row per seat, Start, Back.
    Setup,
    /// One seat's dials (take the chair, difficulty, personality).
    SeatDetail(usize),
}

/// What a wizard frame decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Still asking.
    Stay,
    /// Backed all the way out to the front door.
    Home,
    /// Every question answered: the caller launches from the draft.
    Launch,
}

/// The wizard screen: current step, its live menu, and (on the map
/// step) the discovered scenario entries the menu rows mirror.
pub struct Wizard {
    /// Which question is up.
    pub step: Step,
    /// The step's menu (rows + selection + scroll state).
    pub menu: Menu,
    /// Scenario entries behind the map list's rows.
    pub entries: Vec<ScenarioEntry>,
    /// Menu row -> entry index; `None` rows are section headers and
    /// the trailing Back.
    pub row_entries: Vec<Option<usize>>,
}

/// The browser's section heading for a seat count.
fn format_heading(seats: usize) -> String {
    match seats {
        2 => "- 1v1 -".to_string(),
        n if n % 2 == 0 => format!("- {}v{} -", n / 2, n / 2),
        n => format!("- {n} seats -"),
    }
}

fn map_menu(draft: &NewMatchDraft) -> (Menu, Vec<ScenarioEntry>, Vec<Option<usize>>) {
    let entries = discover_scenarios();
    // Rows interleave section headings with the sorted entries: the
    // grouping is VISIBLE, not just an ordering.
    let mut items: Vec<String> = Vec::new();
    let mut row_entries: Vec<Option<usize>> = Vec::new();
    let mut headers: Vec<usize> = Vec::new();
    let mut last_seats = 0;
    for (i, entry) in entries.iter().enumerate() {
        if entry.seats != last_seats {
            headers.push(items.len());
            row_entries.push(None);
            items.push(format_heading(entry.seats));
            last_seats = entry.seats;
        }
        row_entries.push(Some(i));
        items.push(entry.label.clone());
    }
    row_entries.push(None); // Back
    items.push("Back".to_string());
    let mut menu = Menu::with_headers("OXIDE", items, headers);
    // The remembered pick is a PATH: find it wherever the section sort
    // put it this time (a vanished file just lands on the top row).
    let remembered = entries
        .iter()
        .position(|e| e.path == draft.scenario_path)
        .filter(|_| draft.scenario.is_some())
        .and_then(|e| row_entries.iter().position(|r| *r == Some(e)));
    if let Some(row) = remembered {
        menu.select(row);
    }
    (menu, entries, row_entries)
}

/// The plan the quick flow's preselects mirror: the first AI seat's
/// (the human's own row is inert and never written).
fn ai_plan(draft: &NewMatchDraft) -> SeatPlan {
    draft
        .seats
        .iter()
        .enumerate()
        .find(|(i, _)| *i != draft.seat_choice)
        .map(|(_, p)| *p)
        .unwrap_or_default()
}

fn rows_menu(title: &str, items: &[&str], selected: usize) -> Menu {
    let mut rows: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    rows.push("Back".to_string());
    let mut menu = Menu::new(title, rows);
    menu.select(selected.min(items.len() - 1));
    menu
}

fn seat_label(draft: &NewMatchDraft, i: usize) -> String {
    // Latin-1 separators only: the menu font stops there, and an em
    // dash draws as tofu (the 0.11 setup-screen lesson).
    let scenario = draft.scenario.as_ref().expect("setup wants a map");
    let spec = &scenario.players[i];
    if i == draft.seat_choice {
        format!("{}. {} (you)", i + 1, spec.name)
    } else {
        let plan = draft.seats[i];
        format!(
            "{}. {} · {} · {}",
            i + 1,
            spec.name,
            DIFFICULTY_ITEMS[plan.level_choice],
            PERSONALITY_ITEMS[plan.personality_choice]
        )
    }
}

fn setup_menu(draft: &NewMatchDraft) -> Menu {
    let scenario = draft.scenario.as_ref().expect("setup wants a map");
    let mut rows: Vec<String> = (0..scenario.players.len())
        .map(|i| seat_label(draft, i))
        .collect();
    rows.push("Start match".to_string());
    rows.push("Back".to_string());
    let mut menu = Menu::new("MATCH SETUP", rows);
    menu.select(scenario.players.len()); // Start preselected
    menu
}

fn seat_detail_menu(draft: &NewMatchDraft, i: usize) -> Menu {
    let plan = draft.seats[i];
    let rows = vec![
        "Take this seat".to_string(),
        format!("Difficulty: {}", DIFFICULTY_ITEMS[plan.level_choice]),
        format!(
            "Personality: {}",
            PERSONALITY_ITEMS[plan.personality_choice]
        ),
        "Back".to_string(),
    ];
    let scenario = draft.scenario.as_ref().expect("setup wants a map");
    Menu::new(scenario.players[i].name.to_uppercase(), rows)
}

impl Wizard {
    /// Opens at the map list, every row preselected from the draft.
    pub fn open(draft: &NewMatchDraft) -> Self {
        let (menu, entries, row_entries) = map_menu(draft);
        Self {
            step: Step::Map,
            menu,
            entries,
            row_entries,
        }
    }

    /// The scenario entry a map-list row means, with its stable entry
    /// index (headers and Back mean nothing).
    pub fn entry_at(&self, row: usize) -> Option<(usize, &ScenarioEntry)> {
        let index = self.row_entries.get(row).copied().flatten()?;
        self.entries.get(index).map(|e| (index, e))
    }

    fn goto(&mut self, step: Step, draft: &NewMatchDraft) {
        self.step = step;
        self.menu = match step {
            Step::Map => {
                let (menu, entries, row_entries) = map_menu(draft);
                self.entries = entries;
                self.row_entries = row_entries;
                menu
            }
            Step::Difficulty => {
                rows_menu("DIFFICULTY", &DIFFICULTY_ITEMS, ai_plan(draft).level_choice)
            }
            Step::Personality => rows_menu(
                "OPPONENT",
                &PERSONALITY_ITEMS,
                ai_plan(draft).personality_choice,
            ),
            Step::Faction => rows_menu("FACTION", &FACTION_ITEMS, draft.faction_choice),
            Step::Setup => setup_menu(draft),
            Step::SeatDetail(i) => seat_detail_menu(draft, i),
        };
    }

    /// Writes a 1v1 quick-flow answer through to every AI seat: the two
    /// front-ends share one back — `launch()` reads only the vector.
    fn write_all_seats(draft: &mut NewMatchDraft, write: impl Fn(&mut SeatPlan)) {
        for (i, plan) in draft.seats.iter_mut().enumerate() {
            if i != draft.seat_choice {
                write(plan);
            }
        }
    }

    /// Applies a frame's events. Windowless: menus navigate, the draft
    /// records answers, and the return says whether the caller should
    /// stay, go Home, or launch the match.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        draft: &mut NewMatchDraft,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Result<Out> {
        let escaped = events
            .iter()
            .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
        let choice = self.menu.handle(events, mouse);
        if choice.is_some() {
            sounds.push((SoundKind::Click, None));
        }
        match self.step {
            Step::Map => {
                if escaped {
                    return Ok(Out::Home);
                }
                if let Some(c) = choice {
                    // Headers can't activate, so a row meaning no entry
                    // is the appended Back: return to the front door.
                    let Some(entry) = self.row_entries.get(c).copied().flatten() else {
                        return Ok(Out::Home);
                    };
                    let scenario = match &self.entries[entry].path {
                        Some(path) => Scenario::load(path)
                            .with_context(|| format!("loading {}", path.display()))?,
                        None => Scenario::skirmish(),
                    };
                    draft.set_scenario(scenario, self.entries[entry].path.clone());
                    if draft.team_map() {
                        self.goto(Step::Setup, draft);
                    } else {
                        self.goto(Step::Difficulty, draft);
                    }
                }
            }
            Step::Difficulty => {
                if escaped || choice.is_some_and(|c| c >= DIFFICULTY_ITEMS.len()) {
                    self.goto(Step::Map, draft);
                } else if let Some(c) = choice {
                    Self::write_all_seats(draft, |p| p.level_choice = c);
                    self.goto(Step::Personality, draft);
                }
            }
            Step::Personality => {
                if escaped || choice.is_some_and(|c| c >= PERSONALITY_ITEMS.len()) {
                    self.goto(Step::Difficulty, draft);
                } else if let Some(c) = choice {
                    Self::write_all_seats(draft, |p| p.personality_choice = c);
                    self.goto(Step::Faction, draft);
                }
            }
            Step::Faction => {
                if escaped || choice.is_some_and(|c| c >= FACTION_ITEMS.len()) {
                    self.goto(Step::Personality, draft);
                } else if let Some(c) = choice {
                    draft.faction_choice = c;
                    return Ok(Out::Launch);
                }
            }
            Step::Setup => {
                let seat_rows = draft.seats.len();
                if escaped || choice == Some(seat_rows + 1) {
                    self.goto(Step::Map, draft);
                } else if choice == Some(seat_rows) {
                    return Ok(Out::Launch);
                } else if let Some(c) = choice {
                    self.goto(Step::SeatDetail(c), draft);
                }
            }
            Step::SeatDetail(i) => {
                if escaped || choice == Some(3) {
                    self.goto(Step::Setup, draft);
                } else if let Some(c) = choice {
                    match c {
                        0 => {
                            // Seat choice never permutes seats — parity
                            // carries factions and teams — it moves the
                            // human's chair.
                            draft.seat_choice = i;
                            self.goto(Step::Setup, draft);
                        }
                        1 => {
                            let plan = &mut draft.seats[i];
                            plan.level_choice = (plan.level_choice + 1) % DIFFICULTY_ITEMS.len();
                            self.goto(Step::SeatDetail(i), draft);
                            self.menu.select(1);
                        }
                        2 => {
                            let plan = &mut draft.seats[i];
                            plan.personality_choice =
                                (plan.personality_choice + 1) % PERSONALITY_ITEMS.len();
                            self.goto(Step::SeatDetail(i), draft);
                            self.menu.select(2);
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(Out::Stay)
    }

    /// The debug protocol's stable mode name for the current step —
    /// unchanged from the pre-extraction Mode variants, so automation
    /// scripts keep their footing.
    pub fn mode_name(&self) -> &'static str {
        match self.step {
            Step::Map => "main_menu",
            Step::Difficulty => "difficulty_menu",
            Step::Personality => "personality_menu",
            Step::Faction => "faction_menu",
            Step::Setup => "match_setup",
            Step::SeatDetail(_) => "seat_menu",
        }
    }

    /// The subtitle under the step's menu. The map step ignores this
    /// (its subtitle browses the highlighted entry's blurb).
    pub fn subtitle(&self, _draft: &NewMatchDraft) -> &'static str {
        match self.step {
            Step::Map => "machines eating a dead world",
            Step::Difficulty => "how hard should it think?",
            Step::Personality => "how should it fight?",
            Step::Faction => "which roster do your machines run?",
            Step::Setup => "pick your seat; tune each opponent",
            Step::SeatDetail(_) => "this seat's dials",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn press(key: Key) -> Vec<RawEvent> {
        vec![RawEvent::KeyDown { key }, RawEvent::KeyUp { key }]
    }

    fn drive(w: &mut Wizard, draft: &mut NewMatchDraft, key: Key) -> Out {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        w.update(&press(key), &mut mouse, draft, &mut sounds)
            .expect("update")
    }

    /// The first 1v1 entry's menu row (sections put duels first, so
    /// row 0 is always a duel — the property its own test pins).
    fn pick_first_map(w: &mut Wizard, draft: &mut NewMatchDraft) {
        w.menu.select(0);
        assert_eq!(drive(w, draft, Key::Enter), Out::Stay);
    }

    #[test]
    fn the_wizard_walks_forward_and_back_without_forgetting() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        assert_eq!(w.step, Step::Map);

        pick_first_map(&mut w, &mut draft);
        assert_eq!(w.step, Step::Difficulty, "row 0 is a duel: quick flow");
        assert!(draft.scenario.is_some(), "the draft holds the map");
        assert_eq!(w.menu.selected, 1, "Medium preselected from the draft");

        // Choose Hard, then back out — the answer must survive (it
        // lives in the per-seat vector now).
        drive(&mut w, &mut draft, Key::Down);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Personality);
        assert!(
            draft
                .seats
                .iter()
                .enumerate()
                .all(|(i, p)| i == draft.seat_choice || p.level_choice == 2),
            "the quick flow writes every AI seat's plan"
        );
        assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Stay);
        assert_eq!(w.step, Step::Difficulty);
        assert_eq!(w.menu.selected, 2, "Back re-offers Hard, not the default");

        // Forward to the end: the last answer launches.
        drive(&mut w, &mut draft, Key::Enter);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(w.step, Step::Faction);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Launch);
        assert_eq!(draft.faction_choice, 0);
    }

    #[test]
    fn the_back_row_and_escape_both_unwind_to_home() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Home);

        let mut w = Wizard::open(&draft);
        // The Back row sits past every entry; End jumps the cursor there.
        let last = w.menu.items.len() - 1;
        w.menu.select(last);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Home);
    }

    #[test]
    fn a_back_row_mid_wizard_steps_one_screen_not_all_the_way_out() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        pick_first_map(&mut w, &mut draft);
        assert_eq!(w.step, Step::Difficulty);
        let last = w.menu.items.len() - 1;
        w.menu.select(last);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Map, "Back walks one step");
        let entry = w
            .entries
            .iter()
            .position(|e| e.path == draft.scenario_path)
            .unwrap();
        let expected_row = w
            .row_entries
            .iter()
            .position(|r| *r == Some(entry))
            .unwrap();
        assert_eq!(
            w.menu.selected, expected_row,
            "the map list re-offers the earlier pick, found by PATH"
        );
    }

    #[test]
    fn the_browser_leads_with_duels() {
        let w = Wizard::open(&NewMatchDraft::default());
        assert!(
            w.entries.first().is_some_and(|e| e.seats == 2),
            "row 0 must be a 1v1: a first Play+Enter never launches a team match"
        );
        let seat_counts: Vec<usize> = w.entries.iter().map(|e| e.seats).collect();
        let mut sorted = seat_counts.clone();
        sorted.sort_unstable();
        assert_eq!(seat_counts, sorted, "sections ascend by format");
    }

    #[test]
    fn a_team_map_runs_the_setup_screen_and_reseats_without_permuting() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        let Some(team_row) = w.entries.iter().position(|e| e.seats > 2) else {
            return; // no team maps discovered (bare checkout): nothing to test
        };
        let seats = w.entries[team_row].seats;
        w.menu.select(team_row);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Setup, "team maps skip the quick flow");
        assert_eq!(draft.seats.len(), seats, "the per-seat vector re-derived");
        assert_eq!(
            w.menu.selected, seats,
            "Start preselected under the seat rows"
        );

        // Open seat 1's dials and take the chair.
        w.menu.select(1);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::SeatDetail(1));
        w.menu.select(0);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seat_choice, 1, "the chair moved");
        assert_eq!(w.step, Step::Setup);

        // Cycle seat 0's difficulty twice: Medium -> Hard -> Expert.
        w.menu.select(0);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(w.step, Step::SeatDetail(0));
        w.menu.select(1);
        drive(&mut w, &mut draft, Key::Enter);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seats[0].level_choice, 3, "the dial cycles in place");

        // Start launches; Back from Setup re-derives cleanly on a
        // smaller map afterward (the stale-seat-7 rule).
        w.menu.select(3);
        drive(&mut w, &mut draft, Key::Enter); // Back to Setup
        assert_eq!(w.step, Step::Setup);
        let start = draft.seats.len();
        w.menu.select(start);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Launch);

        draft.set_scenario(Scenario::skirmish(), None);
        assert_eq!(draft.seats.len(), 2, "re-derived at the new width");
        assert!(draft.seat_choice < 2, "the chair clamped onto the board");
    }
}
