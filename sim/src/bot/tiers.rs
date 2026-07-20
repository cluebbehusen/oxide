//! The difficulty ladder: four tiers, zero cheats.
//!
//! Every tier plays under exactly the rules a human gets — no income,
//! vision, or combat multipliers anywhere on the ladder. Difficulty is
//! *considerations*: how often the bot thinks, how many channels it
//! runs, which combat habits its executive practices. The lower three
//! tiers read the world omnisciently (the classic RTS concession,
//! documented rather than hidden); the top tier gives that up too and
//! plays through its own vision, scouting like a player would.

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
    /// Everything Standard does, faster, plus the combat habits that
    /// win even fights: focus fire and rotating the wounded out.
    Veteran,
    /// Veteran with the cheat removed: fog-honest observation, scouting
    /// to compensate. The fair fight, all the way down.
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

    /// The policy dials this tier thinks with.
    pub fn dials(self) -> Dials {
        match self {
            Difficulty::Scrapheap => Dials {
                cadence: 32,
                harvester_target: 3,
                army_size: 4,
                tech: false,
                turret_response: false,
                scouting: false,
                fog_honest: false,
            },
            Difficulty::Standard => Dials {
                cadence: 16,
                ..Dials::full_omniscient()
            },
            Difficulty::Veteran => Dials::full_omniscient(),
            // Prime gives up the cheat, so it must simply play better:
            // a deeper economy, quicker thinking, heavier commitments.
            // All of it is things a human could also do — considerations,
            // not multipliers.
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
