//! Learn-by-doing onboarding: six steps, each advancing only when the
//! player demonstrates the action. The card watches the same command
//! stream the sim records, so hotkeys and card clicks count alike. It
//! can be dismissed at any time; re-entry starts another tutorial match
//! from Home.

/// What the player has demonstrably done this session (flags set by
/// `Game::do_tick` as accepted commands pass the recorder).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag records an independent tutorial milestone"
)]
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
            "Press {hunt} for Hunt when you want them to stop and chase.",
        ],
        touch: &[
            "Long-press ground with a combat unit selected.",
            "Units keep moving and fire at enemies already in range.",
            "Tap Hunt first when you want them to stop and chase.",
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

/// The tutorial's match: the embedded skirmish with default opponents and
/// a raised opening bank, so the lessons' prepaid spends leave the
/// fighter lesson payable at zero income. The raise is tutorial-only, so
/// the scenario file and its fixtures stay unchanged. The playthrough
/// test in `input::tests` pins the arithmetic.
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
    pub fn advance(&mut self, demo: Demo) -> bool {
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
    /// holds, who is hauling. When the lesson is unaffordable and no own
    /// harvester is mining, it becomes the recovery nudge instead.
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
mod tests;
