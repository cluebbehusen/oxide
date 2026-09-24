//! Local policy fixtures.
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, OBSERVATION_VERSION, ObservationData, UnitObs};
use oxide_sim::{BuildingId, BuildingKind, PlayerId, UnitId, UnitKind};
pub(crate) fn observation_data() -> ObservationData {
    ObservationData {
        version: OBSERVATION_VERSION,
        tick: 0,
        me: PlayerId(0),
        scrap: 0,
        map_width: 0,
        map_height: 0,
        my_units: Vec::new(),
        my_carried_units: Vec::new(),
        my_buildings: Vec::new(),
        my_queues: Vec::new(),
        my_queue_progress: Vec::new(),
        my_queued_units: Vec::new(),
        my_repair_targets: Vec::new(),
        ally_units: Vec::new(),
        ally_buildings: Vec::new(),
        enemy_units: Vec::new(),
        enemy_buildings: Vec::new(),
        visible: Vec::new(),
        explored: Vec::new(),
        known_scrap: Vec::new(),
        known_rock: Vec::new(),
        known_pits: Vec::new(),
        known_frames: Vec::new(),
        known_peaks: Vec::new(),
        known_wrecks: Vec::new(),
        salvage_incidents: Vec::new(),
        blips: Vec::new(),
        contact_tracks: Vec::new(),
        faction: oxide_sim::state::Faction::Ferrous,
        my_shells: 0,
        incoming_shells: Vec::new(),
    }
}
pub(crate) fn unit(id: u32, player: PlayerId, kind: UnitKind, tile: TilePos) -> UnitObs {
    UnitObs {
        id: UnitId(id),
        player,
        kind,
        tile,
        hp: kind.stats().max_hp,
        idle: true,
        carrying: 0,
        harvesting: None,
        cargo: 0,
        site: None,
        salvaging: None,
        founding: None,
        repairing: false,
        grounded: false,
    }
}
pub(crate) fn building(
    id: u32,
    player: PlayerId,
    kind: BuildingKind,
    anchor: TilePos,
) -> BuildingObs {
    BuildingObs {
        provisional: false,
        id: BuildingId(id),
        player,
        kind,
        anchor,
        hp: kind.base_stats().max_hp,
        built: true,
        seen: true,
        tier: 0,
    }
}

pub(crate) fn edit_state(state: &mut oxide_sim::State, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut data = serde_json::to_value(&*state).unwrap();
    edit(&mut data);
    *state = serde_json::from_value(data)
        .expect("the edited fixture must satisfy simulation invariants");
}

pub(crate) fn set_tick(state: &mut oxide_sim::State, tick: chassis::Tick) {
    let previous = state.current_tick();
    assert!(tick >= previous, "fixture clocks only move forward");
    edit_state(state, |data| {
        data["tick"] = tick.into();
        // A frozen world moved to a later decision tick retains contact ages.
        for vision in data["vision"].as_array_mut().unwrap() {
            for track in vision["tracking"]["tracks"].as_array_mut().unwrap() {
                for sample in track["history"].as_array_mut().unwrap() {
                    sample["tick"] = (sample["tick"].as_u64().unwrap() + tick - previous).into();
                }
            }
        }
    });
}

pub(crate) fn edit_units(
    state: &mut oxide_sim::State,
    edit: impl FnOnce(&mut Vec<oxide_sim::Unit>),
) {
    edit_state(state, |data| {
        let mut units = serde_json::from_value(data["units"].take()).unwrap();
        edit(&mut units);
        data["units"] = serde_json::to_value(units).unwrap();
    });
}

pub(crate) fn edit_buildings(
    state: &mut oxide_sim::State,
    edit: impl FnOnce(&mut Vec<oxide_sim::Building>),
) {
    edit_state(state, |data| {
        let mut buildings = serde_json::from_value(data["buildings"].take()).unwrap();
        edit(&mut buildings);
        data["buildings"] = serde_json::to_value(buildings).unwrap();
    });
}

pub(crate) fn edit_building(
    state: &mut oxide_sim::State,
    id: BuildingId,
    edit: impl FnOnce(&mut oxide_sim::Building),
) {
    edit_buildings(state, |buildings| {
        edit(buildings.iter_mut().find(|b| b.id == id).unwrap())
    });
}

pub(crate) fn edit_player(
    state: &mut oxide_sim::State,
    id: PlayerId,
    edit: impl FnOnce(&mut oxide_sim::Player),
) {
    edit_state(state, |data| {
        let mut player = serde_json::from_value(data["players"][usize::from(id.0)].take()).unwrap();
        edit(&mut player);
        data["players"][usize::from(id.0)] = serde_json::to_value(player).unwrap();
    });
}

impl crate::utility::UtilityPolicy {
    pub(crate) fn think_residual(
        &mut self,
        dials: &crate::Dials,
        obs: &crate::Observation,
        armies: &[crate::Army],
        enlisted: &[UnitId],
        reserved: &[UnitId],
        public_map: &crate::PublicMapBriefing,
    ) -> Vec<crate::Intent> {
        let mut intelligence = crate::StrategicIntelligence::new();
        intelligence.update(obs);
        self.observe_work_experience(obs);
        self.refresh_allocation_worker_safety(obs, intelligence.units(), intelligence.buildings());
        self.think_with_intelligence(
            dials,
            obs,
            armies,
            enlisted,
            crate::utility::StrategicUtilityContext::new(
                reserved,
                intelligence.units(),
                intelligence.buildings(),
                public_map,
                Vec::new(),
                Default::default(),
            )
            .with_ground_missions(crate::utility::GroundMissionInputs {
                unavailable: reserved,
                enlisted,
                tuning: tuning_for(dials),
                relief: None,
            }),
        )
    }
}

/// The player-facing tuning whose copied fields match `dials`; fixtures that
/// edit those fields fall back to Standard.
fn tuning_for(dials: &crate::Dials) -> crate::difficulty::DifficultyTuning {
    use crate::difficulty::DifficultyTuning;
    use oxide_sim::scenario::BotDifficulty;
    BotDifficulty::ALL
        .into_iter()
        .map(DifficultyTuning::for_level)
        .find(|tuning| {
            tuning.cadence == dials.cadence
                && tuning.minimum_core_equivalents == dials.minimum_core_equivalents
                && tuning.underestimate_own(10_000) == u64::from(dials.own_strength_scale)
        })
        .unwrap_or_else(|| DifficultyTuning::for_level(BotDifficulty::Standard))
}

pub(crate) fn briefing(
    width: i32,
    height: i32,
    walls: impl IntoIterator<Item = TilePos>,
    starts: Vec<crate::StartingFoundry>,
) -> crate::PublicMapBriefing {
    let mut non_ground_terrain = walls
        .into_iter()
        .map(|tile| (tile, oxide_sim::map::Terrain::Rock))
        .collect::<Vec<_>>();
    non_ground_terrain.sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
    crate::PublicMapBriefing {
        regions: Default::default(),
        map_width: width,
        map_height: height,
        starting_foundries: starts,
        teams: vec![Some(0), Some(1)],
        non_ground_terrain,
        extractor_frames: Vec::new(),
        initial_scrap: Vec::new(),
    }
}
