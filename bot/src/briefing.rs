//! Immutable public facts available before the first simulation tick.
//!
//! A briefing is derived from the authored [`Scenario`], not from live
//! [`State`](oxide_sim::State). It records what the match setup and fog-free map
//! preview disclose: static terrain, resource starting locations, teams, and
//! starting Foundry anchors. None of those priors asserts that a resource or
//! Foundry still exists later in the match.

use chassis::grid::TilePos;
use oxide_sim::ids::PlayerId;
use oxide_sim::map::{Map, Terrain};
use oxide_sim::scenario::{Scenario, ScenarioError};

/// One Foundry anchor authored for a player at match start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StartingFoundry {
    /// Seat that owned the Foundry at tick zero.
    pub player: PlayerId,
    /// Top-left anchor of the Foundry footprint.
    pub anchor: TilePos,
}

#[derive(Clone, Default)]
pub(super) struct RegionCache {
    generations: std::sync::Arc<[std::sync::OnceLock<RegionGeneration>; 4]>,
    orientation: usize,
}

impl RegionCache {
    pub(super) fn oriented(&self, flip_x: bool, flip_y: bool) -> Self {
        Self {
            generations: std::sync::Arc::clone(&self.generations),
            orientation: self.orientation ^ (usize::from(flip_x) | (usize::from(flip_y) << 1)),
        }
    }
}

struct RegionGeneration {
    width: i32,
    height: i32,
    terrain: Vec<(TilePos, Terrain)>,
    regions: std::sync::Arc<super::navigation::regions::StaticRegions>,
}

impl core::fmt::Debug for RegionCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RegionCache").finish_non_exhaustive()
    }
}

// Derived memoization does not change the identity of public map knowledge.
impl PartialEq for RegionCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
impl Eq for RegionCache {}

/// Canonical pre-match map knowledge shared by player-facing bots.
///
/// Dynamic facts never enter this type. In particular, `initial_scrap` is not
/// a live amount and `starting_foundries` is not a list of current targets.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublicMapBriefing {
    #[serde(skip)]
    pub(super) regions: RegionCache,
    pub(super) map_width: i32,
    pub(super) map_height: i32,
    pub(super) starting_foundries: Vec<StartingFoundry>,
    /// Raw authored team labels aligned with player ids. Two omitted labels
    /// remain distinct singleton teams rather than aliasing through `None`.
    pub(super) teams: Vec<Option<u8>>,
    /// Non-ground terrain in canonical `(y, x)` order. Missing in-bounds
    /// positions are ordinary ground.
    pub(super) non_ground_terrain: Vec<(TilePos, Terrain)>,
    pub(super) extractor_frames: Vec<TilePos>,
    pub(super) initial_scrap: Vec<(TilePos, u32)>,
}

impl PublicMapBriefing {
    /// Derives the public prior from the exact scenario that will launch.
    pub fn from_scenario(scenario: &Scenario) -> Result<Self, ScenarioError> {
        let (map, anchors) = scenario.parse_map_and_anchors()?;
        Ok(Self::from_parsed(scenario, &map, anchors))
    }

    fn from_parsed(scenario: &Scenario, map: &Map, anchors: Vec<(PlayerId, TilePos)>) -> Self {
        let mut non_ground_terrain = Vec::new();
        let mut initial_scrap = Vec::new();
        for (position, tile) in map.iter() {
            if tile.terrain != Terrain::Ground {
                non_ground_terrain.push((position, tile.terrain));
            }
            if tile.scrap > 0 {
                initial_scrap.push((position, tile.scrap));
            }
        }
        Self {
            regions: RegionCache::default(),
            map_width: map.width(),
            map_height: map.height(),
            starting_foundries: anchors
                .into_iter()
                .map(|(player, anchor)| StartingFoundry { player, anchor })
                .collect(),
            teams: scenario.players.iter().map(|player| player.team).collect(),
            non_ground_terrain,
            extractor_frames: map.extractor_frames().to_vec(),
            initial_scrap,
        }
    }

    pub(super) fn prepare_navigation(&self) {
        for home in [
            TilePos::new(0, 0),
            TilePos::new(self.map_width - 1, 0),
            TilePos::new(0, self.map_height - 1),
            TilePos::new(self.map_width - 1, self.map_height - 1),
        ] {
            let orientation =
                super::orient::Orientation::for_map(self.map_width, self.map_height, home);
            orientation.briefing(self).regions();
        }
    }

    pub(super) fn regions(&self) -> std::sync::Arc<super::navigation::regions::StaticRegions> {
        let cached =
            self.regions.generations[self.regions.orientation].get_or_init(|| RegionGeneration {
                width: self.map_width,
                height: self.map_height,
                terrain: self.non_ground_terrain.clone(),
                regions: std::sync::Arc::new(super::navigation::regions::StaticRegions::build(
                    self,
                )),
            });
        if cached.width == self.map_width
            && cached.height == self.map_height
            && cached.terrain == self.non_ground_terrain
        {
            std::sync::Arc::clone(&cached.regions)
        } else {
            std::sync::Arc::new(super::navigation::regions::StaticRegions::build(self))
        }
    }

    /// Map width in tiles.
    pub fn map_width(&self) -> i32 {
        self.map_width
    }

    /// Map height in tiles.
    pub fn map_height(&self) -> i32 {
        self.map_height
    }

    /// Every authored starting Foundry, in player-id order.
    pub fn starting_foundries(&self) -> &[StartingFoundry] {
        &self.starting_foundries
    }

    /// Starting Foundries belonging to seats hostile to `viewer`.
    pub fn hostile_starting_foundries(
        &self,
        viewer: PlayerId,
    ) -> impl Iterator<Item = &StartingFoundry> {
        self.starting_foundries
            .iter()
            .filter(move |start| self.hostile(viewer, start.player))
    }

    /// Whether the two authored seats begin on opposing teams.
    pub fn hostile(&self, viewer: PlayerId, other: PlayerId) -> bool {
        if viewer == other {
            return false;
        }
        let Some(viewer_team) = self.teams.get(usize::from(viewer.0)) else {
            return false;
        };
        let Some(other_team) = self.teams.get(usize::from(other.0)) else {
            return false;
        };
        !matches!((viewer_team, other_team), (Some(a), Some(b)) if a == b)
    }

    /// Canonical sparse static terrain. Any omitted in-bounds tile is ground.
    pub fn non_ground_terrain(&self) -> &[(TilePos, Terrain)] {
        &self.non_ground_terrain
    }

    /// Authored terrain at `position`, or `None` outside the map.
    pub fn terrain_at(&self, position: TilePos) -> Option<Terrain> {
        if position.x < 0
            || position.y < 0
            || position.x >= self.map_width
            || position.y >= self.map_height
        {
            return None;
        }
        Some(
            self.non_ground_terrain
                .binary_search_by_key(&(position.y, position.x), |(tile, _)| (tile.y, tile.x))
                .ok()
                .map_or(Terrain::Ground, |index| self.non_ground_terrain[index].1),
        )
    }

    /// Every derelict Extractor frame anchor in authored row-major order.
    pub fn extractor_frames(&self) -> &[TilePos] {
        &self.extractor_frames
    }

    /// Initial scrap nodes and authored amounts in `(y, x)` order.
    ///
    /// These are priors for scouting and valuation, not live depletion data.
    pub fn initial_scrap(&self) -> &[(TilePos, u32)] {
        &self.initial_scrap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::scenario::PlayerSpec;
    use oxide_sim::state::Faction;
    use oxide_sim::stats::{RICH_SCRAP_NODE_AMOUNT, SCRAP_NODE_AMOUNT};

    fn briefing_scenario() -> Scenario {
        Scenario {
            name: "briefing".into(),
            seed: 4,
            map: [
                "##########",
                "#1.s.E...#",
                "#...~....#",
                "#..^..S2.#",
                "#3.......#",
                "#........#",
                "##########",
            ]
            .map(str::to_owned)
            .into(),
            players: vec![
                PlayerSpec {
                    name: "one".into(),
                    faction: Faction::Ferrous,
                    team: Some(7),
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "two".into(),
                    faction: Faction::Cupric,
                    team: Some(7),
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "three".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        }
    }

    #[test]
    fn captures_canonical_public_map_facts_without_live_state() {
        let scenario = briefing_scenario();
        let briefing = PublicMapBriefing::from_scenario(&scenario).expect("briefing builds");

        assert_eq!((briefing.map_width(), briefing.map_height()), (10, 7));
        assert_eq!(
            briefing.starting_foundries(),
            &[
                StartingFoundry {
                    player: PlayerId(0),
                    anchor: TilePos::new(1, 1),
                },
                StartingFoundry {
                    player: PlayerId(1),
                    anchor: TilePos::new(7, 3),
                },
                StartingFoundry {
                    player: PlayerId(2),
                    anchor: TilePos::new(1, 4),
                },
            ]
        );
        assert_eq!(briefing.extractor_frames(), &[TilePos::new(5, 1)]);
        assert_eq!(
            briefing.initial_scrap(),
            &[
                (TilePos::new(3, 1), SCRAP_NODE_AMOUNT),
                (TilePos::new(6, 3), RICH_SCRAP_NODE_AMOUNT),
            ]
        );
        assert_eq!(briefing.terrain_at(TilePos::new(4, 2)), Some(Terrain::Pit));
        assert_eq!(briefing.terrain_at(TilePos::new(3, 3)), Some(Terrain::Peak));
        assert_eq!(
            briefing.terrain_at(TilePos::new(4, 3)),
            Some(Terrain::Ground)
        );
        assert_eq!(briefing.terrain_at(TilePos::new(-1, 0)), None);
        assert!(
            briefing
                .non_ground_terrain()
                .windows(2)
                .all(|pair| (pair[0].0.y, pair[0].0.x) < (pair[1].0.y, pair[1].0.x))
        );

        let state = scenario.build().expect("fixture is a playable scenario");
        assert_eq!(
            briefing.initial_scrap()[0].1,
            state.map().scrap_at(TilePos::new(3, 1)),
            "the prior begins equal to tick-zero state without borrowing it"
        );
    }

    #[test]
    fn explicit_teams_share_a_start_while_omitted_teams_remain_singletons() {
        let briefing =
            PublicMapBriefing::from_scenario(&briefing_scenario()).expect("briefing builds");

        assert!(!briefing.hostile(PlayerId(0), PlayerId(1)));
        assert!(briefing.hostile(PlayerId(0), PlayerId(2)));
        assert!(briefing.hostile(PlayerId(2), PlayerId(0)));
        assert!(!briefing.hostile(PlayerId(2), PlayerId(2)));
        assert!(!briefing.hostile(PlayerId(99), PlayerId(0)));
        assert_eq!(
            briefing
                .hostile_starting_foundries(PlayerId(0))
                .map(|start| start.player)
                .collect::<Vec<_>>(),
            [PlayerId(2)]
        );
    }

    #[test]
    fn two_omitted_team_labels_are_distinct_hostile_singletons() {
        let mut scenario = briefing_scenario();
        scenario.players[1].team = None;
        let briefing = PublicMapBriefing::from_scenario(&scenario).expect("briefing builds");

        assert!(briefing.hostile(PlayerId(1), PlayerId(2)));
        assert!(briefing.hostile(PlayerId(2), PlayerId(1)));
        assert_eq!(
            briefing
                .hostile_starting_foundries(PlayerId(1))
                .map(|start| start.player)
                .collect::<Vec<_>>(),
            [PlayerId(0), PlayerId(2)]
        );
    }

    #[test]
    fn briefing_construction_rejects_a_player_anchor_mismatch() {
        let mut extra = briefing_scenario();
        extra.players.pop();
        assert!(matches!(
            PublicMapBriefing::from_scenario(&extra),
            Err(ScenarioError::ExtraAnchor(PlayerId(2), 2))
        ));

        let mut missing = briefing_scenario();
        missing.map[4] = "#........#".into();
        assert!(matches!(
            PublicMapBriefing::from_scenario(&missing),
            Err(ScenarioError::MissingAnchor(PlayerId(2)))
        ));
    }

    #[test]
    fn all_sixteen_anchor_symbols_are_preserved_in_player_order() {
        let anchors = "12345678abcdefgh";
        let width = anchors.len() + 2;
        let border = "#".repeat(width);
        let scenario = Scenario {
            name: "sixteen starts".into(),
            seed: 0,
            map: vec![
                border.clone(),
                format!("#{anchors}#"),
                format!("#{}#", ".".repeat(anchors.len())),
                border,
            ],
            players: (0..16)
                .map(|seat| oxide_sim::scenario::PlayerSpec {
                    name: format!("seat {seat}"),
                    faction: if seat % 2 == 0 {
                        Faction::Ferrous
                    } else {
                        Faction::Cupric
                    },
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        };
        let briefing = PublicMapBriefing::from_scenario(&scenario).expect("all starts parse");

        assert_eq!(briefing.starting_foundries().len(), 16);
        assert!(
            briefing
                .starting_foundries()
                .iter()
                .enumerate()
                .all(|(seat, start)| start.player == PlayerId(seat as u8))
        );
    }

    #[test]
    fn setup_prepares_and_shares_each_oriented_region_index() {
        let map = PublicMapBriefing::from_scenario(&briefing_scenario()).unwrap();
        map.prepare_navigation();
        assert!(
            map.regions
                .generations
                .iter()
                .all(|entry| entry.get().is_some())
        );
        for home in [
            TilePos::new(0, 0),
            TilePos::new(map.map_width - 1, 0),
            TilePos::new(0, map.map_height - 1),
            TilePos::new(map.map_width - 1, map.map_height - 1),
        ] {
            let orientation =
                super::super::orient::Orientation::for_map(map.map_width, map.map_height, home);
            let first = orientation.briefing(&map);
            let second = orientation.briefing(&map);
            assert!(std::sync::Arc::ptr_eq(&first.regions(), &second.regions()));
            let restored = orientation.briefing(&first);
            assert_eq!(restored, map);
            assert!(std::sync::Arc::ptr_eq(&restored.regions(), &map.regions()));
        }
    }
}
