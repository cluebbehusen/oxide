use super::super::test_world::*;
use super::*;
use crate::bot::ContactEvidence;
use crate::ids::{PlayerId, UnitId};

#[test]
fn trailing_stealthy_build_cannot_hide_an_earlier_producer_seal() {
    let obs = observation(PlayerId(0), LEFT_HOME);
    let map = briefing();
    let mut builds: Vec<_> =
        crate::tick::rect_adjacent_tiles(LEFT_HOME, BuildingKind::Foundry.base_stats().size)
            .map(|anchor| (BuildingKind::Barricade, anchor))
            .collect();
    builds.push((BuildingKind::ScuttleCharge, TilePos::new(20, 4)));

    assert!(
        !UtilityPolicy::new().combined_build_layout_is_safe(&obs, &map, &builds),
        "the closing blocking footprints must be checked even when a mine sorts last"
    );
}

#[test]
fn covered_builder_preflight_matches_the_full_frozen_layout_rejection() {
    let scenario = scenario_with(|_| '.');
    let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    let anchor = TilePos::new(18, 10);
    let builder = UnitId(7);
    let orientation = Orientation::for_home(&obs, LEFT_HOME);
    for kind in [BuildingKind::Foundry, BuildingKind::ScuttleCharge] {
        for tile in [
            anchor,
            anchor.offset(1, 1),
            anchor.offset(-1, 0),
            anchor.offset(2, 0),
        ] {
            obs.my_units = vec![unit(builder.0, PlayerId(0), UnitKind::Harvester, tile)];
            let builds = [(kind, anchor, builder)];
            let covered =
                !kind.is_stealthy() && footprint_contains(anchor, kind.base_stats().size, tile);
            assert_eq!(
                UtilityPolicy::build_layout_covers_assigned_builder(&obs, &builds),
                covered
            );
            let policy = UtilityPolicy::new();
            let original = policy.combined_build_layout_is_safe_inner(
                CombinedLayoutContext {
                    obs: &obs,
                    briefing: &map,
                    unit_contacts: &[],
                    building_contacts: &[],
                    orientation: Some(orientation),
                },
                &[(kind, anchor)],
                &builds,
            );
            if covered {
                assert!(!original);
            }
            assert_eq!(
                policy.combined_build_layout_with_builders_is_safe(
                    &obs,
                    &map,
                    &[],
                    &[],
                    orientation,
                    &builds
                ),
                original
            );
        }
    }
    obs.my_units = vec![
        unit(7, PlayerId(0), UnitKind::Harvester, anchor.offset(-1, 0)),
        unit(8, PlayerId(0), UnitKind::Harvester, anchor),
    ];
    let builds = [
        (BuildingKind::Foundry, anchor, UnitId(7)),
        (BuildingKind::Turret, anchor.offset(6, 0), UnitId(8)),
    ];
    assert!(UtilityPolicy::build_layout_covers_assigned_builder(
        &obs, &builds
    ));
    assert!(
        !UtilityPolicy::build_layout_covers_assigned_builder(&obs, &builds[..1]),
        "an unselected worker does not create an assigned-builder rejection"
    );
}

#[test]
fn combined_layout_preserves_a_proposed_foundry_egress() {
    let foundry_anchor = TilePos::new(10, 10);
    let exit = TilePos::new(12, 10);
    let terrain = |tile: TilePos| {
        let inside_foundry = (10..=11).contains(&tile.x) && (10..=11).contains(&tile.y);
        if inside_foundry || tile.y == 10 && tile.x >= exit.x {
            '.'
        } else {
            '^'
        }
    };
    let scenario = scenario_with(terrain);
    let map = PublicMapBriefing::from_scenario(&scenario).expect("single-exit briefing");
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.known_peaks = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
        .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
        .collect();
    obs.known_rock = obs.known_peaks.clone();
    let policy = UtilityPolicy::new();
    let foundry = (BuildingKind::Foundry, foundry_anchor);

    assert!(
        policy.combined_build_layout_is_safe(&obs, &map, &[foundry]),
        "the proposed Foundry can spawn into its authored outside component"
    );
    assert!(
        !policy
            .combined_build_layout_is_safe(&obs, &map, &[foundry, (BuildingKind::Turret, exit)],),
        "the paired defense cannot close the proposed producer's only doorstep"
    );
}

#[test]
fn combined_layout_preserves_an_unfinished_foundry_future_egress() {
    let foundry_anchor = TilePos::new(10, 10);
    let exit = TilePos::new(12, 10);
    let safe_site = TilePos::new(25, 5);
    let terrain = |tile: TilePos| {
        let inside_foundry = footprint_contains(
            foundry_anchor,
            BuildingKind::Foundry.base_stats().size,
            tile,
        );
        if inside_foundry || tile.y == 10 && tile.x >= exit.x || tile.chebyshev(safe_site) <= 1 {
            '.'
        } else {
            '^'
        }
    };
    let scenario = scenario_with(terrain);
    let map = PublicMapBriefing::from_scenario(&scenario).expect("future-egress briefing");
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.known_peaks = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
        .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
        .collect();
    obs.known_rock = obs.known_peaks.clone();
    let mut unfinished = building(7, PlayerId(0), BuildingKind::Foundry, foundry_anchor);
    unfinished.built = false;
    obs.my_buildings.push(unfinished);
    obs.my_queues.push(Vec::new());
    let policy = UtilityPolicy::new();

    assert!(
        policy.combined_build_layout_is_safe(&obs, &map, &[(BuildingKind::Turret, safe_site)],)
    );
    assert!(
        !policy.combined_build_layout_is_safe(&obs, &map, &[(BuildingKind::Turret, exit)],),
        "a paid unfinished producer must retain the doorstep it will need on completion"
    );
}

#[test]
fn combined_layout_preserves_a_deferred_foundry_future_egress() {
    let foundry_anchor = TilePos::new(10, 10);
    let exit = TilePos::new(12, 10);
    let safe_site = TilePos::new(25, 5);
    let terrain = |tile: TilePos| {
        let inside_foundry = footprint_contains(
            foundry_anchor,
            BuildingKind::Foundry.base_stats().size,
            tile,
        );
        if inside_foundry || tile.y == 10 && tile.x >= exit.x || tile.chebyshev(safe_site) <= 1 {
            '.'
        } else {
            '^'
        }
    };
    let scenario = scenario_with(terrain);
    let map = PublicMapBriefing::from_scenario(&scenario).expect("future-egress briefing");
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.known_peaks = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
        .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
        .collect();
    obs.known_rock = obs.known_peaks.clone();
    obs.my_units[0].founding = Some((BuildingKind::Foundry, foundry_anchor));
    let policy = UtilityPolicy::new();

    assert!(
        policy.combined_build_layout_is_safe(&obs, &map, &[(BuildingKind::Turret, safe_site)],)
    );
    assert!(
        !policy.combined_build_layout_is_safe(&obs, &map, &[(BuildingKind::Turret, exit)],),
        "a deferred producer must retain the doorstep it will need once its site exists"
    );
}

fn split_component_planned_foundry_fixture() -> (Observation, PublicMapBriefing) {
    let foundry_anchor = TilePos::new(18, 18);
    let terrain = |tile: TilePos| {
        let inside_foundry = footprint_contains(
            foundry_anchor,
            BuildingKind::Foundry.base_stats().size,
            tile,
        );
        let upper_component = tile.x == 17 && tile.y <= 17;
        let lower_component = tile.x == 17 && tile.y >= 20;
        if inside_foundry || upper_component || lower_component {
            '.'
        } else {
            '^'
        }
    };
    let scenario = scenario_with(terrain);
    let map = PublicMapBriefing::from_scenario(&scenario).expect("split-component briefing");
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.known_peaks = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
        .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
        .collect();
    obs.known_rock = obs.known_peaks.clone();
    (obs, map)
}

#[test]
fn combined_layout_rejects_a_cut_off_authoritative_planned_spawn() {
    let (obs, map) = split_component_planned_foundry_fixture();
    let policy = UtilityPolicy::new();
    let foundry = (BuildingKind::Foundry, TilePos::new(18, 18));
    let lower_blocker = (BuildingKind::Turret, TilePos::new(17, 21));

    assert!(policy.combined_build_layout_is_safe(&obs, &map, &[foundry]));
    assert!(policy.combined_build_layout_is_safe(&obs, &map, &[lower_blocker]));
    assert!(
        !policy.combined_build_layout_is_safe(&obs, &map, &[foundry, lower_blocker]),
        "the radial doorstep is in the lower component even though row-major visits the upper component first"
    );
}

#[test]
fn combined_layout_keeps_a_safe_authoritative_planned_spawn() {
    let (obs, map) = split_component_planned_foundry_fixture();
    let policy = UtilityPolicy::new();
    let foundry = (BuildingKind::Foundry, TilePos::new(18, 18));
    let upper_blocker = (BuildingKind::Turret, TilePos::new(17, 16));

    assert!(policy.combined_build_layout_is_safe(&obs, &map, &[foundry]));
    assert!(policy.combined_build_layout_is_safe(&obs, &map, &[upper_blocker]));
    assert!(
        policy.combined_build_layout_is_safe(&obs, &map, &[foundry, upper_blocker]),
        "cutting the row-major component must not reject a layout whose radial doorstep remains connected"
    );
}

#[test]
fn planned_producer_doorstep_tie_breaks_in_the_authoritative_frame() {
    let obs = observation(PlayerId(0), LEFT_HOME);
    let map = briefing();
    let ground = GroundKnowledge::new(
        crate::bot::query_work::QueryPurpose::NavigationTest,
        &obs,
        &map,
        &[],
    );
    let footprint = PlacementFootprint {
        anchor: TilePos::new(8, 11),
        size: BuildingKind::Foundry.base_stats().size,
        blocks_ground: true,
    };
    let orientation = Orientation::for_home(&obs, TilePos::new(WIDTH - 1, 0));

    assert_eq!(
        planned_ground_producer_spawn_doorstep(
            &ground,
            footprint,
            Some(footprint),
            Some(orientation),
        ),
        Some(TilePos::new(7, 10)),
        "the mirrored policy must retain the world's top-side cross-product tie-break"
    );
    assert_eq!(
        planned_ground_producer_spawn_doorstep(&ground, footprint, Some(footprint), None),
        Some(TilePos::new(7, 13)),
        "running the same key in policy coordinates would choose the opposite tied doorstep"
    );
}

#[test]
fn combined_layout_revalidates_the_exact_builder_against_remembered_danger() {
    let map = briefing();
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.visible.fill(true);
    obs.my_units[0].tile = TilePos::new(8, 10);
    let build = (BuildingKind::Turret, TilePos::new(25, 10), UnitId(1));
    let policy = UtilityPolicy::new();
    let orientation = Orientation::for_home(&obs, LEFT_HOME);
    assert!(policy.combined_build_layout_with_builders_is_safe(
        &obs,
        &map,
        &[],
        &[],
        orientation,
        &[build],
    ));

    let remembered = UnitContact {
        id: UnitId(20),
        player: PlayerId(1),
        kind: UnitKind::Sentinel,
        tile: TilePos::new(16, 10),
        hp: UnitKind::Sentinel.stats().max_hp,
        grounded: false,
        last_seen: obs.tick,
        evidence: ContactEvidence::Remembered,
    };
    assert!(
        !policy.combined_build_layout_with_builders_is_safe(
            &obs,
            &map,
            std::slice::from_ref(&remembered),
            &[],
            orientation,
            &[build],
        ),
        "a frozen builder whose exact command route enters remembered danger is unsafe"
    );
}

#[test]
fn shared_construction_checks_respect_eligibility_origins_and_contested_memory() {
    let dangerous_gap = TilePos::new(20, 4);
    let safe_gap = TilePos::new(20, 22);
    let scenario = scenario_with(|tile| {
        if tile.x == 20 && tile != dangerous_gap && tile != safe_gap {
            '~'
        } else {
            '.'
        }
    });
    let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let anchor = TilePos::new(28, 10);
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.my_units = vec![
        unit(1, obs.me, UnitKind::Harvester, TilePos::new(8, 10)),
        unit(2, obs.me, UnitKind::Harvester, TilePos::new(8, 22)),
    ];
    obs.visible.fill(true);
    let mut policy = UtilityPolicy::new();
    for contested in [false, true, false] {
        policy.contested_harvest_regions = if contested {
            vec![ContestedHarvestRegion {
                center: dangerous_gap,
                last_evidence: obs.tick,
                sweep_started_at: None,
            }]
        } else {
            Vec::new()
        };
        let mut checks = ConstructionChecks::new(
            QueryPurpose::NavigationTest,
            &policy,
            &obs,
            &map,
            &[],
            &[],
            Orientation::for_home(&obs, LEFT_HOME),
        );
        let builders = obs.my_units.iter().collect::<Vec<_>>();
        assert_eq!(
            checks.safe_implicit_builder(BuildingKind::Turret, anchor, &builders),
            Some(UnitId(if contested { 2 } else { 1 }))
        );
        assert_eq!(
            checks.safe_implicit_builder(BuildingKind::Turret, anchor, &builders[1..]),
            Some(UnitId(2))
        );
        assert_eq!(
            checks.safe_implicit_builder(BuildingKind::Turret, anchor, &builders[..1]),
            (!contested).then_some(UnitId(1))
        );
        assert_eq!(
            checks.safe_implicit_builder(BuildingKind::Turret, anchor, &[]),
            None
        );
        let distance = checks
            .builder_travel_cost(&obs.my_units[1], BuildingKind::Turret, anchor)
            .unwrap();
        let mut cursor = obs.my_units[1].clone();
        cursor.tile = anchor.offset(-1, 0);
        let local_distance = checks
            .builder_travel_cost(&cursor, BuildingKind::Turret, anchor)
            .unwrap();
        assert!(local_distance < distance);
        assert_eq!(
            checks.builder_travel_cost(&obs.my_units[1], BuildingKind::Turret, anchor),
            Some(distance)
        );
    }
}

#[test]
fn resource_access_evidence_is_shared_and_invalidates_on_effective_inputs() {
    let node = TilePos::new(12, 10);
    let scenario = scenario_with(|tile| if tile == node { 's' } else { '.' });
    let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    obs.explored.fill(false);
    obs.my_units[0].harvesting = Some(node);
    let mut policy = UtilityPolicy::new();
    let mut ground = GroundKnowledge::new(
        crate::bot::query_work::QueryPurpose::NavigationTest,
        &obs,
        &map,
        &[],
    );
    let (baseline, cold) =
        crate::bot::navigation::work::measure(|| scrap_assets(&policy, &ground, &[LEFT_HOME]));
    assert!(!baseline.is_empty());
    assert!(cold.searches > 0);
    let (reused, warm) =
        crate::bot::navigation::work::measure(|| scrap_assets(&policy, &ground, &[LEFT_HOME]));
    assert_eq!(reused, baseline);
    assert_eq!(warm.searches, 0);

    for y in 0..HEIGHT {
        ground.ground_blocked[(y * WIDTH + 8) as usize] = true;
    }
    assert!(scrap_assets(&policy, &ground, &[LEFT_HOME]).is_empty());
    let mut ground = GroundKnowledge::new(
        crate::bot::query_work::QueryPurpose::NavigationTest,
        &obs,
        &map,
        &[],
    );
    assert_eq!(scrap_assets(&policy, &ground, &[LEFT_HOME]), baseline);
    *ground.scrap.get_mut(&node).unwrap() *= 2;
    let (richer, repriced) =
        crate::bot::navigation::work::measure(|| scrap_assets(&policy, &ground, &[LEFT_HOME]));
    assert_eq!(repriced.searches, 0);
    assert!(richer[0].scrap > baseline[0].scrap);
    assert_eq!(
        richer,
        scrap_assets(&UtilityPolicy::new(), &ground, &[LEFT_HOME])
    );
    ground.scrap.remove(&node);
    assert!(scrap_assets(&policy, &ground, &[LEFT_HOME]).is_empty());

    let ground = GroundKnowledge::new(
        crate::bot::query_work::QueryPurpose::NavigationTest,
        &obs,
        &map,
        &[],
    );
    assert_eq!(scrap_assets(&policy, &ground, &[LEFT_HOME]), baseline);
    assert!(scrap_assets(&policy, &ground, &[]).is_empty());
    assert_eq!(scrap_assets(&policy, &ground, &[LEFT_HOME]), baseline);
    policy.dead_nodes.push(node);
    assert!(scrap_assets(&policy, &ground, &[LEFT_HOME]).is_empty());
    policy.dead_nodes.clear();
    assert_eq!(scrap_assets(&policy, &ground, &[LEFT_HOME]), baseline);
    let mut idle = obs.clone();
    idle.my_units[0].harvesting = None;
    let idle_ground = GroundKnowledge::new(
        crate::bot::query_work::QueryPurpose::NavigationTest,
        &idle,
        &map,
        &[],
    );
    assert!(scrap_assets(&policy, &idle_ground, &[LEFT_HOME]).is_empty());
}

#[test]
fn reachable_scrap_near_owned_foundries_is_defensible_on_every_front() {
    let rear = TilePos::new(2, 10);
    let front = TilePos::new(12, 10);
    let enemy_side = TilePos::new(31, 10);
    let scenario = scenario_with(|tile| {
        if [rear, front, enemy_side].contains(&tile) {
            's'
        } else {
            '.'
        }
    });
    let map = PublicMapBriefing::from_scenario(&scenario).expect("scrap briefing");
    let mut obs = observation(PlayerId(0), LEFT_HOME);
    let expansion = TilePos::new(24, 10);
    obs.my_buildings
        .push(building(2, PlayerId(0), BuildingKind::Foundry, expansion));
    obs.my_queues.push(Vec::new());
    for (id, tile, target) in [
        (2, rear.offset(-1, 0), rear),
        (3, front.offset(-1, 0), front),
        (4, enemy_side.offset(-1, 0), enemy_side),
    ] {
        let mut worker = unit(id, PlayerId(0), UnitKind::Harvester, tile);
        worker.idle = false;
        worker.harvesting = Some(target);
        obs.my_units.push(worker);
    }
    obs.explored.fill(false);
    let starts: Vec<_> = map.hostile_starting_foundries(obs.me).copied().collect();
    let ground = GroundKnowledge::new(
        crate::bot::query_work::QueryPurpose::NavigationTest,
        &obs,
        &map,
        &starts,
    );
    let assets = scrap_assets(&UtilityPolicy::new(), &ground, &[LEFT_HOME, expansion]);
    let defended: BTreeSet<_> = assets
        .iter()
        .flat_map(|asset| asset.tiles.iter().copied())
        .collect();

    assert!(defended.contains(&rear));
    assert!(defended.contains(&front));
    assert!(
        defended.contains(&enemy_side),
        "an exposed cluster remains worth protecting when an owned expansion can serve it"
    );
}
