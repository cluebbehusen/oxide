//! The New Match wizard: map, difficulty, personality, faction — one
//! screen object owning its step, its menus, and the draft they edit.

use crate::game::SoundKind;
use crate::menu::{Menu, ScenarioEntry, discover_scenarios};
use anyhow::{Context, Result};
use macroquad::prelude::Vec2;
use oxide_protocol::{Key, RawEvent};
use oxide_sim::Scenario;

/// Everything New Match has chosen so far. The draft outlives every
/// screen transition: backing from Faction to Difficulty to the map
/// list and forward again re-offers each earlier answer instead of
/// forgetting it.
pub struct NewMatchDraft {
    /// The loaded map, once picked.
    pub scenario: Option<Box<Scenario>>,
    /// Map-list row, for re-preselection.
    pub scenario_choice: usize,
    /// Difficulty row (indexes `Level::LADDER`).
    pub level_choice: usize,
    /// Personality row (feeds [`personality_knob`]).
    pub personality_choice: usize,
    /// Faction row (Ferrous / Cupric / surprise).
    pub faction_choice: usize,
}

impl Default for NewMatchDraft {
    fn default() -> Self {
        Self {
            scenario: None,
            scenario_choice: 0,
            level_choice: 1, // Medium is the fair default
            personality_choice: 0,
            faction_choice: 0,
        }
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
    /// Difficulty picker.
    Difficulty,
    /// Personality picker.
    Personality,
    /// Faction picker — the last question; answering launches.
    Faction,
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
}

fn map_menu(draft: &NewMatchDraft) -> (Menu, Vec<ScenarioEntry>) {
    let entries = discover_scenarios();
    let mut items: Vec<String> = entries.iter().map(|e| e.label.clone()).collect();
    items.push("Back".to_string());
    let mut menu = Menu::new("OXIDE", items);
    menu.select(draft.scenario_choice.min(entries.len()));
    (menu, entries)
}

fn rows_menu(title: &str, items: &[&str], selected: usize) -> Menu {
    let mut rows: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    rows.push("Back".to_string());
    let mut menu = Menu::new(title, rows);
    menu.select(selected.min(items.len() - 1));
    menu
}

impl Wizard {
    /// Opens at the map list, every row preselected from the draft.
    pub fn open(draft: &NewMatchDraft) -> Self {
        let (menu, entries) = map_menu(draft);
        Self {
            step: Step::Map,
            menu,
            entries,
        }
    }

    fn goto(&mut self, step: Step, draft: &NewMatchDraft) {
        self.step = step;
        self.menu = match step {
            Step::Map => {
                let (menu, entries) = map_menu(draft);
                self.entries = entries;
                menu
            }
            Step::Difficulty => rows_menu("DIFFICULTY", &DIFFICULTY_ITEMS, draft.level_choice),
            Step::Personality => {
                rows_menu("OPPONENT", &PERSONALITY_ITEMS, draft.personality_choice)
            }
            Step::Faction => rows_menu("FACTION", &FACTION_ITEMS, draft.faction_choice),
        };
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
                    if c >= self.entries.len() {
                        // The appended Back row returns to the front door.
                        return Ok(Out::Home);
                    }
                    let scenario = match &self.entries[c].path {
                        Some(path) => Scenario::load(path)
                            .with_context(|| format!("loading {}", path.display()))?,
                        None => Scenario::skirmish(),
                    };
                    draft.scenario = Some(Box::new(scenario));
                    draft.scenario_choice = c;
                    self.goto(Step::Difficulty, draft);
                }
            }
            Step::Difficulty => {
                if escaped || choice.is_some_and(|c| c >= DIFFICULTY_ITEMS.len()) {
                    self.goto(Step::Map, draft);
                } else if let Some(c) = choice {
                    draft.level_choice = c;
                    self.goto(Step::Personality, draft);
                }
            }
            Step::Personality => {
                if escaped || choice.is_some_and(|c| c >= PERSONALITY_ITEMS.len()) {
                    self.goto(Step::Difficulty, draft);
                } else if let Some(c) = choice {
                    draft.personality_choice = c;
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
        }
    }

    /// The subtitle under the step's menu. The map step ignores this
    /// (its subtitle browses the highlighted entry's blurb).
    pub fn subtitle(&self, draft: &NewMatchDraft) -> &'static str {
        match self.step {
            Step::Map => "machines eating a dead world",
            Step::Difficulty => {
                // On a team map these dials set EVERY AI seat — the
                // human's ally included; say so instead of surprising.
                let team_map = draft
                    .scenario
                    .as_ref()
                    .is_some_and(|sc| sc.players.len() > 2);
                if team_map {
                    "how hard should they think? (sets every AI seat, your ally too)"
                } else {
                    "how hard should it think?"
                }
            }
            Step::Personality => "how should they fight?",
            Step::Faction => "which roster do your machines run?",
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

    #[test]
    fn the_wizard_walks_forward_and_back_without_forgetting() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        assert_eq!(w.step, Step::Map);

        // Pick the first map (the embedded skirmish rides the list even
        // when no scenarios directory is in reach).
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Difficulty);
        assert!(draft.scenario.is_some(), "the draft holds the map");
        assert_eq!(w.menu.selected, 1, "Medium preselected from the draft");

        // Choose Hard, then back out — the answer must survive.
        drive(&mut w, &mut draft, Key::Down);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Personality);
        assert_eq!(draft.level_choice, 2);
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
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(w.step, Step::Difficulty);
        let last = w.menu.items.len() - 1;
        w.menu.select(last);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Map, "Back walks one step");
        assert_eq!(
            w.menu.selected, draft.scenario_choice,
            "the map list re-offers the earlier pick"
        );
    }
}
