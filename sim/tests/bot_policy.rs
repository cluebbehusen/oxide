//! Utility-policy contracts: deterministic thinking and budget honesty.

use chassis::grid::TilePos;
use oxide_sim::bot::{
    BuildingObs, Dials, DifficultyTuning, Executive, Intent, Observation, PublicMapBriefing,
    UnitObs, UtilityPolicy,
};
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance, PlayerSpec};
use oxide_sim::stats::{BuildingKind, EXTRACTOR_SUPPORT_RADIUS};
use oxide_sim::{BuildingId, Command, Event, Faction, PlayerId, Scenario, UnitId, UnitKind};

fn standard_dials() -> Dials {
    let profile = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 0x5eed_0a16)
        .resolve_profile();
    Dials::scripted(
        &profile,
        DifficultyTuning::for_level(BotDifficulty::Standard),
    )
}

fn standard_dials_without_opening_core_floor() -> Dials {
    let mut dials = standard_dials();
    dials.minimum_core_equivalents = 0;
    dials
}

fn observed_unit(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
    UnitObs {
        id: UnitId(id),
        player: PlayerId(0),
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

fn observed_building(id: u32, kind: BuildingKind, anchor: TilePos, built: bool) -> BuildingObs {
    BuildingObs {
        id: BuildingId(id),
        player: PlayerId(0),
        kind,
        anchor,
        hp: kind.base_stats().max_hp,
        built,
        seen: true,
        tier: 0,
    }
}

fn construction_observation(scrap: u32) -> Observation {
    let map_width = 48;
    let map_height = 24;
    let mut units: Vec<UnitObs> = (0..7)
        .map(|id| {
            observed_unit(
                id,
                UnitKind::Harvester,
                TilePos::new(
                    5 + i32::try_from(id % 3).unwrap(),
                    9 + i32::try_from(id / 3).unwrap(),
                ),
            )
        })
        .collect();
    units.extend((0..3).map(|offset| {
        observed_unit(
            20 + offset,
            UnitKind::Sentinel,
            TilePos::new(7 + i32::try_from(offset).unwrap(), 14),
        )
    }));
    Observation {
        version: oxide_sim::bot::observation::OBSERVATION_VERSION,
        tick: 2_016,
        me: PlayerId(0),
        scrap,
        map_width,
        map_height,
        my_units: units,
        my_buildings: Vec::new(),
        my_queues: Vec::new(),
        ally_units: Vec::new(),
        ally_buildings: Vec::new(),
        enemy_units: Vec::new(),
        enemy_buildings: Vec::new(),
        visible: vec![true; usize::try_from(map_width * map_height).unwrap()],
        explored: vec![true; usize::try_from(map_width * map_height).unwrap()],
        known_scrap: Vec::new(),
        known_rock: Vec::new(),
        known_frames: Vec::new(),
        known_peaks: Vec::new(),
        known_wrecks: Vec::new(),
        salvage_incidents: Vec::new(),
        blips: Vec::new(),
        faction: Faction::Ferrous,
        my_shells: 0,
        incoming_shells: Vec::new(),
    }
}

fn public_map(obs: &Observation) -> PublicMapBriefing {
    let width = usize::try_from(obs.map_width).expect("the test map has a positive width");
    let height = usize::try_from(obs.map_height).expect("the test map has a positive height");
    assert!(width >= 2 && height >= 2);
    let mut map = vec![".".repeat(width); height];
    map[0].replace_range(..1, "1");
    PublicMapBriefing::from_scenario(&Scenario {
        name: "policy test map".into(),
        seed: 0,
        map,
        players: vec![PlayerSpec {
            name: "test seat".into(),
            faction: obs.faction,
            team: None,
            scrap: 0,
            bot: false,
            bot_config: None,
        }],
        units: Vec::new(),
        buildings: Vec::new(),
        meta: None,
    })
    .expect("the focused observation has a matching public map")
}

fn add_building(obs: &mut Observation, building: BuildingObs) {
    obs.my_buildings.push(building);
    obs.my_queues.push(Vec::new());
}

fn player_facing_intents(dials: &Dials, obs: &Observation) -> Vec<Intent> {
    UtilityPolicy::new().think_player_facing(dials, obs, &[], &[], &[], &public_map(obs))
}

fn planned_cost(intent: &Intent) -> u32 {
    match intent {
        Intent::TrainAt { kind, .. } => kind.stats().cost,
        Intent::Build { kind, .. } | Intent::BuildWith { kind, .. } => kind
            .base_stats()
            .construction
            .map_or(0, |construction| construction.cost),
        _ => 0,
    }
}

fn plans_build(intents: &[Intent], kind: BuildingKind, anchor: TilePos) -> bool {
    intents.iter().any(|intent| {
        matches!(
            intent,
            Intent::Build {
                kind: planned_kind,
                anchor: planned_anchor,
            } | Intent::BuildWith {
                kind: planned_kind,
                anchor: planned_anchor,
                ..
            } if *planned_kind == kind && *planned_anchor == anchor
        )
    })
}

fn exact_builder_for(intents: &[Intent], kind: BuildingKind, anchor: TilePos) -> Option<UnitId> {
    intents.iter().find_map(|intent| match intent {
        Intent::BuildWith {
            builder,
            kind: planned_kind,
            anchor: planned_anchor,
        } if *planned_kind == kind && *planned_anchor == anchor => Some(*builder),
        _ => None,
    })
}

fn footprint_distance(first: (TilePos, (i32, i32)), second: (TilePos, (i32, i32))) -> i32 {
    let axis = |a: i32, a_len: i32, b: i32, b_len: i32| {
        let a_far = a + a_len - 1;
        let b_far = b + b_len - 1;
        (a - b_far).max(b - a_far).max(0)
    };
    axis(first.0.x, first.1.0, second.0.x, second.1.0)
        .max(axis(first.0.y, first.1.1, second.0.y, second.1.1))
}

fn block_support_anchors(obs: &mut Observation, extractor: TilePos, through_radius: i32) {
    for y in 0..obs.map_height {
        for x in 0..obs.map_width {
            let tile = TilePos::new(x, y);
            let radius = tile.chebyshev(extractor);
            if (2..=through_radius).contains(&radius) && y != extractor.y {
                obs.known_rock.push(tile);
            }
        }
    }
    obs.known_rock.sort_by_key(|tile| (tile.y, tile.x));
    obs.known_rock.dedup();
}

#[test]
fn identical_inputs_think_identical_intents() {
    let scenario = Scenario::skirmish();
    let state = scenario.build().unwrap();
    let obs = Observation::omniscient(&state, PlayerId(0));
    let dials = Dials::full();
    let mut first = UtilityPolicy::new();
    let mut second = UtilityPolicy::new();
    assert_eq!(
        first.think(&dials, &obs, &[], &[]),
        second.think(&dials, &obs, &[], &[]),
        "a policy is a function of (dials, observation, executive)"
    );
}

#[test]
fn a_think_never_plans_past_the_bank() {
    let scenario = Scenario::skirmish();
    let state = scenario.build().unwrap();
    for player in [0u8, 1] {
        let me = PlayerId(player);
        let obs = Observation::omniscient(&state, me);
        let mut policy = UtilityPolicy::new();
        let intents = policy.think(&Dials::full(), &obs, &[], &[]);
        let planned: u32 = intents.iter().map(planned_cost).sum();
        assert!(
            planned <= obs.scrap,
            "priced intents ({planned}) exceed the bank ({})",
            obs.scrap
        );
    }
}

#[test]
fn scouts_do_not_create_sticky_anti_air_demand_but_armed_flyers_do() {
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(100, BuildingKind::Foundry, TilePos::new(4, 10), true),
    );
    add_building(
        &mut obs,
        observed_building(101, BuildingKind::Fabricator, TilePos::new(10, 10), true),
    );
    obs.known_scrap.push((TilePos::new(7, 8), 500));
    let mut policy = UtilityPolicy::new();
    let mut dials = standard_dials_without_opening_core_floor();
    dials.deep_tech = false;
    dials.radar = false;
    dials.mines = false;
    dials.expansion = false;
    dials.extractors = false;
    dials.turret_response = false;
    dials.upgrades = false;
    let aa_kind = oxide_sim::stats::Role::AntiAir.unit_for(obs.faction);
    let has_aa_response = |intents: &[Intent]| {
        intents.iter().any(|intent| {
            matches!(intent, Intent::TrainAt { kind, .. } if *kind == aa_kind)
                || matches!(
                    intent,
                    Intent::Build {
                        kind: BuildingKind::FlakTurret,
                        ..
                    }
                )
        })
    };

    for scout in [UnitKind::Gnat, UnitKind::Kestrel] {
        obs.enemy_units = vec![UnitObs {
            player: PlayerId(1),
            ..observed_unit(200, scout, TilePos::new(18, 10))
        }];
        let current = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
        assert!(
            !has_aa_response(&current),
            "an unarmed {scout:?} must not divert the economy into AA: {current:?}"
        );

        obs.tick += 24;
        obs.enemy_units.clear();
        let remembered = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
        assert!(
            !has_aa_response(&remembered),
            "seeing {scout:?} must not leave sticky hostile-air evidence: {remembered:?}"
        );
        obs.tick += 24;
    }

    obs.enemy_units = vec![UnitObs {
        player: PlayerId(1),
        ..observed_unit(201, UnitKind::Condor, TilePos::new(18, 10))
    }];
    let current = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert!(
        current
            .iter()
            .any(|intent| matches!(intent, Intent::TrainAt { kind, .. } if *kind == aa_kind)),
        "an armed flyer should request a dedicated AA unit: {current:?}"
    );
    assert!(
        current.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::FlakTurret,
                ..
            }
        )),
        "an armed flyer should raise static AA over known salvage: {current:?}"
    );

    obs.tick += 24;
    obs.enemy_units.clear();
    let remembered = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert!(
        has_aa_response(&remembered),
        "an armed flyer should leave bounded sticky AA evidence: {remembered:?}"
    );
}

#[test]
fn a_starved_commander_liquidates_its_walls_for_one_more_wave() {
    // The scripted salvage doctrine: bank starved, nothing known left
    // to mine or strip, a standing defense — the policy sells
    // cheapest-first. The learner meets the mechanic from the
    // receiving side of exactly this intent.
    use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
    use oxide_sim::stats::BuildingKind;
    let scenario = Scenario {
        name: "starved".into(),
        seed: 5,
        map: vec![
            "################".into(),
            "#..............#".into(),
            "#..1...........#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "F".into(),
                faction: oxide_sim::Faction::Ferrous,
                team: None,
                scrap: 3,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "C".into(),
                faction: oxide_sim::Faction::Cupric,
                team: None,
                scrap: 3,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![UnitSpec {
            player: 0,
            kind: oxide_sim::UnitKind::Harvester,
            x: 6,
            y: 2,
        }],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 8,
                y: 2,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Bastion,
                x: 10,
                y: 2,
            },
        ],
        meta: None,
    };
    let state = scenario.build().unwrap();
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let mut policy = UtilityPolicy::new();
    let intents = policy.think(&Dials::full(), &obs, &[], &[]);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, Intent::Salvage { building } if *building == turret)),
        "starved with dry ground: the cheapest wall goes first, got {intents:?}"
    );
}

#[test]
fn standard_opening_can_train_and_restore_its_supported_frame_from_150_scrap() {
    let scenario = Scenario::skirmish();
    let briefing = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let mut state = scenario.build().unwrap();
    let me = PlayerId(0);
    let dials = standard_dials();
    let mut policy = UtilityPolicy::new();
    let mut executive = Executive::new();

    let first_obs = Observation::fog_honest(&state, me);
    assert_eq!(first_obs.scrap, 150);
    let first_intents = policy.think_player_facing(&dials, &first_obs, &[], &[], &[], &briefing);
    assert!(
        first_intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Harvester,
                ..
            }
        )),
        "the normal opening worker order must remain"
    );
    assert_eq!(
        exact_builder_for(&first_intents, BuildingKind::Extractor, TilePos::new(8, 4)),
        Some(UnitId(0)),
        "the supported home frame must fit beside that worker order with a safe exact builder: \
         {first_intents:?}"
    );
    let planned: u32 = first_intents.iter().map(planned_cost).sum();
    assert_eq!(
        planned, 150,
        "adaptive production must leave exactly the frame restoration fund"
    );

    let mut reports = Vec::new();
    let first_commands = executive.apply_with_reservations(me, &first_obs, &first_intents, &[]);
    assert!(first_commands.iter().any(|command| matches!(
        command.command,
        Command::Train {
            kind: UnitKind::Harvester,
            ..
        }
    )));
    reports.push(state.tick(&first_commands));

    if !state
        .buildings()
        .iter()
        .any(|building| building.player == me && building.kind == BuildingKind::Extractor)
    {
        // On the real opening, harvest assignments may claim every idle
        // worker during the first lowering. Their persistent work makes one
        // available to construction on the next think without changing the
        // economic choice or spending another scrap.
        while !oxide_sim::bot::difficulty::strategic_admission_tick(state.current_tick()) {
            state.tick(&[]);
        }
        let second_obs = Observation::fog_honest(&state, me);
        let second_intents =
            policy.think_player_facing(&dials, &second_obs, &[], &[], &[], &briefing);
        let second_commands =
            executive.apply_with_reservations(me, &second_obs, &second_intents, &[]);
        assert!(second_commands.iter().any(|command| matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Extractor,
                ..
            }
        )));
        reports.push(state.tick(&second_commands));
    }

    assert!(
        reports
            .iter()
            .flat_map(|report| &report.events)
            .all(|event| {
                !matches!(
                    event,
                    Event::CommandRejected {
                        player,
                        ..
                    } if *player == me
                )
            })
    );
    assert!(
        state
            .buildings()
            .iter()
            .any(|building| building.player == me && building.kind == BuildingKind::Extractor),
        "the opening must place the paid Extractor site"
    );
    assert_eq!(
        state.player(me).scrap,
        0,
        "the worker and home Extractor intentionally spend the full opening bank"
    );
}

fn assert_current_emergency_defense_preserves_opening_escrow(
    seat: u8,
    threat_kind: UnitKind,
    defense_kind: BuildingKind,
    intruder_tile: TilePos,
) {
    let mut scenario = Scenario::skirmish();
    let defense_cost = defense_kind
        .base_stats()
        .construction
        .expect("emergency defenses have a construction price")
        .cost;
    let extractor_cost = BuildingKind::Extractor
        .base_stats()
        .construction
        .expect("Extractors have a construction price")
        .cost;
    scenario.players[usize::from(seat)].scrap = 150 + defense_cost;
    let intruder = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player != seat && unit.kind == UnitKind::Sentinel)
        .expect("Skirmish has one opposing Sentinel");
    intruder.kind = threat_kind;
    (intruder.x, intruder.y) = (intruder_tile.x, intruder_tile.y);

    let briefing = PublicMapBriefing::from_scenario(&scenario).expect("the briefing builds");
    let mut state = scenario.build().expect("the threatened opening builds");
    let me = PlayerId(seat);
    let dials = standard_dials();
    let mut policy = UtilityPolicy::new();
    let mut executive = Executive::new();

    let observation = Observation::fog_honest(&state, me);
    assert!(
        observation
            .enemy_units
            .iter()
            .any(|unit| unit.kind == threat_kind && observation.visible(unit.tile)),
        "the {threat_kind:?} must be current visible evidence for the emergency exception"
    );
    let intents = policy.think_player_facing(&dials, &observation, &[], &[], &[], &briefing);
    assert_eq!(
        intents
            .iter()
            .filter(|intent| matches!(
                intent,
                Intent::Build { kind, .. } | Intent::BuildWith { kind, .. }
                    if *kind == defense_kind
            ))
            .count(),
        1,
        "the visible opening {threat_kind:?} should admit exactly one emergency {defense_kind:?}: {intents:?}"
    );
    assert!(
        intents.iter().all(|intent| match intent {
            Intent::Build { kind, .. } | Intent::BuildWith { kind, .. } => {
                *kind == defense_kind
            }
            Intent::Upgrade { .. } => false,
            _ => true,
        }),
        "the current emergency exception must not admit other below-floor capital: {intents:?}"
    );
    assert!(intents.iter().any(|intent| matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Harvester,
            ..
        }
    )));
    assert_eq!(
        intents.iter().map(planned_cost).sum::<u32>(),
        observation.scrap - extractor_cost,
        "the emergency and fourth worker may spend only the bank above the home Extractor fund: {intents:?}"
    );

    let commands = executive.apply_with_reservations(me, &observation, &intents, &[]);
    let report = state.tick(&commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player,
            ..
        } if *player == me
    )));
    assert_eq!(
        state.player(me).scrap,
        defense_cost + extractor_cost,
        "the worker is paid immediately while both construction promises remain in the bank"
    );

    let mut defense_anchor = commands.iter().find_map(|command| match command.command {
        Command::Build { kind, anchor, .. } if kind == defense_kind => Some(anchor),
        _ => None,
    });
    let mut extractor_anchor = None;
    let mut defense_site = None;
    let mut extractor_site = None;
    let mut defense_advanced = false;
    let mut extractor_advanced = false;

    for _ in 0..4_000 {
        let commands = if oxide_sim::bot::difficulty::strategic_admission_tick(state.current_tick())
        {
            let observation = Observation::fog_honest(&state, me);
            if extractor_site.is_none() {
                assert!(
                    observation.scrap >= extractor_cost,
                    "emergency defense must preserve the home Extractor fund until its site is paid"
                );
            }
            let intents =
                policy.think_player_facing(&dials, &observation, &[], &[], &[], &briefing);
            executive.apply_with_reservations(me, &observation, &intents, &[])
        } else {
            Vec::new()
        };
        for command in &commands {
            let (kind, anchor) = match command.command {
                Command::Build { kind, anchor, .. } => (kind, anchor),
                _ => continue,
            };
            let promised = if kind == defense_kind {
                &mut defense_anchor
            } else if kind == BuildingKind::Extractor {
                &mut extractor_anchor
            } else {
                continue;
            };
            if let Some(previous) = *promised {
                assert_eq!(
                    anchor, previous,
                    "lowering must not oscillate an unpaid {kind:?} promise between sites"
                );
            } else {
                *promised = Some(anchor);
            }
        }
        let report = state.tick(&commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            Event::CommandRejected {
                player,
                ..
            } if *player == me
        )));
        for (kind, promised_anchor, site, advanced) in [
            (
                defense_kind,
                defense_anchor,
                &mut defense_site,
                &mut defense_advanced,
            ),
            (
                BuildingKind::Extractor,
                extractor_anchor,
                &mut extractor_site,
                &mut extractor_advanced,
            ),
        ] {
            let owned: Vec<_> = state
                .buildings()
                .iter()
                .filter(|building| building.player == me && building.kind == kind)
                .collect();
            assert!(
                owned.len() <= 1,
                "the opening must not pay for duplicate {kind:?} promises: {owned:?}"
            );
            let Some(building) = owned.first().copied() else {
                continue;
            };
            assert_eq!(
                Some(building.anchor),
                promised_anchor,
                "the paid site must be the stable promised site"
            );
            if let Some(previous) = *site {
                assert_eq!(building.id, previous, "a paid site must remain stable");
            } else {
                *site = Some(building.id);
            }
            *advanced |= building.built || building.hp > kind.base_stats().max_hp / 5;
        }
        if defense_site.is_some()
            && extractor_site.is_some()
            && defense_advanced
            && extractor_advanced
        {
            break;
        }
    }
    assert!(
        defense_site.is_some() && defense_advanced,
        "the emergency defense promise must become one accepted, progressing paid site"
    );
    assert!(
        extractor_site.is_some() && extractor_advanced,
        "the reserved home Extractor must become one accepted, progressing paid site"
    );
}

#[test]
fn current_ground_and_air_defenses_preserve_the_fourth_worker_and_home_extractor_fund() {
    for (seat, threat_kind, defense_kind, intruder_tile) in [
        (
            0,
            UnitKind::Sentinel,
            BuildingKind::Turret,
            TilePos::new(14, 4),
        ),
        (
            1,
            UnitKind::Condor,
            BuildingKind::FlakTurret,
            TilePos::new(27, 10),
        ),
    ] {
        assert_current_emergency_defense_preserves_opening_escrow(
            seat,
            threat_kind,
            defense_kind,
            intruder_tile,
        );
    }
}

#[test]
fn noncurrent_air_evidence_cannot_spend_below_floor_opening_escrow() {
    let mut scenario = Scenario::skirmish();
    let flak_cost = BuildingKind::FlakTurret
        .base_stats()
        .construction
        .expect("Flak has a construction price")
        .cost;
    scenario.players[0].scrap = 150 + flak_cost;
    let hidden_intruder = scenario
        .units
        .iter_mut()
        .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
        .expect("Skirmish has one enemy Sentinel");
    hidden_intruder.kind = UnitKind::Moth;

    let briefing = PublicMapBriefing::from_scenario(&scenario).expect("the briefing builds");
    let mut state = scenario.build().expect("the hidden-threat opening builds");
    let me = PlayerId(0);
    let observation = Observation::fog_honest(&state, me);
    assert!(
        observation.enemy_units.is_empty(),
        "the distant Moth must remain noncurrent evidence in the opening observation"
    );

    let dials = standard_dials();
    let mut policy = UtilityPolicy::new();
    let intents = policy.think_player_facing(&dials, &observation, &[], &[], &[], &briefing);
    assert!(
        intents.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Turret | BuildingKind::FlakTurret,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Turret | BuildingKind::FlakTurret,
                ..
            }
        )),
        "hidden air evidence must not admit either emergency defense: {intents:?}"
    );
    assert!(
        intents.iter().all(|intent| match intent {
            Intent::Build { kind, .. } | Intent::BuildWith { kind, .. } => {
                *kind == BuildingKind::Extractor
            }
            Intent::Upgrade { .. } => false,
            _ => true,
        }),
        "surplus Flak money must not unlock capital beyond the safe home Extractor: {intents:?}"
    );
    assert!(intents.iter().any(|intent| matches!(
        intent,
        Intent::Build {
            kind: BuildingKind::Extractor,
            ..
        } | Intent::BuildWith {
            kind: BuildingKind::Extractor,
            ..
        }
    )));
    assert!(intents.iter().any(|intent| matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Harvester,
            ..
        }
    )));

    let commands = Executive::new().apply_with_reservations(me, &observation, &intents, &[]);
    assert!(commands.iter().all(|command| !matches!(
        command.command,
        Command::Build {
            kind: BuildingKind::Turret | BuildingKind::FlakTurret,
            ..
        }
    )));
    let report = state.tick(&commands);
    assert!(report.events.iter().all(|event| !matches!(
        event,
        Event::CommandRejected {
            player: PlayerId(0),
            ..
        }
    )));
    assert!(state.buildings().iter().all(|building| {
        building.player != me
            || !matches!(
                building.kind,
                BuildingKind::Turret | BuildingKind::FlakTurret
            )
    }));
}

#[test]
fn prime_opening_core_blocks_optional_capital_until_the_floor_is_projected() {
    let home = TilePos::new(2, 10);
    let mut obs = construction_observation(10_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 0x5eed_0a16)
        .resolve_profile();
    let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));

    let mut policy = UtilityPolicy::new();
    let deficient = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert_eq!(
        deficient
            .iter()
            .filter(|intent| matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Sentinel,
                    ..
                }
            ))
            .count(),
        2,
        "the shallow Foundry queue should absorb two of the five missing equivalents: {deficient:?}"
    );
    assert!(deficient.iter().all(|intent| !matches!(
        intent,
        Intent::Build { .. } | Intent::BuildWith { .. } | Intent::Upgrade { .. }
    )));

    obs.my_units.extend((23..28).map(|id| {
        observed_unit(
            id,
            UnitKind::Sentinel,
            TilePos::new(10 + i32::try_from(id - 23).unwrap(), 14),
        )
    }));
    obs.my_units.sort_unstable_by_key(|unit| unit.id);
    let ready =
        UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert!(
        ready.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Fabricator,
                ..
            }
        )),
        "the same bank may fund the first voluntary tech project once Prime has eight equivalents: {ready:?}"
    );
}

#[test]
fn reaching_the_core_floor_after_cancelling_an_unsafe_site_does_not_cancel_twice() {
    let home = TilePos::new(2, 10);
    let site = BuildingId(1);
    let site_anchor = TilePos::new(12, 10);
    let mut obs = construction_observation(10_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    add_building(
        &mut obs,
        observed_building(site.0, BuildingKind::Turret, site_anchor, false),
    );
    obs.my_units.extend((23..27).map(|id| {
        observed_unit(
            id,
            UnitKind::Sentinel,
            TilePos::new(10 + i32::try_from(id - 23).unwrap(), 14),
        )
    }));
    let mut threat = observed_unit(50, UnitKind::Sentinel, site_anchor.offset(1, 0));
    threat.player = PlayerId(1);
    obs.enemy_units.push(threat);
    obs.my_units.sort_unstable_by_key(|unit| unit.id);

    let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 0x5eed_0a16)
        .resolve_profile();
    let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
    let intents = player_facing_intents(&dials, &obs);

    assert_eq!(
        intents
            .iter()
            .filter(|intent| matches!(intent, Intent::CancelSite { building } if *building == site))
            .count(),
        1,
        "one construction pass owns the unsafe-site cancellation: {intents:?}"
    );
    assert!(intents.iter().any(|intent| matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Sentinel,
            ..
        }
    )));
}

#[test]
fn capital_guard_accepts_a_shallow_sentinel_an_exact_remainder_or_no_ground_objective() {
    let home = TilePos::new(2, 10);
    let mut obs = construction_observation(0);
    obs.my_units.extend([
        observed_unit(23, UnitKind::Sentinel, TilePos::new(10, 14)),
        observed_unit(24, UnitKind::Sentinel, TilePos::new(11, 14)),
    ]);
    obs.my_units.sort_unstable_by_key(|unit| unit.id);
    for (id, kind, anchor) in [
        (0, BuildingKind::Foundry, home),
        (1, BuildingKind::Fabricator, TilePos::new(7, 3)),
        (2, BuildingKind::Airworks, TilePos::new(12, 3)),
    ] {
        add_building(&mut obs, observed_building(id, kind, anchor, true));
    }
    obs.my_queues[0] = vec![UnitKind::Harvester, UnitKind::Harvester];
    obs.my_queues[1] = vec![UnitKind::Lancer, UnitKind::Lancer];
    obs.my_queues[2] = vec![UnitKind::Kestrel, UnitKind::Kestrel];
    let mut hostile = observed_building(50, BuildingKind::Foundry, TilePos::new(40, 10), true);
    hostile.player = PlayerId(1);
    obs.enemy_buildings.push(hostile);

    let mut dials = standard_dials();
    dials.turret_response = false;
    dials.aa_response = false;
    dials.radar = false;
    dials.reclaimers = false;
    dials.extractors = false;
    dials.upgrades = false;
    dials.expansion = false;
    dials.mines = false;
    dials.barricade_cap = 0;
    let crucible_cost = BuildingKind::Crucible
        .base_stats()
        .construction
        .expect("Crucibles have a construction price")
        .cost;
    let sentinel_cost = UnitKind::Sentinel.stats().cost;
    let plans_crucible = |world: &Observation| {
        player_facing_intents(&dials, world).iter().any(|intent| {
            matches!(
                intent,
                Intent::Build {
                    kind: BuildingKind::Crucible,
                    ..
                }
            )
        })
    };

    obs.scrap = crucible_cost + sentinel_cost - 1;
    assert!(!plans_crucible(&obs));
    obs.scrap += 1;
    assert!(plans_crucible(&obs));

    obs.scrap = crucible_cost;
    obs.my_queues[0][1] = UnitKind::Sentinel;
    assert!(plans_crucible(&obs));

    obs.my_queues[0][1] = UnitKind::Harvester;
    obs.enemy_buildings.clear();
    assert!(plans_crucible(&obs));
}

#[test]
fn post_floor_capital_keeps_the_foundry_actively_reinforcing() {
    let home = TilePos::new(2, 10);
    let mut obs = construction_observation(0);
    obs.my_units.extend([
        observed_unit(23, UnitKind::Sentinel, TilePos::new(10, 14)),
        observed_unit(24, UnitKind::Sentinel, TilePos::new(11, 14)),
    ]);
    obs.my_units.sort_unstable_by_key(|unit| unit.id);
    for (id, kind, anchor) in [
        (0, BuildingKind::Foundry, home),
        (1, BuildingKind::Fabricator, TilePos::new(7, 3)),
        (2, BuildingKind::Airworks, TilePos::new(12, 3)),
    ] {
        add_building(&mut obs, observed_building(id, kind, anchor, true));
    }
    let mut hostile = observed_building(50, BuildingKind::Foundry, TilePos::new(40, 10), true);
    hostile.player = PlayerId(1);
    obs.enemy_buildings.push(hostile);

    let mut dials = standard_dials();
    dials.turret_response = false;
    dials.aa_response = false;
    dials.radar = false;
    dials.reclaimers = false;
    dials.extractors = false;
    dials.upgrades = false;
    dials.expansion = false;
    dials.mines = false;
    dials.barricade_cap = 0;
    let crucible_cost = BuildingKind::Crucible
        .base_stats()
        .construction
        .expect("Crucibles have a construction price")
        .cost;
    let sentinel_cost = UnitKind::Sentinel.stats().cost;

    obs.scrap = crucible_cost + sentinel_cost;
    let funded = player_facing_intents(&dials, &obs);
    let reinforcement = funded
        .iter()
        .position(|intent| {
            matches!(
                intent,
                Intent::TrainAt {
                    building: BuildingId(0),
                    kind: UnitKind::Sentinel,
                }
            )
        })
        .expect("an empty Foundry receives the ring-fenced reinforcement");
    let capital = funded
        .iter()
        .position(|intent| {
            matches!(
                intent,
                Intent::Build {
                    kind: BuildingKind::Crucible,
                    ..
                }
            )
        })
        .expect("the remaining exact capital fund stays spendable");
    assert!(
        reinforcement < capital,
        "production must precede capital: {funded:?}"
    );

    obs.scrap = crucible_cost;
    let short = player_facing_intents(&dials, &obs);
    assert!(
        short.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Sentinel,
                ..
            } | Intent::Build {
                kind: BuildingKind::Crucible,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Crucible,
                ..
            }
        )),
        "the partial combined fund must accumulate instead of starving either purchase: {short:?}"
    );

    obs.scrap = crucible_cost;
    obs.my_queues[0] = vec![UnitKind::Harvester, UnitKind::Sentinel];
    let already_shallow = player_facing_intents(&dials, &obs);
    assert_eq!(
        already_shallow
            .iter()
            .filter(|intent| matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Sentinel,
                    ..
                }
            ))
            .count(),
        0,
        "an existing shallow order must not be duplicated: {already_shallow:?}"
    );
    assert!(already_shallow.iter().any(|intent| matches!(
        intent,
        Intent::Build {
            kind: BuildingKind::Crucible,
            ..
        }
    )));
}

#[test]
fn player_facing_restoration_waits_for_the_full_frame_footprint() {
    let home = TilePos::new(2, 10);
    let frame = TilePos::new(8, 10);
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    obs.known_frames.push(frame);
    let hidden = frame.offset(1, 1);
    let hidden_index = usize::try_from(hidden.y * obs.map_width + hidden.x).unwrap();
    obs.explored[hidden_index] = false;
    let dials = standard_dials_without_opening_core_floor();

    let partial = player_facing_intents(&dials, &obs);
    assert!(
        !plans_build(&partial, BuildingKind::Extractor, frame),
        "a known anchor is not enough to promise an unseen 2x2 footprint: {partial:?}"
    );

    obs.explored[hidden_index] = true;
    let complete = player_facing_intents(&dials, &obs);
    assert_eq!(
        exact_builder_for(&complete, BuildingKind::Extractor, frame),
        Some(UnitId(5)),
        "the restoration becomes legal with the nearest safe exact builder once every footprint \
         tile is known: {complete:?}"
    );
}

#[test]
fn player_facing_restoration_waits_for_a_visible_occupant_to_clear() {
    let home = TilePos::new(2, 10);
    let frame = TilePos::new(8, 10);
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    obs.known_frames.push(frame);
    let mut occupant = observed_unit(90, UnitKind::Sentinel, frame.offset(1, 0));
    occupant.player = PlayerId(1);
    obs.enemy_units.push(occupant);
    let dials = standard_dials_without_opening_core_floor();

    let occupied = player_facing_intents(&dials, &obs);
    assert!(
        !plans_build(&occupied, BuildingKind::Extractor, frame),
        "a visible hostile ground unit temporarily blocks restoration: {occupied:?}"
    );

    let occupant_index =
        usize::try_from(obs.enemy_units[0].tile.y * obs.map_width + obs.enemy_units[0].tile.x)
            .unwrap();
    obs.visible[occupant_index] = false;
    obs.enemy_units.clear();
    let hidden = player_facing_intents(&dials, &obs);
    assert_eq!(
        exact_builder_for(&hidden, BuildingKind::Extractor, frame),
        Some(UnitId(5)),
        "the fog-honest view omits an unseen unit, so stale occupancy must not block the nearest \
         safe exact builder: {hidden:?}"
    );

    obs.visible[occupant_index] = true;
    let mut departed = observed_unit(90, UnitKind::Sentinel, TilePos::new(30, 10));
    departed.player = PlayerId(1);
    obs.enemy_units.push(departed);
    let cleared = player_facing_intents(&dials, &obs);
    assert_eq!(
        exact_builder_for(&cleared, BuildingKind::Extractor, frame),
        Some(UnitId(5)),
        "the fixed frame is retried with the nearest safe exact builder after the occupant leaves \
         the footprint and its approach: {cleared:?}"
    );
}

#[test]
fn player_facing_restoration_requires_a_clean_bounded_sweep_after_worker_damage() {
    let home = TilePos::new(2, 10);
    let frame = TilePos::new(8, 10);
    let incident = frame.offset(2, 1);
    let hidden_corner = incident.offset(
        oxide_sim::stats::HARVEST_INCIDENT_DANGER_RADIUS,
        oxide_sim::stats::HARVEST_INCIDENT_DANGER_RADIUS,
    );
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    obs.known_frames.push(frame);
    let dials = standard_dials_without_opening_core_floor();
    let mut policy = UtilityPolicy::new();
    let restores_frame = |intents: &[Intent]| plans_build(intents, BuildingKind::Extractor, frame);

    let _ = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    obs.tick = oxide_sim::bot::difficulty::strategic_admission_at_or_after(obs.tick + 1);
    obs.my_units[5].hp -= 1;
    obs.salvage_incidents.push(incident);
    let warned = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert!(
        !restores_frame(&warned),
        "a nearby worker hit pauses both restoration and its capital claim: {warned:?}"
    );
    let overseer = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(
        restores_frame(&overseer),
        "the profile-free Overseer retains its frozen restoration policy: {overseer:?}"
    );

    obs.tick = oxide_sim::bot::difficulty::strategic_admission_at_or_after(
        obs.tick + oxide_sim::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1,
    );
    obs.salvage_incidents.clear();
    obs.visible.fill(false);
    let expired_in_fog = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert!(
        !restores_frame(&expired_in_fog),
        "warning expiry in fog must not send another builder into the contested frame: \
         {expired_in_fog:?}"
    );

    obs.visible.fill(true);
    let hidden_index = usize::try_from(hidden_corner.y * obs.map_width + hidden_corner.x).unwrap();
    obs.visible[hidden_index] = false;
    obs.tick = oxide_sim::bot::difficulty::strategic_admission_at_or_after(obs.tick + 1);
    let partial_sweep = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert!(
        !restores_frame(&partial_sweep),
        "partial current sight cannot reopen a worker kill zone: {partial_sweep:?}"
    );

    obs.visible[hidden_index] = true;
    obs.tick = oxide_sim::bot::difficulty::strategic_admission_at_or_after(obs.tick + 1);
    let completed_sweep =
        policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map(&obs));
    assert_eq!(
        exact_builder_for(&completed_sweep, BuildingKind::Extractor, frame),
        Some(UnitId(5)),
        "one bounded danger-free sweep releases the nearest safe exact builder without a second \
         cooldown: {completed_sweep:?}"
    );
}

#[test]
fn remote_own_extractor_focuses_a_supporting_foundry_before_airworks() {
    let home = TilePos::new(2, 10);
    let extractor = TilePos::new(28, 10);
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    add_building(
        &mut obs,
        observed_building(2, BuildingKind::Extractor, extractor, true),
    );
    obs.known_frames.push(extractor);
    let mut dials = standard_dials_without_opening_core_floor();
    dials.foundry_cap = 4;

    let pretech = player_facing_intents(&dials, &obs);
    assert!(
        !pretech.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Foundry,
                ..
            }
        )),
        "an economic priority cannot bypass the real Fabricator prerequisite"
    );
    assert!(pretech.iter().any(|intent| matches!(
        intent,
        Intent::Build {
            kind: BuildingKind::Fabricator,
            ..
        }
    )));

    add_building(
        &mut obs,
        observed_building(1, BuildingKind::Fabricator, TilePos::new(5, 3), true),
    );
    let intents = player_facing_intents(&dials, &obs);
    let (builder, anchor) = intents
        .iter()
        .find_map(|intent| match intent {
            Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor,
            } => Some((*builder, *anchor)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("remote Extractor should focus a Foundry, got {intents:?}"));
    assert_eq!(
        builder,
        UnitId(2),
        "the support claim must retain the nearest route-capable worker through lowering"
    );
    assert!(
        !intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Airworks,
                ..
            }
        )),
        "consolidating a restored claim must precede the old Airworks rung"
    );
    assert!(
        footprint_distance(
            (anchor, BuildingKind::Foundry.base_stats().size),
            (extractor, BuildingKind::Extractor.base_stats().size),
        ) <= EXTRACTOR_SUPPORT_RADIUS,
        "the expansion at {anchor:?} must actually support the Extractor at {extractor:?}"
    );
    assert_eq!(
        anchor.chebyshev(extractor),
        2,
        "adjacent 2x2 footprints are the nearest legal support geometry"
    );
}

#[test]
fn extractor_support_search_reaches_the_full_legal_edge() {
    let home = TilePos::new(2, 10);
    let extractor = TilePos::new(28, 10);
    let mut obs = construction_observation(1_000);
    for (id, kind, anchor) in [
        (0, BuildingKind::Foundry, home),
        (1, BuildingKind::Fabricator, TilePos::new(5, 3)),
        (2, BuildingKind::Extractor, extractor),
    ] {
        add_building(&mut obs, observed_building(id, kind, anchor, true));
    }
    obs.known_frames.push(extractor);
    block_support_anchors(&mut obs, extractor, 8);
    let mut dials = standard_dials_without_opening_core_floor();
    dials.foundry_cap = 4;

    let intents = player_facing_intents(&dials, &obs);
    let (builder, anchor) = intents
        .iter()
        .find_map(|intent| match intent {
            Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor,
            } => Some((*builder, *anchor)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the complete support ring should contain a site: {intents:?}"));
    assert_eq!(
        builder,
        UnitId(2),
        "the edge claim must preserve its proven route-capable worker"
    );
    assert_eq!(
        anchor.chebyshev(extractor),
        EXTRACTOR_SUPPORT_RADIUS + 1,
        "all closer anchors were obstructed, so the legal edge must be searched"
    );
    assert!(
        footprint_distance(
            (anchor, BuildingKind::Foundry.base_stats().size),
            (extractor, BuildingKind::Extractor.base_stats().size),
        ) <= EXTRACTOR_SUPPORT_RADIUS
    );
}

#[test]
fn impossible_extractor_support_does_not_pin_the_crucible_fund() {
    let home = TilePos::new(2, 10);
    let extractor = TilePos::new(28, 10);
    let mut obs = construction_observation(470);
    obs.my_units.extend([
        observed_unit(23, UnitKind::Sentinel, TilePos::new(10, 14)),
        observed_unit(24, UnitKind::Sentinel, TilePos::new(11, 14)),
    ]);
    for (id, kind, anchor) in [
        (0, BuildingKind::Foundry, home),
        (1, BuildingKind::Fabricator, TilePos::new(5, 3)),
        (2, BuildingKind::Airworks, TilePos::new(8, 3)),
        (3, BuildingKind::Extractor, extractor),
    ] {
        add_building(&mut obs, observed_building(id, kind, anchor, true));
    }
    obs.known_frames.push(extractor);
    block_support_anchors(&mut obs, extractor, EXTRACTOR_SUPPORT_RADIUS + 1);
    let mut dials = standard_dials();
    dials.foundry_cap = 4;

    let intents = player_facing_intents(&dials, &obs);
    assert!(
        intents.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            }
        )),
        "a remote Extractor without a viable support site is not a capital rung: {intents:?}"
    );
    assert!(
        intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Crucible,
                ..
            }
        )),
        "the next real tech rung must keep its full 470-scrap fund: {intents:?}"
    );
}

#[test]
fn extractor_support_requires_a_builder_on_the_reachable_side() {
    let home = TilePos::new(22, 10);
    let extractor = TilePos::new(36, 10);
    let mut obs = construction_observation(470);
    obs.my_units.extend([
        observed_unit(23, UnitKind::Sentinel, TilePos::new(10, 14)),
        observed_unit(24, UnitKind::Sentinel, TilePos::new(11, 14)),
    ]);
    for unit in &mut obs.my_units {
        unit.tile = TilePos::new(5, 5 + i32::try_from(unit.id.0 % 8).unwrap());
    }
    for (id, kind, anchor) in [
        (0, BuildingKind::Foundry, home),
        (1, BuildingKind::Fabricator, TilePos::new(25, 3)),
        (2, BuildingKind::Airworks, TilePos::new(28, 3)),
        (3, BuildingKind::Extractor, extractor),
    ] {
        add_building(&mut obs, observed_building(id, kind, anchor, true));
    }
    obs.known_frames.push(extractor);
    obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(18, y)).collect();
    let mut dials = standard_dials();
    dials.foundry_cap = 4;

    let intents = player_facing_intents(&dials, &obs);
    assert!(
        intents.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            }
        )),
        "legal support terrain is not viable while every builder is across a known wall: {intents:?}"
    );
    assert!(
        intents.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Crucible,
                ..
            }
        )),
        "an unreachable support claim must not reserve away the next tech rung: {intents:?}"
    );
}

#[test]
fn projected_support_and_unknown_routes_do_not_create_duplicate_foundry_claims() {
    let home = TilePos::new(2, 10);
    let extractor = TilePos::new(28, 10);
    let planned_support = TilePos::new(24, 7);
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    add_building(
        &mut obs,
        observed_building(1, BuildingKind::Fabricator, TilePos::new(5, 3), true),
    );
    add_building(
        &mut obs,
        observed_building(2, BuildingKind::Extractor, extractor, true),
    );
    add_building(
        &mut obs,
        observed_building(3, BuildingKind::Foundry, planned_support, false),
    );
    obs.known_frames.push(extractor);
    let mut dials = standard_dials();
    dials.foundry_cap = 4;

    let already_planned = player_facing_intents(&dials, &obs);
    let foundry_claims: Vec<TilePos> = already_planned
        .iter()
        .filter_map(|intent| match intent {
            Intent::Build {
                kind: BuildingKind::Foundry,
                anchor,
            } => Some(*anchor),
            _ => None,
        })
        .collect();
    assert_eq!(
        foundry_claims,
        vec![planned_support],
        "the policy may resume the paid site, but must not start another Foundry"
    );

    obs.my_buildings.pop();
    obs.my_queues.pop();
    for y in 0..obs.map_height {
        obs.explored[usize::try_from(y * obs.map_width + 20).unwrap()] = false;
    }
    let unknown_route = player_facing_intents(&dials, &obs);
    assert!(
        !unknown_route.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Foundry,
                ..
            }
        )),
        "an own Extractor across an unproved ground route must not become a founding promise"
    );
}

#[test]
fn player_facing_reclaimers_scale_to_producer_demand_past_the_overseer_ceiling() {
    let home = TilePos::new(2, 10);
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    for (id, kind, anchor) in [
        (1, BuildingKind::Fabricator, TilePos::new(34, 3)),
        (2, BuildingKind::Airworks, TilePos::new(37, 3)),
        (3, BuildingKind::Crucible, TilePos::new(40, 3)),
    ] {
        add_building(&mut obs, observed_building(id, kind, anchor, true));
    }
    let mut dials = Dials::full();
    dials.tech = false;
    dials.scouting = false;
    dials.turret_response = false;
    dials.upgrades = false;
    dials.radar = false;
    for offset in 0..dials.reclaimer_cap {
        add_building(
            &mut obs,
            observed_building(
                10 + u32::try_from(offset).unwrap(),
                BuildingKind::Reclaimer,
                TilePos::new(12 + i32::try_from(offset * 2).unwrap(), 3),
                true,
            ),
        );
    }

    let player_facing = player_facing_intents(&dials, &obs);
    assert!(
        player_facing.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Reclaimer,
                ..
            }
        )),
        "a dry frame-less economy may keep scaling through ordinary legal Reclaimers"
    );

    let overseer = UtilityPolicy::new().think(&dials, &obs, &[], &[]);
    assert!(
        !overseer.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Reclaimer,
                ..
            }
        )),
        "the profile-free Overseer's historical cap must remain frozen"
    );

    for building in &mut obs.my_buildings {
        if building.kind == BuildingKind::Reclaimer {
            building.tier = 1;
            building.hp = BuildingKind::Reclaimer.tier_stats(1).max_hp;
        }
    }
    add_building(
        &mut obs,
        BuildingObs {
            tier: 1,
            hp: BuildingKind::Reclaimer.tier_stats(1).max_hp,
            ..observed_building(30, BuildingKind::Reclaimer, TilePos::new(20, 8), true)
        },
    );
    let funded = player_facing_intents(&dials, &obs);
    assert!(
        !funded.iter().any(|intent| matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Reclaimer,
                ..
            }
        )),
        "four Refineries fund four completed producers at the policy target; construction must stop"
    );
}

#[test]
fn pending_reclaimers_count_once_toward_future_income() {
    let home = TilePos::new(2, 10);
    let mut obs = construction_observation(1_000);
    add_building(
        &mut obs,
        observed_building(0, BuildingKind::Foundry, home, true),
    );
    obs.tick = oxide_sim::stats::FOUNDRY_DRIP_START_TICK;
    let site = BuildingId(40);
    add_building(
        &mut obs,
        observed_building(40, BuildingKind::Reclaimer, TilePos::new(12, 3), false),
    );
    obs.my_units[0].site = Some(site);
    obs.my_units[1].founding = Some((BuildingKind::Reclaimer, TilePos::new(16, 3)));
    let mut dials = Dials::full();
    dials.tech = false;
    dials.scouting = false;
    dials.turret_response = false;
    dials.upgrades = false;
    dials.radar = false;

    let projected_sites = player_facing_intents(&dials, &obs);
    assert!(
        projected_sites.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Reclaimer,
                anchor,
            } if *anchor != TilePos::new(12, 3) && *anchor != TilePos::new(16, 3)
        )),
        "one paid site plus one deferred claim already meets the one-producer target: {projected_sites:?}"
    );

    obs.my_units[1].founding = None;
    obs.my_buildings[1].tier = 1;
    let upgrading = player_facing_intents(&dials, &obs);
    assert!(
        upgrading.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Reclaimer,
                anchor,
            } if *anchor != TilePos::new(12, 3)
        )),
        "an automatic Refinery upgrade contributes its eventual rate while offline: {upgrading:?}"
    );
}
