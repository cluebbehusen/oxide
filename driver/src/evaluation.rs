//! Shared pieces of the evaluation harnesses: manifest maps and their
//! families, match modes, fully seated plans, and publishing rows.

use crate::bot_eval::{
    EvaluationController, EvaluationFactionCell, EvaluationGeometry, EvaluationLeg, EvaluationPlan,
    EvidenceBatch, preflight_destinations,
};
use anyhow::{Context, Result, bail};
use oxide_sim::Scenario;
use oxide_sim::scenario::BotConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// File a run publishes in its output directory.
pub const ROWS_FILE: &str = "rows.jsonl";

/// Compact rows of the evaluated legs, published beside their replays.
pub const REPLAY_INDEX_FILE: &str = "legs.jsonl";

/// Map shape, reported separately because it decides which capabilities a
/// match needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapFamily {
    /// Open ground between the bases.
    Open,
    /// Ground routes narrowed by terrain.
    Constrained,
    /// No ground route between the bases.
    Severed,
}

impl MapFamily {
    /// Stable lowercase name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Constrained => "constrained",
            Self::Severed => "severed",
        }
    }
}

/// How a map divides its seats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Two seats on opposing teams.
    Duel,
    /// Two teams of equal size.
    Teams,
    /// More than two seats, each its own team.
    FreeForAll,
}

impl MatchMode {
    /// Stable lowercase name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Duel => "duel",
            Self::Teams => "teams",
            Self::FreeForAll => "free-for-all",
        }
    }

    /// Classifies seats by their teams; any other division is refused.
    pub fn of(teams: &[u8]) -> Result<Self> {
        let mut sizes: BTreeMap<u8, usize> = BTreeMap::new();
        for &team in teams {
            *sizes.entry(team).or_default() += 1;
        }
        let equal = sizes
            .values()
            .all(|&size| Some(&size) == sizes.values().next());
        Ok(match (teams.len(), sizes.len()) {
            (2, 2) => Self::Duel,
            (seats, count) if seats > 2 && count == seats => Self::FreeForAll,
            (_, 2) if equal => Self::Teams,
            _ => bail!(
                "seats on teams {teams:?} are neither a duel, two equal teams nor a free-for-all"
            ),
        })
    }
}

/// One manifest map.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMap {
    /// Scenario path relative to the manifest, or `skirmish` for the built-in
    /// map.
    pub path: String,
    /// Map shape.
    pub family: MapFamily,
}

impl ManifestMap {
    /// Row label: the scenario file stem.
    pub fn key(&self) -> String {
        Path::new(&self.path)
            .file_stem()
            .map_or_else(|| self.path.clone(), |stem| stem.to_string_lossy().into())
    }
}

/// Loads every manifest map, resolving relative paths against `base`, and
/// checks that each builds.
pub fn load_scenarios(maps: &[ManifestMap], base: &Path) -> Result<Vec<Scenario>> {
    maps.iter()
        .map(|map| {
            let scenario = if map.path == "skirmish" {
                Scenario::skirmish()
            } else {
                let path = base.join(&map.path);
                Scenario::load(&path)
                    .with_context(|| format!("loading scenario {}", path.display()))?
            };
            scenario
                .build()
                .with_context(|| format!("building scenario {}", map.path))?;
            Ok(scenario)
        })
        .collect()
}

/// Whether no value in `values` repeats an earlier one.
pub fn distinct<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[..index].contains(value))
}

/// A map's teams by seat.
pub fn seat_teams(source: &Scenario) -> Result<Vec<u8>> {
    Ok(source
        .build()?
        .players()
        .iter()
        .map(|player| player.team)
        .collect())
}

/// A plan with every seat controlled, the authored human chair included.
pub fn seated_plan(
    source: &Scenario,
    leg: EvaluationLeg,
    seats: impl Iterator<Item = BotConfig>,
) -> EvaluationPlan {
    let mut scenario = source.clone();
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }
    EvaluationPlan {
        leg,
        scenario,
        controllers: seats
            .map(|config| Some(EvaluationController::configured(config)))
            .collect(),
        geometry: EvaluationGeometry::Authored,
        faction_cell: EvaluationFactionCell::Authored,
    }
}

/// Refuses an output directory that already holds evaluation rows.
pub fn preflight_output(directory: &Path) -> Result<PathBuf> {
    let path = directory.join(ROWS_FILE);
    preflight_destinations(std::slice::from_ref(&path))?;
    Ok(path)
}

/// Publishes rows to `path` without replacing an existing file.
pub fn publish_rows<T: Serialize>(rows: &[T], path: &Path) -> Result<()> {
    let mut batch = EvidenceBatch::default();
    batch.stage_jsonl(rows, path)?;
    batch.publish()
}
