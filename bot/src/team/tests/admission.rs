use super::*;
use crate::observation::UnitObs;
use crate::orient::Orientation;
use crate::test_support::operations::*;
use oxide_sim::scenario::{BotConfig, BotStance};

#[test]
fn a_pending_team_watch_does_not_own_units_before_deployment_acceptance() {
    let mut obs = test_island_observation();
    obs.known_rock.clear();
    obs.enemy_buildings.clear();
    obs.ally_buildings.push(test_building(
        600,
        2,
        BuildingKind::Foundry,
        TilePos::new(24, 15),
    ));
    obs.enemy_units.push(UnitObs {
        player: PlayerId(1),
        ..test_unit(90, UnitKind::Sentinel, TilePos::new(26, 15))
    });
    obs.my_units = [
        (1, TilePos::new(4, 14)),
        (2, TilePos::new(4, 16)),
        (3, TilePos::new(21, 14)),
        (4, TilePos::new(21, 16)),
        (5, TilePos::new(22, 15)),
        (6, TilePos::new(20, 15)),
    ]
    .map(|(id, tile)| test_unit(id, UnitKind::Sentinel, tile))
    .into();
    let mut profile = crate::profile::ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        0x0A16_7EA0,
    ));
    profile.traits.support = 70;
    profile.traits.fortification = 65;
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let mut relief = TeamReliefPlanner::new();

    let map = crate::test_support::briefing(obs.map_width, obs.map_height, [], vec![]);
    let proposal = relief
        .prepare_relief(crate::team::TeamReliefPreparation {
            tuning,
            obs: &obs,
            home: TEST_HOME,
            map: &map,
            orientation: Orientation::for_home(&obs, TEST_HOME),
            admission: TeamReliefAdmission {
                additionally_reserved: &[],
                allow_new_operation: true,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        })
        .unwrap();
    assert!(relief.operation().is_none());
    assert!(
        relief.reservations().is_empty(),
        "an unaccepted proposal owns no units"
    );
    let accepted = relief
        .prepare_relief_commit(proposal)
        .unwrap()
        .apply(&mut relief);
    assert!(accepted.intents.is_empty());
    assert_eq!(
        relief.operation().unwrap().phase,
        crate::team::TeamReliefPhase::Preparing
    );
    let restored: TeamReliefPlanner =
        serde_json::from_value(serde_json::to_value(&relief).unwrap()).unwrap();
    assert_eq!(restored, relief);
    relief = restored;
    obs.tick += tuning.reaction_delay + oxide_sim::TICKS_PER_SECOND as u64;
    let accepted = relief.think_unrestricted(&profile, tuning, &obs, TEST_HOME, &[], &[]);
    assert_eq!(relief.reservations(), accepted.reservations);
    assert_eq!(accepted.reservations.len(), 2);
    assert!(!accepted.reservations.contains(&UnitId(1)));
    assert!(!accepted.reservations.contains(&UnitId(2)));
}
