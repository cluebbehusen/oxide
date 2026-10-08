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

/// Maximum number of seats addressable by authored map anchors.
pub const MAX_PLAYERS: usize = 16;

/// A match definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Match victory rules, or an open-ended sandbox with optional Foundries.
    #[serde(default, skip_serializing_if = "crate::is_default")]
    pub mode: ScenarioMode,
    /// Display name.
    pub name: String,
    /// Master seed for simulation randomness.
    pub seed: u64,
    /// The playfield as ASCII rows (see [`crate::map`] for the legend).
    pub map: Vec<String>,
    /// One entry per player; matches require an anchor `1`..`8` or `a`..`h`
    /// per seat. Sandbox anchors are optional.
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

/// Rules for scenario setup and match completion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioMode {
    /// Foundries define seat survival and team victory.
    #[default]
    Match,
    /// Foundries are optional and play does not end automatically.
    Sandbox,
}

impl ScenarioMode {
    pub(crate) fn is_match(self) -> bool {
        self == Self::Match
    }
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
    /// How the seat's bot plays. Authored scenario data rides inside every
    /// replay. `None` seats no bot at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_config: Option<BotConfig>,
}

/// How one seat's bot plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    /// How accurately and promptly the bot reasons.
    #[serde(default, skip_serializing_if = "crate::is_default")]
    pub difficulty: BotDifficulty,
    /// The broad tempo and risk posture selected by the player.
    #[serde(default, skip_serializing_if = "crate::is_default")]
    pub stance: BotStance,
    /// Seed for the bot's hidden, deterministic personality.
    #[serde(default, skip_serializing_if = "crate::is_default")]
    pub personality_seed: u64,
}

impl BotConfig {
    /// Constructs an exact configuration.
    pub const fn new(difficulty: BotDifficulty, stance: BotStance, personality_seed: u64) -> Self {
        Self {
            difficulty,
            stance,
            personality_seed,
        }
    }
}

/// Player-facing bot skill rung. Every rung obeys the same game rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BotDifficulty {
    /// Slow and distractible, while retaining a competent economy.
    Scrapheap,
    /// The default opponent.
    #[default]
    Standard,
    /// A more attentive and accurate opponent.
    Veteran,
    /// The controller's strongest planning and execution.
    Prime,
}

impl BotDifficulty {
    /// Every difficulty in player-facing order.
    pub const ALL: [Self; 4] = [Self::Scrapheap, Self::Standard, Self::Veteran, Self::Prime];

    /// Stable lowercase name used by CLIs and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scrapheap => "scrapheap",
            Self::Standard => "standard",
            Self::Veteran => "veteran",
            Self::Prime => "prime",
        }
    }
}

impl std::fmt::Display for BotDifficulty {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for BotDifficulty {
    type Err = ParseBotDifficultyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|difficulty| value.eq_ignore_ascii_case(difficulty.as_str()))
            .ok_or_else(|| ParseBotDifficultyError(value.to_owned()))
    }
}

/// An invalid player-facing difficulty name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown bot difficulty `{0}`; expected scrapheap, standard, veteran, or prime")]
pub struct ParseBotDifficultyError(String);

/// Player-selected strategic posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BotStance {
    /// Favor defense, sustain, and counterattack.
    Turtle,
    /// Mix pressure, development, and defense.
    #[default]
    Balanced,
    /// Favor pressure, harassment, and earlier commitments.
    Aggressive,
}

impl BotStance {
    /// Every stance in player-facing order.
    pub const ALL: [Self; 3] = [Self::Turtle, Self::Balanced, Self::Aggressive];

    /// Stable lowercase name used by CLIs and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turtle => "turtle",
            Self::Balanced => "balanced",
            Self::Aggressive => "aggressive",
        }
    }
}

impl std::fmt::Display for BotStance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for BotStance {
    type Err = ParseBotStanceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|stance| value.eq_ignore_ascii_case(stance.as_str()))
            .ok_or_else(|| ParseBotStanceError(value.to_owned()))
    }
}

/// An invalid player-facing stance name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown bot stance `{0}`; expected turtle, balanced, or aggressive")]
pub struct ParseBotStanceError(String);

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
    /// Player count must be 1..=16.
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
    /// Parses the map and validates Foundry anchors against the declared mode
    /// and player table. Shared by state construction and the immutable
    /// pre-match bot briefing so those two views cannot disagree.
    pub fn parse_map_and_anchors(&self) -> Result<(Map, Vec<(PlayerId, TilePos)>), ScenarioError> {
        if self.players.is_empty() || self.players.len() > MAX_PLAYERS {
            return Err(ScenarioError::PlayerCount(self.players.len()));
        }
        let (map, anchors) = Map::parse(&self.map)?;
        if let Some((player, _)) = anchors
            .iter()
            .find(|(player, _)| usize::from(player.0) >= self.players.len())
        {
            return Err(ScenarioError::ExtraAnchor(*player, self.players.len()));
        }
        for index in 0..self.players.len() {
            let player = PlayerId::from_index(index);
            if self.mode.is_match() && !anchors.iter().any(|(anchored, _)| *anchored == player) {
                return Err(ScenarioError::MissingAnchor(player));
            }
        }
        Ok((map, anchors))
    }

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
        let (map, anchors) = self.parse_map_and_anchors()?;
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
                let team = if let Some(id) = spec.team {
                    if let Some((_, dense)) = team_ids.iter().find(|(k, _)| *k == Some(id)) {
                        *dense
                    } else {
                        let dense =
                            u8::try_from(team_ids.len()).expect("teams never outnumber seats");
                        team_ids.push((Some(id), dense));
                        dense
                    }
                } else {
                    let dense = u8::try_from(team_ids.len()).expect("teams never outnumber seats");
                    team_ids.push((None, dense));
                    dense
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
        if self.mode.is_match() && self.players.len() > 1 {
            let first = players[0].team;
            if players.iter().all(|p| p.team == first) {
                return Err(ScenarioError::OneTeam);
            }
        }
        let mut state = State::assemble(map, players, self.seed);
        state.mode = self.mode;

        for &(player, anchor) in &anchors {
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
        if self.mode.is_match()
            && let Some((first, rest)) = anchors.split_first()
        {
            let width = state.map().width();
            let height = state.map().height();
            let idx = |t: TilePos| t.row_major(width);
            let flood = |passable: &dyn Fn(crate::map::Terrain) -> bool| {
                let mut open = std::collections::VecDeque::new();
                let mut seen = vec![false; chassis::grid::cell_count(width, height)];
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
mod tests;
