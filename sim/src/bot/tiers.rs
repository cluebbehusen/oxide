//! The difficulty ladder: four tiers, zero cheats — including vision.
//!
//! Every tier plays under exactly the rules a human gets: no income,
//! vision, or combat multipliers anywhere, and since Connor's 0.7
//! all-honest ruling, **no omniscience either** — every tier observes
//! through its own fog of war and scouts to know anything at all.
//! Difficulty is purely *considerations*: how often the bot thinks,
//! how deep its economy runs, which combat habits its executive
//! practices. (Mixing honest and omniscient rungs was measured and
//! rejected: an omniscient bot times every push off true totals and
//! beats its equally-skilled honest twin 17-3, so the two kinds can't
//! be ordered on one ladder. The classic 0.6 bot stays in-tree as the
//! omniscient benchmark the honest ladder is gated against.)

use super::executive::Doctrine;
use super::utility::Dials;
use serde::{Deserialize, Serialize};

/// One rung of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Difficulty {
    /// Slow thinker, small ambitions: a light economy, small unfocused
    /// armies, no tech, no reaction to raids. The tutorial opponent.
    Scrapheap,
    /// The full economy and the army lifecycle, at a measured cadence —
    /// armies mass and march together, but fight without finesse.
    Standard,
    /// Everything Standard does, twice as fast, plus the combat habits
    /// that win even fights: focus fire and rotating the wounded out.
    Veteran,
    /// Veteran thinking faster on a deeper economy with heavier
    /// commitments — the ceiling of the scripted ladder. (Working name;
    /// the learned policy takes this rung if it earns promotion.)
    Prime,
}

impl Difficulty {
    /// All tiers, weakest first.
    pub const LADDER: [Difficulty; 4] = [
        Difficulty::Scrapheap,
        Difficulty::Standard,
        Difficulty::Veteran,
        Difficulty::Prime,
    ];

    /// The policy dials this tier thinks with. All fog-honest; all
    /// scouting (a blind commander that never scouts knows nothing).
    pub fn dials(self) -> Dials {
        match self {
            Difficulty::Scrapheap => Dials {
                cadence: 32,
                harvester_target: 3,
                army_size: 4,
                tech: false,
                turret_response: false,
                ..Dials::full()
            },
            Difficulty::Standard => Dials {
                cadence: 24,
                turret_response: false,
                ..Dials::full()
            },
            Difficulty::Veteran => Dials::full(),
            Difficulty::Prime => Dials {
                cadence: 4,
                harvester_target: 6,
                army_size: 6,
                ..Dials::full()
            },
        }
    }

    /// The combat habits this tier's executive practices.
    pub fn doctrine(self) -> Doctrine {
        match self {
            Difficulty::Scrapheap | Difficulty::Standard => Doctrine {
                focus_fire: false,
                pullback: false,
            },
            Difficulty::Veteran | Difficulty::Prime => Doctrine::default(),
        }
    }
}
