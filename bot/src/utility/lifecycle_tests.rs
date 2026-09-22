use super::construction_checks::{GroundKnowledge, KnowledgeDomain, shortest_path_between};
use super::test_world::*;
use super::*;
use crate::Orientation;
use crate::navigation::public_fields::BlockedGroundLayout;
use crate::planning::{PlanningWork, Progress};
use crate::query_work::QueryPurpose;
use oxide_sim::ids::PlayerId;

#[test]
fn rejected_policy_state_keeps_queries_correct_for_restored_and_changed_inputs() {
    let node = TilePos::new(12, 10);
    let map =
        PublicMapBriefing::from_scenario(&scenario_with(
            |tile| {
                if tile == node { 's' } else { '.' }
            },
        ))
        .unwrap();
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.visible.fill(true);
    obs.known_scrap = vec![(node, 400)];
    obs.my_units[0].harvesting = Some(node);
    let foundation = (BuildingKind::Turret, TilePos::new(9, 10));
    obs.my_units[0].founding = Some(foundation);

    let evaluate = |policy: &UtilityPolicy, obs: &Observation, cancelled: bool| {
        let ground = GroundKnowledge::new(QueryPurpose::NavigationTest, obs, &map, &[])
            .retained(policy, false);
        let paths = [KnowledgeDomain::Ground, KnowledgeDomain::Air].map(|domain| {
            shortest_path_between(
                &ground,
                &[TilePos::new(7, 10)],
                &[TilePos::new(15, 10)],
                None,
                domain,
            )
        });
        let assets = ground.resource_assets(policy);
        let danger = policy.harvest_danger_projection(obs, None, None);
        let harvest = policy.economic_harvest_regions(
            obs,
            &map,
            &ResourceSnapshot::from_observation(obs),
            Orientation::for_home(obs, LEFT_HOME),
            &[],
            (&[], &[]),
        );
        let cancellations = [foundation];
        policy.prepare_ground_producer_egress_after(
            obs,
            FoundationCancellations(if cancelled { &cancellations } else { &[] }),
        );
        let egress = policy.preserves_ground_producer_egress_prepared(
            &[],
            (BuildingKind::Reclaimer, TilePos::new(7, 11)),
        );
        let blocked = BlockedGroundLayout::from_predicate(&map, |tile| {
            danger.contains(tile) || policy.harvest_location_contested(tile)
        });
        let field = policy
            .queries
            .expansion_routing_cache
            .borrow_mut()
            .danger_aware_source_set(QueryPurpose::NavigationTest, &map, &blocked, [node]);
        (paths, assets, danger, harvest, egress, field)
    };
    let mut policy = UtilityPolicy::new();
    let baseline = evaluate(&policy, &obs, false);
    assert!(baseline.0.iter().all(Option::is_some));
    assert!(!baseline.1.is_empty());
    assert!(!baseline.3.is_empty());
    assert_ne!(
        policy.queries.knowledge_paths.borrow().clone(),
        Default::default()
    );
    let checkpoint = policy.speculative_checkpoint();
    policy.state.work_experience.dead_nodes.push(node);
    policy
        .state
        .contested_harvest_regions
        .push(ContestedHarvestRegion {
            center: TilePos::new(14, 10),
            last_evidence: obs.tick,
            sweep_started_at: None,
        });
    let mut changed = obs.clone();
    changed.blips.push(TilePos::new(24, 10));
    let speculative = evaluate(&policy, &changed, true);
    assert_ne!(speculative, baseline);
    let warmed = policy.queries.clone();
    policy.restore_checkpoint(checkpoint.clone());
    assert_eq!(policy.speculative_checkpoint(), checkpoint);
    assert_eq!(
        policy.queries, warmed,
        "rollback must not copy query storage"
    );
    assert_eq!(evaluate(&policy, &obs, false), baseline);

    for change in 0..6 {
        let mut current = obs.clone();
        match change {
            0 => current.known_scrap[0].1 = 70,
            1 => current.my_units[0].harvesting = None,
            2 => current.visible.fill(false),
            3 => current
                .known_rock
                .extend((0..HEIGHT).map(|y| TilePos::new(8, y))),
            4 => current.blips.push(TilePos::new(15, 10)),
            5 => current.my_buildings[0].built = false,
            _ => unreachable!(),
        }
        for cancelled in [true, false] {
            assert_eq!(
                evaluate(&policy, &current, cancelled),
                evaluate(&UtilityPolicy::new(), &current, cancelled),
                "input {change}, cancellation {cancelled}"
            );
            assert_eq!(evaluate(&policy, &obs, false), baseline);
        }
    }
}

#[test]
fn checkpoint_preserves_planning_allowance_continuations_and_eventual_results() {
    let map = briefing();
    let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
    let source = TilePos::new(7, 10);
    let request = |policy: &UtilityPolicy, tick| {
        policy
            .planning
            .field(QueryPurpose::NavigationTest, tick, &map, &blocked, [source])
    };
    let mut policy = UtilityPolicy::new();
    policy.planning = PlanningWork::with_allowance(128);
    policy.planning.begin(0);
    assert_eq!(request(&policy, 0), Progress::Deferred);
    let checkpoint = policy.speculative_checkpoint();
    policy.planning.begin(1);
    assert_eq!(request(&policy, 1), Progress::Deferred);
    let control = policy.clone();
    policy.state.work_experience.dead_nodes.push(source);
    policy.restore_checkpoint(checkpoint);
    assert_eq!(policy.state, control.state);
    assert_eq!(policy.planning, control.planning);
    for tick in 1..100 {
        policy.planning.begin(tick);
        control.planning.begin(tick);
        let actual = request(&policy, tick);
        assert_eq!(actual, request(&control, tick));
        assert_eq!(policy.planning, control.planning);
        if let Progress::Ready(field) = actual {
            assert_eq!(field.footprint_distance(source, (1, 1)), Some(0));
            return;
        }
    }
    panic!("the preserved continuation must finish without replenishing its same-tick allowance");
}
