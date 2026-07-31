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
use serde::{Deserialize, Deserializer, Serialize};
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
    /// Structures standing — built, full hp — at match start, beyond
    /// the Foundries the map anchors place. Empty on every shipped map;
    /// the workhorse of arena experiments (a defense-mode duel needs
    /// turrets that never spent build time). Skipped when empty so
    /// existing scenario and replay bytes stand.
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
    /// Measured duration band, e.g. "5-8 min": the p25-p75 decision
    /// window from `driver pace-sweep` at Medium with the shipped
    /// artifact. An artifact-stamped measurement, never a gate —
    /// re-stamp it when a weights or balance bless moves the clock.
    #[serde(default)]
    pub duration: String,
    /// Mode support, e.g. "1v1" or "2v2".
    #[serde(default)]
    pub mode: String,
    /// Resource richness in plain words ("lean", "standard", "rich").
    #[serde(default)]
    pub richness: String,
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
    /// How that bot plays: a ladder level and personality. `None` means
    /// the legacy rule-cascade bot — which is also what keeps replays
    /// recorded before bot configs existed reproducing, since the
    /// scenario (and therefore
    /// this config) rides inside every replay. The legacy bot is
    /// team-blind: a seat with a `team` must set a config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_config: Option<BotConfig>,
}

/// A named strategic personality selected in match setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedStyle {
    /// Fortify, invest, and counterattack.
    Turtle,
    /// Mix economy, production, and pressure.
    Balanced,
    /// Commit earlier and keep the initiative.
    Aggressive,
}

impl NamedStyle {
    /// All setup-visible styles in display order.
    pub const ALL: [Self; 3] = [Self::Turtle, Self::Balanced, Self::Aggressive];

    /// Inclusive aggression envelope reserved for this named family.
    pub const fn aggression_bounds(self) -> (u32, u32) {
        match self {
            Self::Turtle => (0, 249),
            Self::Balanced => (250, 749),
            Self::Aggressive => (750, 1000),
        }
    }
}

/// A bot's complementary job within its team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    /// No specialized team job, including every default free-for-all seat.
    Generalist,
    /// Apply direct pressure and screen for teammates.
    Vanguard,
    /// Carry the team's economic investment.
    Industry,
    /// Protect and sustain allied positions.
    Support,
    /// Supply long-range pressure against entrenched targets.
    Siege,
}

/// Why a bot profile selection is not meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BotConfigError {
    /// A raw aggression value and named style would both own personality.
    #[error("aggression and style are mutually exclusive")]
    AmbiguousPersonality,
    /// A variant only has meaning inside a named style.
    #[error("variant requires a named style")]
    VariantWithoutStyle,
    /// Every named style currently has exactly three curated variants.
    #[error("variant must be 0, 1, or 2, got {0}")]
    InvalidVariant(u8),
    /// The neural policy's public aggression domain is 0..=1000.
    #[error("aggression must be at most 1000, got {0}")]
    InvalidAggression(u32),
}

/// A shipped-ladder bot seat: difficulty plus personality and team job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    /// Named difficulty on the neural ladder.
    pub level: crate::bot::Level,
    /// Exact legacy personality knob, 0..=1000. This remains supported
    /// for experiments and old scenarios; it cannot accompany `style`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggression: Option<u32>,
    /// Named personality. When both personality fields are absent, the
    /// scenario seed deals a named style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<NamedStyle>,
    /// Curated variant within `style`, 0..=2. When absent, a dedicated
    /// construction-time stream deals one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<u8>,
    /// Optional authored team job. Automatic roles remain mirrored and
    /// complementary; an authored role is mirrored onto its opponent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_role: Option<TeamRole>,
}

impl BotConfig {
    /// Validates combinations that serde cannot express structurally.
    pub fn validate(self) -> Result<(), BotConfigError> {
        if self.aggression.is_some() && self.style.is_some() {
            return Err(BotConfigError::AmbiguousPersonality);
        }
        if self.variant.is_some() && self.style.is_none() {
            return Err(BotConfigError::VariantWithoutStyle);
        }
        if let Some(variant) = self.variant
            && variant >= crate::bot::NAMED_VARIANT_COUNT
        {
            return Err(BotConfigError::InvalidVariant(variant));
        }
        if let Some(aggression) = self.aggression
            && aggression > 1000
        {
            return Err(BotConfigError::InvalidAggression(aggression));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for BotConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            level: crate::bot::Level,
            #[serde(default)]
            aggression: Option<u32>,
            #[serde(default)]
            style: Option<NamedStyle>,
            #[serde(default)]
            variant: Option<u8>,
            #[serde(default)]
            team_role: Option<TeamRole>,
        }

        let fields = Fields::deserialize(deserializer)?;
        let config = Self {
            level: fields.level,
            aggression: fields.aggression,
            style: fields.style,
            variant: fields.variant,
            team_role: fields.team_role,
        };
        config.validate().map_err(serde::de::Error::custom)?;
        Ok(config)
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
    /// A pre-built structure is misplaced or mis-owned.
    #[error("starting building #{0} is invalid (owner in range? footprint on open ground?)")]
    BadBuilding(usize),
    /// Two Foundries can't reach each other: the match could never end.
    #[error("players {0} and {1} are sealed apart — no ground route between their foundries")]
    Disconnected(PlayerId, PlayerId),
    /// Every seat on one team: nobody to fight, no way to win.
    #[error("all players share one team — the match could never end")]
    OneTeam,
    /// A teamed seat asked for the config-less classic bot, which is
    /// team-blind by design and would spend the match targeting allies.
    #[error(
        "player {0} shares a team but fields the classic bot — teamed bot seats need a bot_config"
    )]
    TeamBotNeedsConfig(PlayerId),
    /// A bot's personality or team-role selection cannot be resolved.
    #[error(transparent)]
    BotProfile(#[from] crate::bot::BotProfileError),
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
        crate::bot::profile::resolve_bot_profiles_from_parts(self, &map, &anchors)?;
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
                    resigned: false,
                }
            })
            .collect();
        if self.players.len() > 1 {
            let first = players[0].team;
            if players.iter().all(|p| p.team == first) {
                return Err(ScenarioError::OneTeam);
            }
        }
        // The config-less classic bot is team-blind by design (frozen for
        // pre-0.7 replay reproduction); on a seat with a genuine teammate
        // it would spend the match targeting allies. A team of one is
        // fine — everyone really is its enemy there.
        for (index, spec) in self.players.iter().enumerate() {
            let teamed = players
                .iter()
                .enumerate()
                .any(|(j, p)| j != index && p.team == players[index].team);
            if teamed && spec.bot && spec.bot_config.is_none() {
                return Err(ScenarioError::TeamBotNeedsConfig(PlayerId(index as u8)));
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

        // Authored structures claim ground before units so a unit spec
        // standing inside a footprint fails honestly as BadUnit. Overlaps
        // among the structures themselves fail here: the first placement
        // registers its footprint, so the second's ground reads occupied.
        for (index, spec) in self.buildings.iter().enumerate() {
            let anchor = TilePos::new(spec.x, spec.y);
            let (w, h) = spec.kind.stats().size;
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

        // Authoring tripwire: every pair of Foundries must share a ground
        // route, or the victory condition is unreachable by construction.
        // Flood from the first anchor over terrain (scrap mines out and
        // buildings — foundries and authored structures alike — can be
        // demolished, so terrain is the honest floor of reachability).
        if let Some((first, rest)) = anchors.split_first() {
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
        assert_eq!(turret.hp, BuildingKind::Turret.stats().max_hp);

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
