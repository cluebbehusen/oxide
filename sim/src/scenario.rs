//! Scenarios: everything needed to start (and therefore reproduce) a match.
//!
//! A scenario is data — JSON with an ASCII map — and doubles as the `setup`
//! half of a replay, so replay files are self-contained: no scenario file has
//! to survive for a replay to reproduce.

use crate::ids::PlayerId;
use crate::map::{Map, MapError};
use crate::state::{Faction, Player, State};
use crate::stats::{BuildingKind, UnitKind};
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A match definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Display name.
    pub name: String,
    /// Master seed: the sim RNG and every bot derive from it.
    pub seed: u64,
    /// The playfield as ASCII rows (see [`crate::map`] for the legend).
    pub map: Vec<String>,
    /// One entry per player; map anchors `1`..`8` must match.
    pub players: Vec<PlayerSpec>,
    /// Starting units.
    #[serde(default)]
    pub units: Vec<UnitSpec>,
}

/// One player's starting conditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSpec {
    /// Display name.
    pub name: String,
    /// Which roster this seat runs (and its sprite tint).
    pub faction: Faction,
    /// Team index; seats sharing one stand and fall together. `None`
    /// puts the seat on its own team (every pre-team scenario is a
    /// free-for-all of one-player teams).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    /// Starting scrap.
    #[serde(default = "default_scrap")]
    pub scrap: u32,
    /// Whether the built-in bot should run this player (the sim itself
    /// ignores this — shells and drivers honor it).
    #[serde(default)]
    pub bot: bool,
    /// How that bot plays: a ladder level and personality. `None` means
    /// the legacy rule-cascade bot — which is also what keeps replays
    /// recorded before bot configs existed reproducing, since the
    /// scenario (and therefore
    /// this config) rides inside every replay. The legacy bot is
    /// team-blind: a seat with a `team` must set a config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_config: Option<BotConfig>,
}

/// A shipped-ladder bot seat: difficulty plus personality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    /// Named difficulty on the neural ladder.
    pub level: crate::bot::Level,
    /// Personality knob 0..=1000 (turtle to aggressive); `None` deals
    /// one from the scenario seed, deterministically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggression: Option<u32>,
}

fn default_scrap() -> u32 {
    100
}

/// A starting unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitSpec {
    /// Owning player index.
    pub player: u8,
    /// Unit type.
    pub kind: UnitKind,
    /// Spawn tile x.
    pub x: i32,
    /// Spawn tile y.
    pub y: i32,
}

/// Errors from loading or building a scenario.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    /// Filesystem failure.
    #[error("scenario io: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed JSON.
    #[error("scenario format: {0}")]
    Format(#[from] serde_json::Error),
    /// Bad map text.
    #[error(transparent)]
    Map(#[from] MapError),
    /// Player count must be 1..=8.
    #[error("scenario needs 1 to 8 players, got {0}")]
    PlayerCount(usize),
    /// A player has no Foundry anchor on the map.
    #[error("no map anchor for player {0}")]
    MissingAnchor(PlayerId),
    /// An anchor digit exceeds the player list.
    #[error("map anchor for player {0} but only {1} players declared")]
    ExtraAnchor(PlayerId, usize),
    /// A Foundry footprint hangs off the map or covers rock/scrap.
    #[error("player {0}'s foundry at {1} does not fit on open ground")]
    BadFootprint(PlayerId, TilePos),
    /// A starting unit is misplaced or mis-owned.
    #[error("starting unit #{0} is invalid (owner in range? tile passable?)")]
    BadUnit(usize),
    /// Two Foundries can't reach each other: the match could never end.
    #[error("players {0} and {1} are sealed apart — no ground route between their foundries")]
    Disconnected(PlayerId, PlayerId),
    /// Every seat on one team: nobody to fight, no way to win.
    #[error("all players share one team — the match could never end")]
    OneTeam,
}

impl Scenario {
    /// Parses a scenario from JSON text.
    pub fn from_json(text: &str) -> Result<Self, ScenarioError> {
        Ok(serde_json::from_str(text)?)
    }

    /// Loads a scenario from a JSON file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ScenarioError> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    /// The built-in two-player map: human as Ferrous, bot as Cupric.
    pub fn skirmish() -> Self {
        Self::from_json(include_str!("../../scenarios/skirmish.json"))
            .expect("embedded skirmish scenario is validated by tests")
    }

    /// Validates the scenario and constructs the initial [`State`].
    ///
    /// Building the same scenario twice yields bit-identical states (a test
    /// enforces this).
    pub fn build(&self) -> Result<State, ScenarioError> {
        if self.players.is_empty() || self.players.len() > 8 {
            return Err(ScenarioError::PlayerCount(self.players.len()));
        }
        let (map, anchors) = Map::parse(&self.map)?;

        if let Some((player, _)) = anchors
            .iter()
            .find(|(p, _)| (p.0 as usize) >= self.players.len())
        {
            return Err(ScenarioError::ExtraAnchor(*player, self.players.len()));
        }
        let players: Vec<Player> = self
            .players
            .iter()
            .enumerate()
            .map(|(index, spec)| Player {
                name: spec.name.clone(),
                faction: spec.faction,
                team: spec.team.unwrap_or(index as u8),
                scrap: spec.scrap,
            })
            .collect();
        if self.players.len() > 1 {
            let first = players[0].team;
            if players.iter().all(|p| p.team == first) {
                return Err(ScenarioError::OneTeam);
            }
        }
        let mut state = State::assemble(map, players, self.seed);

        for index in 0..self.players.len() {
            let player = PlayerId(index as u8);
            let anchor = anchors
                .iter()
                .find(|(p, _)| *p == player)
                .map(|(_, a)| *a)
                .ok_or(ScenarioError::MissingAnchor(player))?;
            let (w, h) = BuildingKind::Foundry.stats().size;
            let footprint_ok = (0..h)
                .flat_map(|dy| (0..w).map(move |dx| anchor.offset(dx, dy)))
                .all(|t| state.passable(t));
            if !footprint_ok {
                return Err(ScenarioError::BadFootprint(player, anchor));
            }
            state.place_building(player, BuildingKind::Foundry, anchor);
        }

        for (index, spec) in self.units.iter().enumerate() {
            let tile = TilePos::new(spec.x, spec.y);
            if (spec.player as usize) >= self.players.len() || !state.passable(tile) {
                return Err(ScenarioError::BadUnit(index));
            }
            state.spawn_unit(PlayerId(spec.player), spec.kind, tile.center());
        }

        // Authoring tripwire: every pair of Foundries must share a ground
        // route, or the victory condition is unreachable by construction.
        // Flood from the first anchor over terrain (scrap mines out, so
        // nodes count as eventually-open; buildings placed above are only
        // the foundries themselves, whose ring must connect anyway).
        if let Some((first, rest)) = anchors.split_first() {
            let map = &self.map;
            let _ = map;
            let mut open = std::collections::VecDeque::new();
            let width = state.map().width();
            let height = state.map().height();
            let idx = |t: TilePos| (t.y * width + t.x) as usize;
            let mut seen = vec![false; (width * height) as usize];
            let walkable = |t: TilePos, state: &State| {
                t.x >= 0
                    && t.y >= 0
                    && t.x < width
                    && t.y < height
                    && state
                        .map()
                        .tile(t)
                        .is_some_and(|tile| tile.terrain == crate::map::Terrain::Ground)
            };
            let seed = first.1;
            seen[idx(seed)] = true;
            open.push_back(seed);
            while let Some(t) = open.pop_front() {
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let n = t.offset(dx, dy);
                    if walkable(n, &state) && !seen[idx(n)] {
                        seen[idx(n)] = true;
                        open.push_back(n);
                    }
                }
            }
            for (player, anchor) in rest {
                if !seen[idx(*anchor)] {
                    return Err(ScenarioError::Disconnected(first.0, *player));
                }
            }
        }
        state.refresh_vision();
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skirmish_builds_and_is_deterministic() {
        let scenario = Scenario::skirmish();
        let a = scenario.build().unwrap();
        let b = scenario.build().unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.players.len(), 2);
        assert_eq!(a.buildings.len(), 2);
        assert_eq!(a.units.len(), 8);
        assert!(a.players.iter().all(|p| p.scrap > 0));
    }

    #[test]
    fn missing_anchor_is_an_error() {
        let mut scenario = Scenario::skirmish();
        scenario.players.push(PlayerSpec {
            name: "third".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap: 0,
            bot: false,
            bot_config: None,
        });
        assert!(matches!(
            scenario.build(),
            Err(ScenarioError::MissingAnchor(PlayerId(2)))
        ));
    }

    #[test]
    fn misplaced_unit_is_an_error() {
        let mut scenario = Scenario::skirmish();
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 0,
            y: 0, // border rock
        });
        assert!(matches!(scenario.build(), Err(ScenarioError::BadUnit(_))));
    }
}
