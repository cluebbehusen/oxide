//! Learn-by-doing onboarding: six steps, each advancing when the
//! player actually demonstrates the action — never on a timer, never
//! on "next". The card watches the same command stream the sim
//! records, so a keyboard purist and a card-clicker graduate the same
//! way. Dismissible at any time; re-entry is just starting another
//! tutorial match from Home.

/// What the player has demonstrably done this session (flags set by
/// `Game::do_tick` as accepted commands pass the recorder).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Demo {
    /// Trained anything at a building.
    pub trained: bool,
    /// Trained a machine that fights.
    pub trained_fighter: bool,
    /// Sent a harvester to a node.
    pub harvested: bool,
    /// A harvest load actually reached the bank — the mining lesson's
    /// evidence. An accepted order that never pays proves nothing.
    pub deposited: bool,
    /// Placed a construction site.
    pub built: bool,
    /// Issued a default advance.
    pub advanced: bool,
    /// Opened the pause menu.
    pub paused_menu: bool,
}

/// One tutorial card.
pub struct Step {
    /// Card headline.
    pub title: &'static str,
    /// Body lines for mouse and keyboard.
    desktop: &'static [&'static str],
    /// Body lines for a touch-only build, which has no keys, right
    /// button, or Shift.
    touch: &'static [&'static str],
}

impl Step {
    /// The body lines this build's player can follow.
    pub fn body(&self, touch_only: bool) -> &'static [&'static str] {
        if touch_only { self.touch } else { self.desktop }
    }
}

/// The six demonstrations, in teaching order.
pub const STEPS: [Step; 6] = [
    Step {
        title: "Train a Harvester",
        desktop: &[
            "Click your Foundry, then the Harvester card (or press {train}).",
            "Harvesters are your economy: they haul scrap, build, and weld.",
        ],
        touch: &[
            "Tap your Foundry, then the Harvester card.",
            "Harvesters are your economy: they haul scrap, build, and weld.",
        ],
    },
    Step {
        title: "Gather scrap",
        desktop: &[
            "Select a Harvester and right-click a scrap pile.",
            "Wait for its first load to reach your Foundry.",
            "The red IDLE count shows available Harvesters; press {idle} to select one.",
        ],
        touch: &[
            "Select a Harvester and long-press a scrap pile.",
            "Wait for its first load to reach your Foundry.",
            "The red IDLE count shows available Harvesters; tap it to select one.",
        ],
    },
    Step {
        title: "Build a structure",
        desktop: &[
            "Select a second Harvester and leave the first one mining.",
            "Open construction ({build}), choose a category and building,",
            "then click open ground. Red tint means you can't build there.",
            "Hold Shift to chain: keep placing, and each build queues up.",
        ],
        touch: &[
            "Select a second Harvester and leave the first one mining.",
            "Tap Build, then a building, then open ground to place a ghost.",
            "Tap the ghost to build it. Red tint means you can't build there.",
        ],
    },
    Step {
        title: "Train a combat unit",
        desktop: &[
            "Train a Sentinel at the Foundry.",
            "Build a Fabricator to unlock advanced units and aircraft.",
            "Select any visible unit to see its damage, range, and valid targets.",
        ],
        touch: &[
            "Train a Sentinel at the Foundry.",
            "Build a Fabricator to unlock advanced units and aircraft.",
            "Select any visible unit to see its damage, range, and valid targets.",
        ],
    },
    Step {
        title: "Advance under fire",
        desktop: &[
            "Right-click ground with a combat unit selected.",
            "Units keep moving and fire at enemies already in range.",
            "Press {attack} for attack-move when you want them to stop and chase.",
        ],
        touch: &[
            "Long-press ground with a combat unit selected.",
            "Units keep moving and fire at enemies already in range.",
            "Tap Attack-move first when you want them to stop and chase.",
        ],
    },
    Step {
        title: "Win the match",
        desktop: &[
            "Press {back} to cancel, deselect, then open the pause menu.",
            "Destroy all enemy Foundries to win.",
        ],
        touch: &[
            "Tap the menu button at the top right to open the pause menu.",
            "Destroy all enemy Foundries to win.",
        ],
    },
];

/// The tutorial's match: the embedded skirmish with the same scripted
/// opponent as an ordinary match and a raised opening bank. The
/// authored 150 ran dry after the lesson's prepaid spends and left
/// the fighter lesson unpayable at zero income. The raise is
/// tutorial-only, so the scenario file and its fixtures stay unchanged.
/// The playthrough test in `input::tests` pins the arithmetic.
pub fn tutorial_scenario() -> oxide_sim::Scenario {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[0].scrap = 260;
    for p in scenario.players.iter_mut().skip(1) {
        p.bot_config = Some(oxide_sim::scenario::BotConfig::default());
    }
    scenario
}

/// One line of live coaching drawn under the lesson body.
pub enum CoachLine {
    /// The lesson's price against the live bank and hauling count.
    Status(String),
    /// The out-of-scrap escape hatch: the lesson is unaffordable and
    /// nothing is mining, so the coach points at an idle harvester.
    Recovery(String),
}

impl CoachLine {
    /// The line as drawn.
    pub fn text(&self) -> &str {
        match self {
            CoachLine::Status(s) | CoachLine::Recovery(s) => s,
        }
    }
}

/// Live tutorial state.
pub struct Tutorial {
    /// Index into [`STEPS`].
    pub step: usize,
}

impl Tutorial {
    /// Starts at the first lesson.
    pub fn new() -> Self {
        Self { step: 0 }
    }

    /// Advances past every step the player has already demonstrated.
    /// Returns true while the tutorial still has cards to show.
    pub fn advance(&mut self, demo: &Demo) -> bool {
        loop {
            let done = match self.step {
                0 => demo.trained,
                1 => demo.deposited,
                2 => demo.built,
                3 => demo.trained_fighter,
                4 => demo.advanced,
                5 => demo.paused_menu,
                _ => return false,
            };
            if !done {
                return true;
            }
            self.step += 1;
        }
    }

    /// The prepaid scrap the current lesson's literal instruction
    /// spends: the trained kinds by their stats, the building lesson
    /// by the palette's first structure (the digit its text steers
    /// toward).
    fn required_spend(&self) -> Option<u32> {
        match self.step {
            0 => Some(oxide_sim::UnitKind::Harvester.stats().cost),
            2 => oxide_sim::BuildingKind::Turret
                .base_stats()
                .construction
                .map(|c| c.cost),
            3 => Some(oxide_sim::UnitKind::Sentinel.stats().cost),
            _ => None,
        }
    }

    /// Whether the card carries a coach line this step — pure over the
    /// step index, so the card rect (shared by drawing and input
    /// hit-testing) sizes itself without live game state.
    pub fn coach_active(&self) -> bool {
        self.required_spend().is_some()
    }

    /// The card's economy line: what the lesson costs, what the bank
    /// holds, who is hauling. When the lesson is unaffordable and no
    /// own harvester is mining, it becomes the recovery nudge instead
    /// — the tutorial must never teach into a dead end it won't name
    /// the exit of.
    pub fn coach(&self, game: &crate::game::Game) -> Option<CoachLine> {
        let cost = self.required_spend()?;
        let bank = game.state.player(game.presentation.human).scrap;
        let hauling = game
            .state
            .units()
            .iter()
            .filter(|u| {
                u.player == game.presentation.human
                    && matches!(u.order, oxide_sim::Order::Harvest { .. })
            })
            .count();
        if bank < cost && hauling == 0 {
            return Some(CoachLine::Recovery(
                recovery_line(crate::platform::TOUCH_ONLY).to_string(),
            ));
        }
        Some(CoachLine::Status(format!(
            "next: {cost} scrap | you have {bank} | {hauling} hauling"
        )))
    }
}

/// The out-of-scrap nudge, in the order gesture this build has.
fn recovery_line(touch_only: bool) -> &'static str {
    if touch_only {
        "Out of scrap: use the idle badge to grab a harvester, then long-press a scrap pile."
    } else {
        "Out of scrap: use the idle badge to grab a harvester, then right-click a scrap pile."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_lesson_waits_for_its_demonstration() {
        let mut t = Tutorial::new();
        let mut demo = Demo::default();
        assert!(t.advance(&demo), "school is in session");
        assert_eq!(t.step, 0);
        demo.trained = true;
        assert!(t.advance(&demo));
        assert_eq!(t.step, 1, "training graduates lesson one only");
        demo.harvested = true;
        assert!(t.advance(&demo));
        assert_eq!(t.step, 1, "an accepted order alone is not income");
        demo.deposited = true;
        demo.built = true;
        assert!(t.advance(&demo));
        assert_eq!(t.step, 3, "already-demonstrated steps skip in one pass");
    }

    #[test]
    fn a_prodigy_graduates_immediately() {
        let mut t = Tutorial::new();
        let demo = Demo {
            trained: true,
            trained_fighter: true,
            harvested: true,
            deposited: true,
            built: true,
            advanced: true,
            paused_menu: true,
        };
        assert!(!t.advance(&demo), "nothing left to teach");
    }

    #[test]
    fn the_coach_prices_the_lesson_and_names_the_exit_when_broke() {
        let game = crate::game::Game::with_viewport(
            tutorial_scenario(),
            macroquad::prelude::vec2(1280.0, 800.0),
        )
        .expect("the tutorial scenario builds");
        let t = Tutorial::new();
        let line = t.coach(&game).expect("the training lesson has a price");
        match line {
            CoachLine::Status(s) => {
                assert!(s.contains("50 scrap"), "names the harvester's price: {s}");
                assert!(s.contains("260"), "names the live bank: {s}");
            }
            CoachLine::Recovery(_) => panic!("a funded bank needs no rescue"),
        }

        let mut broke = tutorial_scenario();
        broke.players[0].scrap = 0;
        let game = crate::game::Game::with_viewport(broke, macroquad::prelude::vec2(1280.0, 800.0))
            .expect("the broke variant builds");
        let t = Tutorial { step: 3 };
        match t.coach(&game).expect("the fighter lesson has a price") {
            CoachLine::Recovery(s) => {
                assert!(
                    s.contains("idle badge"),
                    "offers to select an idle harvester: {s}"
                );
            }
            CoachLine::Status(s) => panic!("zero bank, zero income must nudge, got: {s}"),
        }
    }

    #[test]
    fn every_card_string_is_ascii() {
        // The bundled font cannot be trusted to cover typographic punctuation.
        for step in &STEPS {
            for touch_only in [false, true] {
                for line in std::iter::once(&step.title).chain(step.body(touch_only)) {
                    assert!(line.is_ascii(), "non-ASCII in card text: {line}");
                }
            }
        }
    }

    #[test]
    fn touch_lessons_name_no_keys_or_mouse_buttons() {
        for step in &STEPS {
            for line in step.body(true) {
                crate::platform::assert_touch_copy(line);
            }
        }
        crate::platform::assert_touch_copy(recovery_line(true));
    }
}
