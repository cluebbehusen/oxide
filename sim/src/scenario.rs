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
    /// Completed structures present at match start, beyond the Foundries
    /// placed by map anchors. Primarily useful for focused scenarios and
    /// tests. Skipped when empty for compact scenario and replay files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buildings: Vec<BuildingSpec>,
    /// Authored presentation metadata for browsers and previews. The
    /// sim ignores it entirely; it is hashed with the scenario text like
    /// any other byte, and absent on older files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<ScenarioMeta>,
}

/// Presentation-only facts a map browser shows before anyone commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ScenarioMeta {
    /// One-sentence strategic hook.
    #[serde(default)]
    pub hook: String,
    /// Pace label: "quick", "standard", "large", or "vast" — a claim
    /// about map *scale*, which map-audit's route bands hold honest.
    /// It is not a clock reading; `driver pace-sweep` measures those.
    #[serde(default)]
    pub pace: String,
    /// Optional measured duration band, e.g. "5-8 min". This is a
    /// presentation claim rather than a gate; leave it empty until the
    /// current opponent and human play support it.
    #[serde(default)]
    pub duration: String,
    /// Mode support, e.g. "1v1" or "2v2".
    #[serde(default)]
    pub mode: String,
    /// Resource richness in plain words ("lean", "standard", "rich").
    #[serde(default)]
    pub richness: String,
    /// Fairness class. Empty (the default) claims exact 180-degree
    /// paired-seat mirroring, which the map gates verify tile by tile.
    /// "metric" claims measured fairness instead — equal room, route,
    /// scrap, and extractor access within tolerance, with no tile
    /// mirror — the class free-for-all layouts live in.
    #[serde(default)]
    pub symmetry: String,
    /// Tileset/theme key for grading and previews.
    #[serde(default)]
    pub theme: String,
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
    /// Which built-in controller drives the seat. Authored scenario
    /// data rides inside every replay. `None` seats no bot at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_config: Option<BotConfig>,
}

/// A built-in bot controller selected for one seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "controller", rename_all = "snake_case")]
pub enum BotConfig {
    /// The fair, fog-honest rules-based opponent.
    Scripted,
}

// Internally tagged unit variants accept sibling fields even when the enum
// asks Serde to deny them. Struct-shaped wire types keep current input strict
// while allowing versioned replays to carry their historical setup metadata
// as far as replay validation. Historical settings select no retired runtime;
// they normalize to the sole maintained controller.
#[derive(Deserialize)]
#[serde(untagged)]
enum BotConfigWire {
    Current(CurrentBotConfigWire),
    Legacy(LegacyBotConfigWire),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentBotConfigWire {
    controller: BotController,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum BotController {
    Scripted,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyBotConfigWire {
    #[serde(rename = "level")]
    _level: LegacyBotLevel,
    #[serde(default)]
    aggression: Option<u32>,
    #[serde(default)]
    style: Option<LegacyNamedStyle>,
    #[serde(default)]
    variant: Option<u8>,
    #[serde(default, rename = "team_role")]
    _team_role: Option<LegacyTeamRole>,
}

impl LegacyBotConfigWire {
    fn validate(&self) -> Result<(), &'static str> {
        if self.aggression.is_some() && self.style.is_some() {
            return Err("aggression and style are mutually exclusive");
        }
        if self.variant.is_some() && self.style.is_none() {
            return Err("variant requires a named style");
        }
        if self.variant.is_some_and(|variant| variant > 2) {
            return Err("variant must be 0, 1, or 2");
        }
        if self.aggression.is_some_and(|aggression| aggression > 1_000) {
            return Err("aggression must be at most 1000");
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LegacyBotLevel {
    Easy,
    Medium,
    Hard,
    Expert,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyNamedStyle {
    Turtle,
    Balanced,
    Aggressive,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyTeamRole {
    Generalist,
    Vanguard,
    Industry,
    Support,
    Siege,
}

impl<'de> Deserialize<'de> for BotConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match BotConfigWire::deserialize(deserializer)? {
            BotConfigWire::Current(CurrentBotConfigWire {
                controller: BotController::Scripted,
            }) => Ok(Self::Scripted),
            BotConfigWire::Legacy(wire) => {
                wire.validate().map_err(serde::de::Error::custom)?;
                Ok(Self::Scripted)
            }
        }
    }
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

/// A pre-built structure standing at match start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildingSpec {
    /// Owning player index.
    pub player: u8,
    /// Building type.
    pub kind: BuildingKind,
    /// Anchor tile x (top-left of the footprint).
    pub x: i32,
    /// Anchor tile y.
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
    #[error("scenario needs 1 to 16 players, got {0}")]
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
    /// A pre-built structure is misplaced or mis-owned.
    #[error("starting building #{0} is invalid (owner in range? footprint on open ground?)")]
    BadBuilding(usize),
    /// Two Foundries can't reach each other by ground or air: the
    /// match could never end.
    #[error(
        "players {0} and {1} are sealed apart; no ground or air route connects their foundries"
    )]
    Disconnected(PlayerId, PlayerId),
    /// Every seat on one team: nobody to fight, no way to win.
    #[error("all players share one team, so the match could never end")]
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

    /// Moves a seat onto a roster: swaps the faction, keeps any
    /// faction-derived name honest ("North West Cupric" retints to
    /// "North West Ferrous"), and remaps the seat's authored starting
    /// units through their role so faction-bound kinds survive the
    /// flip. Name collisions are the caller's to resolve — two seats
    /// may legitimately end up on one roster.
    pub fn retint_seat(&mut self, seat: usize, faction: Faction) {
        let Some(player) = self.players.get_mut(seat) else {
            return;
        };
        if player.faction == faction {
            return;
        }
        player.name = retinted_name(&player.name, player.faction, faction);
        player.faction = faction;
        for unit in self.units.iter_mut().filter(|u| u.player as usize == seat) {
            unit.kind = unit.kind.role().unit_for(faction);
        }
    }

    /// Validates the scenario and constructs the initial [`State`].
    ///
    /// Building the same scenario twice yields bit-identical states (a test
    /// enforces this).
    pub fn build(&self) -> Result<State, ScenarioError> {
        if self.players.is_empty() || self.players.len() > 16 {
            return Err(ScenarioError::PlayerCount(self.players.len()));
        }
        let (map, anchors) = Map::parse(&self.map)?;

        if let Some((player, _)) = anchors
            .iter()
            .find(|(p, _)| (p.0 as usize) >= self.players.len())
        {
            return Err(ScenarioError::ExtraAnchor(*player, self.players.len()));
        }
        // Teams normalize to dense ids by first appearance: seats naming
        // the same explicit id share one, and every omitted seat gets a
        // fresh singleton — an authored id can never alias a "team of
        // one" seat, whatever number it picked. For every shipped map
        // (all-explicit in authored order, or all-omitted) the dense ids
        // equal the raw values, so old hashes stand.
        let mut team_ids: Vec<(Option<u8>, u8)> = Vec::new();
        let players: Vec<Player> = self
            .players
            .iter()
            .map(|spec| {
                let team = match spec.team {
                    Some(id) => match team_ids.iter().find(|(k, _)| *k == Some(id)) {
                        Some((_, dense)) => *dense,
                        None => {
                            let dense = team_ids.len() as u8;
                            team_ids.push((Some(id), dense));
                            dense
                        }
                    },
                    None => {
                        let dense = team_ids.len() as u8;
                        team_ids.push((None, dense));
                        dense
                    }
                };
                Player {
                    name: spec.name.clone(),
                    faction: spec.faction,
                    team,
                    scrap: spec.scrap,
                    recovery_allowance: 0,
                    recovery_target: 0,
                    recovery_ready: true,
                    resigned: false,
                    eliminated_at: None,
                }
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
            let (w, h) = BuildingKind::Foundry.base_stats().size;
            let footprint_ok = (0..h)
                .flat_map(|dy| (0..w).map(move |dx| anchor.offset(dx, dy)))
                .all(|t| state.passable(t));
            if !footprint_ok {
                return Err(ScenarioError::BadFootprint(player, anchor));
            }
            state.place_building(player, BuildingKind::Foundry, anchor);
        }

        // Authored structures claim ground before units so a unit spec
        // standing inside a footprint fails honestly as BadUnit. Overlaps
        // among the structures themselves fail here: the first placement
        // registers its footprint, so the second's ground reads occupied.
        for (index, spec) in self.buildings.iter().enumerate() {
            let anchor = TilePos::new(spec.x, spec.y);
            let (w, h) = spec.kind.base_stats().size;
            let footprint_ok = (0..h)
                .flat_map(|dy| (0..w).map(move |dx| anchor.offset(dx, dy)))
                .all(|t| state.passable(t));
            if (spec.player as usize) >= self.players.len() || !footprint_ok {
                return Err(ScenarioError::BadBuilding(index));
            }
            state.place_building(PlayerId(spec.player), spec.kind, anchor);
        }

        for (index, spec) in self.units.iter().enumerate() {
            let tile = TilePos::new(spec.x, spec.y);
            // Validated in the unit's own movement domain: a flyer may
            // legally start over any on-map tile it could hover over in
            // play — rock included — while walkers need open ground.
            let standable = state.passable_for(spec.kind.stats().domain, tile);
            if (spec.player as usize) >= self.players.len() || !standable {
                return Err(ScenarioError::BadUnit(index));
            }
            state.spawn_unit(PlayerId(spec.player), spec.kind, tile.center());
        }

        // Authoring tripwire: every pair of Foundries must share a route
        // some mover can actually take, or the victory condition is
        // unreachable by construction. Ground connectivity is the
        // ordinary case; an air route is an honest fallback — the shared
        // tree reaches the sky at tier two, so a Foundry across a pit
        // can genuinely be scouted, bombed, and boarded. Only terrain
        // that seals the sky as well (mesas) makes a true seal. Flood
        // over terrain (scrap mines out and buildings — foundries and
        // authored structures alike — can be demolished, so terrain is
        // the honest floor of reachability).
        if let Some((first, rest)) = anchors.split_first() {
            let width = state.map().width();
            let height = state.map().height();
            let idx = |t: TilePos| (t.y * width + t.x) as usize;
            let flood = |passable: &dyn Fn(crate::map::Terrain) -> bool| {
                let mut open = std::collections::VecDeque::new();
                let mut seen = vec![false; (width * height) as usize];
                let walkable = |t: TilePos| {
                    t.x >= 0
                        && t.y >= 0
                        && t.x < width
                        && t.y < height
                        && state
                            .map()
                            .tile(t)
                            .is_some_and(|tile| passable(tile.terrain))
                };
                let seed = first.1;
                seen[idx(seed)] = true;
                open.push_back(seed);
                while let Some(t) = open.pop_front() {
                    for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        let n = t.offset(dx, dy);
                        if walkable(n) && !seen[idx(n)] {
                            seen[idx(n)] = true;
                            open.push_back(n);
                        }
                    }
                }
                seen
            };
            let ground = flood(&|terrain| terrain == crate::map::Terrain::Ground);
            let mut air = None;
            for (player, anchor) in rest {
                if ground[idx(*anchor)] {
                    continue;
                }
                let air = air
                    .get_or_insert_with(|| flood(&|terrain| terrain != crate::map::Terrain::Peak));
                if !air[idx(*anchor)] {
                    return Err(ScenarioError::Disconnected(first.0, *player));
                }
            }
        }
        state.refresh_vision();
        Ok(state)
    }
}

/// The name a seat wears after a retint onto `to`'s roster: any
/// faction word in the authored name flips ("East Cupric" becomes
/// "East Ferrous"); a name without one keeps itself. This is the one
/// definition of the rule — [`Scenario::retint_seat`] applies it at
/// launch and the setup screen previews through it, so the card can
/// never disagree with the launched match.
pub fn retinted_name(name: &str, from: Faction, to: Faction) -> String {
    let label = |f: Faction| match f {
        Faction::Ferrous => "Ferrous",
        Faction::Cupric => "Cupric",
    };
    name.replace(label(from), label(to))
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
    fn retint_swaps_roster_name_and_faction_bound_kinds() {
        let mut scenario = Scenario::skirmish();
        // A faction-bound starter proves the role remap.
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Stinger,
            x: 3,
            y: 3,
        });
        let old_name = scenario.players[1].name.clone();
        assert_eq!(scenario.players[1].faction, Faction::Cupric);
        scenario.retint_seat(1, Faction::Ferrous);
        assert_eq!(scenario.players[1].faction, Faction::Ferrous);
        assert_ne!(
            scenario.players[1].name, old_name,
            "a faction-derived name follows the roster"
        );
        assert!(
            scenario
                .units
                .iter()
                .filter(|u| u.player == 1)
                .all(|u| u.kind.faction() != Some(Faction::Cupric)),
            "no seat keeps the other roster's kinds"
        );
        assert!(
            scenario.units.iter().any(|u| u.kind == UnitKind::Flakhound),
            "the stinger crossed to its ferrous role twin"
        );
        // Same faction again: a no-op, not a name churn.
        let name = scenario.players[1].name.clone();
        scenario.retint_seat(1, Faction::Ferrous);
        assert_eq!(scenario.players[1].name, name);
        scenario.build().expect("a retinted scenario still builds");
    }

    #[test]
    fn player_count_bounds_are_enforced_and_named() {
        let mut scenario = Scenario::skirmish();
        scenario.players.clear();
        let empty = scenario.build();
        assert!(matches!(empty, Err(ScenarioError::PlayerCount(0))));

        let mut crowded = Scenario::skirmish();
        while crowded.players.len() < 17 {
            crowded.players.push(crowded.players[0].clone());
        }
        let err = crowded.build().expect_err("seventeen seats must refuse");
        assert!(matches!(err, ScenarioError::PlayerCount(17)));
        assert!(
            err.to_string().contains("1 to 16"),
            "the message must name the real bound, got: {err}"
        );
    }

    #[test]
    fn a_blocked_foundry_footprint_is_an_error() {
        // The anchor byte sits on open ground but its 2x2 footprint
        // reaches into border rock: the scenario must refuse rather
        // than stand a Foundry inside a wall.
        let mut scenario = Scenario::skirmish();
        let last = scenario.map.len() - 2;
        let row = scenario.map[last].clone();
        let inner = row.trim_matches('#').len() + row.len() - row.trim_start_matches('#').len() - 1;
        let _ = inner;
        // Move player 0's anchor to the last interior column, so the
        // footprint's second column lands on the border.
        for line in scenario.map.iter_mut() {
            *line = line.replace('1', ".");
        }
        let width = scenario.map[1].len();
        let mut edge_row: Vec<char> = scenario.map[1].chars().collect();
        edge_row[width - 2] = '1';
        scenario.map[1] = edge_row.into_iter().collect();
        assert!(matches!(
            scenario.build(),
            Err(ScenarioError::BadFootprint(PlayerId(0), _))
        ));
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
    fn authored_structures_stand_built_and_validate_their_ground() {
        let mut scenario = Scenario::skirmish();
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 9,
            y: 5,
        });
        let state = scenario.build().unwrap();
        let turret = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Turret)
            .expect("the authored turret stands");
        assert!(turret.built, "at full strength from tick zero");
        assert_eq!(turret.hp, BuildingKind::Turret.base_stats().max_hp);

        // The same anchor twice: the second footprint reads occupied.
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 9,
            y: 5,
        });
        assert!(matches!(
            scenario.build(),
            Err(ScenarioError::BadBuilding(1))
        ));
        // Border rock refuses a footprint outright.
        scenario.buildings.clear();
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 0,
            y: 0,
        });
        assert!(matches!(
            scenario.build(),
            Err(ScenarioError::BadBuilding(0))
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
