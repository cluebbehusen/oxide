//! Learn-by-doing onboarding: six steps, each advancing when the
//! player actually demonstrates the action — never on a timer, never
//! on "next". The card watches the same command stream the sim
//! records, so a keyboard purist and a card-clicker graduate the same
//! way. Dismissible at any time; re-entry is just starting another
//! tutorial match from Home.

/// What the player has demonstrably done this session (flags set by
/// `Game::do_tick` as accepted commands pass the recorder).
#[derive(Debug, Default, Clone, Copy)]
pub struct Demo {
    /// Trained anything at a building.
    pub trained: bool,
    /// Trained a machine that fights.
    pub trained_fighter: bool,
    /// Sent a harvester to a node.
    pub harvested: bool,
    /// Placed a construction site.
    pub built: bool,
    /// Issued an attack-move.
    pub attack_moved: bool,
    /// Opened the pause menu.
    pub paused_menu: bool,
}

/// One tutorial card.
pub struct Step {
    /// Card headline.
    pub title: &'static str,
    /// Body lines.
    pub body: &'static [&'static str],
}

/// The six demonstrations, in teaching order.
pub const STEPS: [Step; 6] = [
    Step {
        title: "Train a Harvester",
        body: &[
            "Click your Foundry, then the Harvester card (or press H).",
            "Harvesters are your economy: they haul scrap, build, and weld.",
        ],
    },
    Step {
        title: "Put it to work",
        body: &[
            "Select a harvester and right-click a scrap pile.",
            "It will mine and haul home until the node runs dry.",
        ],
    },
    Step {
        title: "Raise a building",
        body: &[
            "Select a harvester, click a structure card (or B, then a digit),",
            "then click open ground. Red tint means the sim will refuse —",
            "try somewhere rocky once and watch it say no.",
        ],
    },
    Step {
        title: "Arm yourself",
        body: &[
            "Train a fighter: a Sentinel from the Foundry holds ground.",
            "The Fabricator (a build card) unlocks the whole roster —",
            "hover any card to see how a machine fights.",
        ],
    },
    Step {
        title: "March with intent",
        body: &[
            "Right-click ground with a fighter selected: that's attack-move.",
            "Your machines engage whatever they meet on the way.",
        ],
    },
    Step {
        title: "The rest is yours",
        body: &[
            "Esc opens the menu (settings, saves, surrender-with-dignity).",
            "Destroy every enemy Foundry to win. Good hunting.",
        ],
    },
];

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
                1 => demo.harvested,
                2 => demo.built,
                3 => demo.trained_fighter,
                4 => demo.attack_moved,
                5 => demo.paused_menu,
                _ => return false,
            };
            if !done {
                return true;
            }
            self.step += 1;
        }
    }
}
