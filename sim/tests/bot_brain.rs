//! Bot observation honesty and executive army-lifecycle contracts,
//! driven through the public API.

use chassis::grid::TilePos;
use oxide_sim::bot::{ArmyState, Brain, Executive, Intent, Observation, Specialty};
use oxide_sim::scenario::{
    BotConfig, BotDifficulty, BotStance, BuildingSpec, PlayerSpec, UnitSpec,
};
use oxide_sim::{
    BuildingKind, Command, Faction, Order, PlayerCommand, PlayerId, Scenario, State, UnitKind,
};

fn open_arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "brain-arena".into(),
        seed: 42,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#....................2.#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 500,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 500,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn large_open_arena(units: Vec<UnitSpec>) -> Scenario {
    let width = 40usize;
    let height = 24usize;
    let mut map = vec![format!("#{}#", ".".repeat(width - 2)); height];
    map[0] = "#".repeat(width);
    map[height - 1] = "#".repeat(width);
    for (x, y, marker) in [(1usize, 1usize, b'1'), (width - 3, height - 3, b'2')] {
        let mut row = map[y].as_bytes().to_vec();
        row[x] = marker;
        map[y] = String::from_utf8(row).unwrap();
    }
    let mut scenario = open_arena(units);
    scenario.map = map;
    scenario
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

#[test]
fn unseen_enemy_activity_cannot_touch_a_fog_honest_observation() {
    // Control and variant share everything except enemy movement deep in
    // player 0's fog. The fog-honest observations must stay bit-identical
    // — the review's core guarantee: filtering, not trust.
    let scenario = open_arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Sentinel, 19, 9),
        unit(1, UnitKind::Harvester, 20, 8),
    ]);
    let mut control = scenario.build().unwrap();
    let mut variant = scenario.build().unwrap();
    let wanderer = control.units()[1].id;

    for step in 0..10u32 {
        // The variant's enemy wanders its home corner (still in fog).
        let goal = if step % 2 == 0 {
            TilePos::new(17, 10)
        } else {
            TilePos::new(20, 9)
        };
        variant.tick(&[cmd(
            1,
            Command::Move {
                units: vec![wanderer],
                goal,
                queue: false,
            },
        )]);
        control.tick(&[]);
        for _ in 0..20 {
            variant.tick(&[]);
            control.tick(&[]);
        }
        let control_obs = Observation::fog_honest(&control, PlayerId(0));
        let variant_obs = Observation::fog_honest(&variant, PlayerId(0));
        assert_eq!(
            &control_obs.visible, &variant_obs.visible,
            "enemy activity cannot become a vision source"
        );
        let a = serde_json::to_string(&control_obs).unwrap();
        let b = serde_json::to_string(&variant_obs).unwrap();
        assert_eq!(a, b, "fog-honest observation leaked unseen enemy state");
    }
    // Sanity: the worlds themselves really did diverge (observations are
    // tile-resolution; the state hash is not).
    assert_ne!(
        control.hash(),
        variant.hash(),
        "test premise: the worlds actually diverged"
    );
}

#[test]
fn observation_distinguishes_explored_peaks_from_flyable_rock() {
    let mut scenario = open_arena(vec![unit(0, UnitKind::Harvester, 4, 2)]);
    let mut row = scenario.map[2].as_bytes().to_vec();
    row[6] = b'^';
    row[7] = b'#';
    scenario.map[2] = String::from_utf8(row).unwrap();
    let state = scenario.build().unwrap();
    let peak = TilePos::new(6, 2);
    let rock = TilePos::new(7, 2);

    let fog = Observation::fog_honest(&state, PlayerId(0));
    assert!(fog.visible(peak));
    assert!(!fog.visible(TilePos::new(20, 10)));
    assert!(!fog.visible(TilePos::new(-1, peak.y)));
    assert!(!fog.visible(TilePos::new(fog.map_width, peak.y)));
    assert!(fog.explored(peak));
    assert!(!fog.explored(TilePos::new(20, 10)));
    assert!(!fog.explored(TilePos::new(-1, peak.y)));
    assert!(!fog.explored(TilePos::new(fog.map_width, peak.y)));
    assert!(fog.known_rock.contains(&peak));
    assert!(fog.known_rock.contains(&rock));
    assert_eq!(fog.known_peaks, vec![peak]);

    let omniscient = Observation::omniscient(&state, PlayerId(0));
    assert!(omniscient.visible(TilePos::new(20, 10)));
    assert!(omniscient.explored(TilePos::new(20, 10)));
    assert!(omniscient.known_rock.contains(&rock));
    assert!(omniscient.known_peaks.contains(&peak));
    assert!(!omniscient.known_peaks.contains(&rock));
}

#[test]
fn fog_honest_visibility_is_the_canonical_row_major_team_mask() {
    let scenario = open_arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Harvester, 19, 9),
    ]);
    let state = scenario.build().unwrap();
    let me = PlayerId(0);
    let observation = Observation::fog_honest(&state, me);
    let vision = state.vision(me);
    let expected: Vec<bool> = (0..state.map().height())
        .flat_map(|y| (0..state.map().width()).map(move |x| vision.visible(TilePos::new(x, y))))
        .collect();

    assert_eq!(observation.visible, expected);
    assert!(observation.visible(TilePos::new(4, 2)));
    assert!(!observation.visible(TilePos::new(19, 9)));
}

#[test]
fn fog_honest_observation_exposes_only_live_anonymous_salvage_incidents() {
    let incident_tile = TilePos::new(7, 5);
    let mut state = open_arena(vec![
        unit(0, UnitKind::Harvester, incident_tile.x, incident_tile.y),
        unit(1, UnitKind::Sentinel, 10, 5),
    ])
    .build()
    .unwrap();
    let sentinel = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(1))
        .unwrap()
        .id;

    let mut observed = None;
    for _ in 0..200 {
        state.tick(&[]);
        let observation = Observation::fog_honest(&state, PlayerId(0));
        if !observation.salvage_incidents.is_empty() {
            observed = Some(observation);
            break;
        }
    }
    let observation = observed.expect("the nearby Sentinel hits the Harvester");
    assert_eq!(observation.salvage_incidents, vec![incident_tile]);
    assert!(
        Observation::fog_honest(&state, PlayerId(1))
            .salvage_incidents
            .is_empty(),
        "the attacker cannot read the victim team's private warning memory"
    );
    assert_eq!(
        serde_json::from_str::<Observation>(&serde_json::to_string(&observation).unwrap()).unwrap(),
        observation,
        "the observation wire preserves the anonymous warning tiles"
    );

    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![sentinel],
            goal: TilePos::new(19, 9),
            queue: false,
        },
    )]);
    for _ in 0..oxide_sim::stats::HARVEST_INCIDENT_MEMORY_TICKS + 200 {
        state.tick(&[]);
    }
    assert!(
        Observation::fog_honest(&state, PlayerId(0))
            .salvage_incidents
            .is_empty(),
        "expired vision incidents must not leak into a later observation"
    );
}

#[test]
fn a_later_trip_loss_keeps_replacement_workers_out_of_the_same_kill_zone() {
    let exposed = TilePos::new(20, 5);
    let safe = TilePos::new(2, 9);
    let original_at = TilePos::new(19, 5);
    let replacement_at = TilePos::new(3, 3);
    let mut scenario = large_open_arena(vec![
        unit(0, UnitKind::Harvester, original_at.x, original_at.y),
        unit(0, UnitKind::Harvester, replacement_at.x, replacement_at.y),
        unit(0, UnitKind::Kestrel, 18, 7),
        unit(1, UnitKind::Bombard, 29, 5),
        // A harmless spotter gives the hidden Bombard a legal target without
        // itself making salvage unsafe.
        unit(1, UnitKind::Harvester, 24, 3),
    ]);
    scenario.players[0].scrap = 0;
    for source in [exposed, safe] {
        let mut row = scenario.map[source.y as usize].as_bytes().to_vec();
        row[source.x as usize] = b's';
        scenario.map[source.y as usize] = String::from_utf8(row).unwrap();
    }
    let mut state = scenario.build().unwrap();
    let original = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.tile() == original_at)
        .expect("the first route worker exists")
        .id;
    let replacement = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.tile() == replacement_at)
        .expect("the replacement worker exists")
        .id;
    let bombard = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Bombard)
        .expect("the hidden gun exists")
        .id;

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![original],
            node: exposed,
            queue: false,
        },
    )]);
    let mut delivered = false;
    for _ in 0..800 {
        let report = state.tick(&[]);
        if report.events.iter().any(|event| {
            matches!(
                event,
                oxide_sim::Event::ScrapDeposited {
                    player: PlayerId(0),
                    ..
                }
            )
        }) {
            delivered = true;
            break;
        }
    }
    assert!(delivered, "the route completed its first load");
    assert!(matches!(
        state.unit(original).expect("the first worker still lives").order,
        Order::Harvest {
            node,
            retiring: false,
            ..
        } if node == exposed
    ));

    // A prior wound makes the staged artillery shot lethal. The first trip
    // remains a real, completed Harvest cycle; only the later attack is
    // arranged so the regression does not depend on two projectile timings.
    let original_slot = state
        .units()
        .iter()
        .position(|unit| unit.id == original)
        .expect("the first worker still exists");
    let mut document = serde_json::to_value(&state).unwrap();
    document["units"][original_slot]["hp"] = serde_json::json!(45);
    state = serde_json::from_value(document).unwrap();

    let mut second_trip_started = false;
    for _ in 0..500 {
        state.tick(&[]);
        if state.unit(original).is_some_and(|unit| unit.carrying > 0) {
            second_trip_started = true;
            break;
        }
    }
    assert!(
        second_trip_started,
        "the same persistent Harvest program began a later trip"
    );
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![bombard],
            target: oxide_sim::Target::Unit(original),
            queue: false,
        },
    )]);
    for _ in 0..80 {
        if !state.shells().is_empty() {
            break;
        }
        state.tick(&[]);
    }
    assert!(
        !state.shells().is_empty(),
        "the fog-honest spotter gave the hidden gun a legal shot"
    );
    state.tick(&[cmd(
        1,
        Command::Stop {
            units: vec![bombard],
        },
    )]);
    let mut died = false;
    for _ in 0..120 {
        let report = state.tick(&[]);
        if report.events.iter().any(
            |event| matches!(event, oxide_sim::Event::UnitDied { unit, .. } if *unit == original),
        ) {
            died = true;
            break;
        }
    }
    assert!(died, "the later-trip worker died in the exposed zone");

    let observation = Observation::fog_honest(&state, PlayerId(0));
    let incident = *observation
        .salvage_incidents
        .first()
        .expect("the loss leaves an anonymous local warning");
    assert!(incident.chebyshev(exposed) <= oxide_sim::stats::HARVEST_INCIDENT_DANGER_RADIUS);
    assert!(
        observation
            .known_wrecks
            .iter()
            .any(|(tile, amount)| tile.chebyshev(incident)
                <= oxide_sim::stats::HARVEST_INCIDENT_DANGER_RADIUS
                && *amount > 0),
        "the tempting fresh wreck is known near the loss"
    );

    while !oxide_sim::bot::difficulty::strategic_admission_tick(state.current_tick()) {
        state.tick(&[]);
    }
    let mut brain = Brain::scripted(
        PlayerId(0),
        scenario.seed,
        BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 7),
    );
    let commands = brain.act(&state);
    let assigned: Vec<_> = commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Harvest { units, node, .. } if units.contains(&replacement) => Some(*node),
            _ => None,
        })
        .collect();
    assert_eq!(assigned, vec![safe]);
    assert!(assigned.iter().all(|node| {
        node.chebyshev(incident) > oxide_sim::stats::HARVEST_INCIDENT_DANGER_RADIUS
    }));
}

#[test]
fn omniscient_observation_reports_frames_and_fresh_battlefield_wrecks() {
    let mut scenario = open_arena(vec![
        unit(0, UnitKind::Lancer, 5, 5),
        unit(1, UnitKind::Scuttler, 7, 5),
    ]);
    let frame = TilePos::new(12, 6);
    let mut row = scenario.map[frame.y as usize].as_bytes().to_vec();
    row[frame.x as usize] = b'E';
    scenario.map[frame.y as usize] = String::from_utf8(row).unwrap();
    let mut state = scenario.build().unwrap();
    let lancer = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Lancer)
        .unwrap()
        .id;
    let victim = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Scuttler)
        .unwrap()
        .id;
    let victim_tile = state.unit(victim).expect("the target starts alive").tile();

    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![lancer],
            target: oxide_sim::Target::Unit(victim),
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(
            |event| matches!(event, oxide_sim::Event::UnitDied { unit, .. } if *unit == victim)
        ),
        "premise: the opening rail shot creates salvage on its command tick"
    );

    let observation = Observation::omniscient(&state, PlayerId(0));
    assert_eq!(observation.known_frames, vec![frame]);
    let fresh_value = UnitKind::Scuttler.stats().cost * oxide_sim::stats::WRECK_VALUE_NUM
        / oxide_sim::stats::WRECK_VALUE_DEN;
    let expected_value = fresh_value.saturating_sub(u32::from(
        report
            .tick
            .is_multiple_of(oxide_sim::stats::WRECK_DECAY_TICKS),
    ));
    assert_eq!(
        observation.known_wrecks,
        vec![(victim_tile, expected_value)],
        "the complete-world view reports the exact live wreck field"
    );
}

#[test]
fn fog_honest_shows_ghosts_not_live_enemies() {
    // A scout sees the enemy foundry, walks home, and the fog-honest
    // observation keeps a ghost (seen: false) while hiding the enemy
    // units it can no longer see. The bystander is a harvester on purpose:
    // it cannot fight, so it never chases the scout back into sight.
    let scenario = open_arena(vec![
        unit(0, UnitKind::Harvester, 16, 9),
        unit(1, UnitKind::Harvester, 19, 9),
    ]);
    let mut state = scenario.build().unwrap();
    let scout = state.units()[0].id;
    let obs = Observation::fog_honest(&state, PlayerId(0));
    assert!(
        obs.enemy_buildings.iter().any(|b| b.seen),
        "the scout starts in sight of the enemy foundry"
    );
    assert!(!obs.enemy_units.is_empty(), "and of a worker");

    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(3, 2),
            queue: false,
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
    }
    let obs = Observation::fog_honest(&state, PlayerId(0));
    assert!(
        obs.enemy_buildings.iter().any(|b| !b.seen),
        "the foundry lingers as a ghost"
    );
    let remembered_anchor = obs
        .enemy_buildings
        .iter()
        .find(|b| !b.seen)
        .expect("the foundry ghost exists")
        .anchor;
    assert!(
        obs.explored(remembered_anchor),
        "the ghost's ground remains explored"
    );
    assert!(
        !obs.visible(remembered_anchor),
        "remembered ground is not current sight"
    );
    assert!(
        obs.enemy_units.is_empty(),
        "unseen enemy units are simply absent"
    );
    // Omniscient control: everything is live there.
    let omni = Observation::omniscient(&state, PlayerId(0));
    assert!(omni.enemy_units.len() == 1 && omni.enemy_buildings.iter().all(|b| b.seen));
}

#[test]
fn allied_observations_reveal_presence_but_not_private_programs() {
    let scenario = Scenario {
        name: "allied-observation".into(),
        seed: 17,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......2...............#".into(),
            "#........s.............#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#....................3.#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Observer".into(),
                faction: Faction::Ferrous,
                team: Some(7),
                scrap: 200,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Ally".into(),
                faction: Faction::Cupric,
                team: Some(7),
                scrap: 1_000,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Enemy".into(),
                faction: Faction::Ferrous,
                team: Some(9),
                scrap: 200,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            unit(1, UnitKind::Harvester, 10, 3),
            unit(1, UnitKind::Harvester, 13, 3),
            unit(1, UnitKind::Harvester, 13, 6),
            unit(1, UnitKind::Harvester, 10, 6),
            unit(1, UnitKind::Skyhook, 10, 8),
            unit(1, UnitKind::Sentinel, 9, 8),
        ],
        buildings: vec![BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x: 14,
            y: 3,
        }],
        meta: None,
    };
    let mut state = scenario.build().expect("the team arena builds");
    let (worker, stripper, builder, founder, sling, rider) = {
        let id_at = |tile| {
            state
                .units()
                .iter()
                .find(|unit| unit.tile() == tile)
                .expect("the authored allied unit stands at its start")
                .id
        };
        (
            id_at(TilePos::new(10, 3)),
            id_at(TilePos::new(13, 3)),
            id_at(TilePos::new(13, 6)),
            id_at(TilePos::new(10, 6)),
            id_at(TilePos::new(10, 8)),
            id_at(TilePos::new(9, 8)),
        )
    };
    let stripped = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Turret)
        .expect("the allied Turret stands")
        .id;
    state.tick(&[cmd(
        1,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(9, 3),
            queue: false,
        },
    )]);
    for _ in 0..30 {
        state.tick(&[]);
        if state.unit(worker).is_some_and(|unit| unit.carrying > 0) {
            break;
        }
    }
    let live = state.unit(worker).expect("the allied worker lives");
    assert!(live.carrying > 0, "test premise: the ally mined scrap");
    assert!(matches!(live.order, Order::Harvest { .. }));

    let site_anchor = TilePos::new(14, 6);
    let founding_anchor = TilePos::new(16, 6);
    state.tick(&[
        cmd(
            1,
            Command::Salvage {
                units: vec![stripper],
                building: stripped,
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Build {
                units: vec![builder],
                kind: BuildingKind::Turret,
                anchor: site_anchor,
                queue: false,
                defer: false,
            },
        ),
        cmd(
            1,
            Command::Build {
                units: vec![founder],
                kind: BuildingKind::Turret,
                anchor: founding_anchor,
                queue: false,
                defer: true,
            },
        ),
        cmd(
            1,
            Command::Load {
                units: vec![rider],
                transport: sling,
                queue: false,
            },
        ),
    ]);
    assert!(matches!(
        state.unit(stripper).expect("the stripper lives").order,
        Order::Salvage { building } if building == stripped
    ));
    assert!(matches!(
        state.unit(builder).expect("the builder lives").order,
        Order::Build { .. }
    ));
    assert!(matches!(
        state.unit(founder).expect("the founder lives").order,
        Order::Found { kind: BuildingKind::Turret, anchor } if anchor == founding_anchor
    ));
    assert!(
        !state
            .unit(sling)
            .expect("the allied sling lives")
            .cargo
            .is_empty(),
        "test premise: the allied transport carries a private manifest"
    );
    assert!(state.unit(rider).is_none(), "the rider is inside the sling");

    for observation in [
        Observation::omniscient(&state, PlayerId(0)),
        Observation::fog_honest(&state, PlayerId(0)),
    ] {
        assert!(observation.my_units.is_empty());
        assert_eq!(observation.ally_units.len(), 5);
        assert!(observation.ally_units.iter().any(|ally| ally.id == worker));
        assert!(observation.ally_units.iter().any(|ally| ally.id == sling));
        for ally in &observation.ally_units {
            assert!(!ally.idle, "ally intent is opaque, never reported as idle");
            assert_eq!(ally.carrying, 0, "ally carried scrap is private");
            assert_eq!(ally.cargo, 0, "ally transport manifests are private");
            assert_eq!(ally.site, None, "ally construction orders are private");
            assert_eq!(ally.salvaging, None, "ally salvage orders are private");
            assert_eq!(ally.founding, None, "ally deferred claims are private");
        }
        assert!(
            observation
                .ally_buildings
                .iter()
                .any(|building| building.player == PlayerId(1)
                    && building.kind == BuildingKind::Foundry),
            "the ally's standing base remains public team presence"
        );
    }
}

#[test]
fn own_observations_expose_salvage_and_deferred_found_commitments() {
    let mut scenario = open_arena(vec![unit(0, UnitKind::Harvester, 4, 3)]);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 6,
        y: 3,
    });
    let mut state = scenario.build().unwrap();
    let worker = state.units()[0].id;
    let turret = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .expect("the authored Turret stands")
        .id;

    state.tick(&[cmd(
        0,
        Command::Salvage {
            units: vec![worker],
            building: turret,
            queue: false,
        },
    )]);
    let salvage = Observation::omniscient(&state, PlayerId(0));
    assert_eq!(salvage.my_units[0].salvaging, Some(turret));
    assert_eq!(salvage.my_units[0].founding, None);

    let anchor = TilePos::new(9, 3);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![worker],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: true,
        },
    )]);
    let founding = Observation::fog_honest(&state, PlayerId(0));
    assert_eq!(
        founding.my_units[0].founding,
        Some((BuildingKind::Turret, anchor))
    );
    assert_eq!(founding.my_units[0].salvaging, None);
    assert_eq!(founding.my_units[0].site, None);
}

#[test]
fn own_observations_expose_current_paid_repairs_without_disclosing_queued_programs() {
    let mut scenario = open_arena(vec![
        unit(0, UnitKind::Harvester, 4, 3),
        unit(0, UnitKind::Harvester, 4, 9),
    ]);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 17,
        y: 3,
    });
    let state = scenario.build().unwrap();
    let turret = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .unwrap()
        .id;
    let mut document = serde_json::to_value(&state).unwrap();
    for building in document["buildings"].as_array_mut().unwrap() {
        if building["id"] == serde_json::json!(turret.0) {
            building["hp"] = serde_json::json!(1);
        }
    }
    let mut state: State = serde_json::from_value(document).unwrap();
    let active = state.units()[0].id;
    let queued = state.units()[1].id;
    state.tick(&[
        cmd(
            0,
            Command::Repair {
                units: vec![active],
                building: turret,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![queued],
                goal: TilePos::new(19, 9),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Repair {
                units: vec![queued],
                building: turret,
                queue: true,
            },
        ),
    ]);

    let observation = Observation::fog_honest(&state, PlayerId(0));
    assert!(
        observation
            .my_units
            .iter()
            .any(|unit| unit.id == active && unit.repairing),
        "current voluntary upkeep is visible to the owner"
    );
    assert!(
        observation
            .my_units
            .iter()
            .any(|unit| unit.id == queued && !unit.repairing),
        "a queued program stays opaque until it becomes current"
    );

    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![active],
        },
    )]);
    let active = state.unit(active).unwrap();
    assert_eq!(active.order, Order::Idle);
    assert!(active.queue.is_empty());

    let queued = state.unit(queued).unwrap();
    assert!(matches!(queued.order, Order::Move { .. }));
    assert!(
        queued
            .queue
            .iter()
            .any(|order| matches!(order, Order::Repair { .. })),
        "stopping active repair must not erase another unit's queued program"
    );
}

#[test]
fn a_stranded_brain_spends_only_on_one_recovery_harvester() {
    let price = UnitKind::Harvester.stats().cost;

    let mut short = open_arena(Vec::new());
    short.players[0].scrap = price - 1;
    let short = short.build().expect("the stranded arena builds");
    let mut brain = Brain::balanced(PlayerId(0), 91);
    assert_eq!(
        brain.act(&short),
        Vec::new(),
        "ordinary policy spending pauses while the replacement fund is short"
    );

    let mut funded = open_arena(Vec::new());
    funded.players[0].scrap = price;
    let funded = funded.build().expect("the funded arena builds");
    let foundry = funded
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .expect("the stranded seat retains its Foundry")
        .id;
    let mut brain = Brain::balanced(PlayerId(0), 91);
    assert_eq!(
        brain.act(&funded),
        vec![PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        }],
        "the complete reserve buys exactly one recovery unit"
    );
}

#[test]
fn a_scripted_brain_cancels_paid_repairs_and_delivers_deferred_capital() {
    let mut scenario = open_arena(vec![
        unit(0, UnitKind::Harvester, 4, 3),
        unit(0, UnitKind::Harvester, 4, 10),
        unit(0, UnitKind::Harvester, 5, 4),
        unit(0, UnitKind::Harvester, 6, 4),
        unit(0, UnitKind::Sentinel, 6, 6),
        unit(0, UnitKind::Sentinel, 7, 6),
        unit(0, UnitKind::Sentinel, 8, 6),
    ]);
    scenario.buildings.extend([
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 5,
            y: 8,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Airworks,
            x: 8,
            y: 8,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Crucible,
            x: 11,
            y: 8,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 17,
            y: 2,
        },
    ]);
    let foundry_cost = BuildingKind::Foundry
        .base_stats()
        .construction
        .expect("expansion Foundries have a price")
        .cost;
    scenario.players[0].scrap = foundry_cost;
    let state = scenario.build().expect("the deferred-capital arena builds");
    let turret = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .expect("the repair patient stands")
        .id;
    let mut document = serde_json::to_value(&state).unwrap();
    for building in document["buildings"].as_array_mut().unwrap() {
        if building["id"] == serde_json::json!(turret.0) {
            building["hp"] = serde_json::json!(1);
        }
    }
    let mut state: State = serde_json::from_value(document).unwrap();
    let founder = state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(4, 3))
        .unwrap()
        .id;
    let repairer = state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(4, 10))
        .unwrap()
        .id;
    let anchor = TilePos::new(17, 8);
    let report = state.tick(&[
        cmd(
            0,
            Command::Repair {
                units: vec![repairer],
                building: turret,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Build {
                units: vec![founder],
                kind: BuildingKind::Foundry,
                anchor,
                queue: false,
                defer: true,
            },
        ),
    ]);
    assert!(
        report
            .events
            .iter()
            .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
        "test premise: both voluntary programs are accepted: {:?}",
        report.events
    );
    assert!(matches!(
        state.unit(founder).unwrap().order,
        Order::Found {
            kind: BuildingKind::Foundry,
            anchor: promised,
        } if promised == anchor
    ));
    assert!(matches!(
        state.unit(repairer).unwrap().order,
        Order::Repair { building } if building == turret
    ));
    assert!(
        Observation::fog_honest(&state, PlayerId(0))
            .my_units
            .iter()
            .any(|unit| unit.id == repairer && unit.repairing),
        "own voluntary upkeep is an honest controller-visible program"
    );

    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 0);
    let mut brain = Brain::scripted(PlayerId(0), scenario.seed, config);
    while !oxide_sim::bot::difficulty::strategic_admission_tick(state.current_tick()) {
        state.tick(&[]);
    }
    let commands = brain.act(&state);
    assert!(
        commands.iter().any(|command| matches!(
            &command.command,
            Command::Stop { units } if units == &[repairer]
        )),
        "the real player-facing brain cancels upkeep while construction is unpaid: {commands:?}"
    );
    assert!(
        commands.iter().all(|command| !matches!(
            command.command,
            Command::Train { .. }
                | Command::Build { .. }
                | Command::Repair { .. }
                | Command::RepairUnit { .. }
                | Command::UpgradeBuilding { .. }
        )),
        "no command may spend the deferred claim's bank: {commands:?}"
    );

    let mut insufficient = false;
    let mut claimed = false;
    for _ in 0..1_000 {
        let commands = brain.act(&state);
        let report = state.tick(&commands);
        insufficient |= report.events.iter().any(|event| {
            matches!(
                event,
                oxide_sim::Event::OrderStalled {
                    unit,
                    reason: oxide_sim::StallReason::InsufficientScrap,
                    ..
                } if *unit == founder
            )
        });
        claimed = state.buildings().iter().any(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Foundry
                && building.anchor == anchor
        });
        if claimed {
            break;
        }
    }
    assert!(claimed, "the deferred Foundry reaches and claims its site");
    assert!(
        !insufficient,
        "voluntary upkeep must not starve its arrival"
    );
    assert_eq!(state.player(PlayerId(0)).scrap, 0);
}

#[test]
fn a_dispatched_build_that_never_appears_blacklists_only_its_anchor() {
    let mut scenario = open_arena(
        (0..5)
            .map(|offset| unit(0, UnitKind::Harvester, 3 + offset, 3))
            .collect(),
    );
    scenario.players[0].scrap = 2_000;
    let mut salvage_row = scenario.map[6].as_bytes().to_vec();
    salvage_row[12] = b's';
    scenario.map[6] = String::from_utf8(salvage_row).unwrap();
    let mut state = scenario.build().expect("the construction arena builds");
    let mut continuing = Brain::balanced(PlayerId(0), scenario.seed);
    let macro_cadence = oxide_sim::bot::difficulty::STRATEGIC_ADMISSION_CADENCE;
    let fabricator_anchor = |commands: &[PlayerCommand]| {
        commands.iter().find_map(|command| match command.command {
            Command::Build {
                kind: BuildingKind::Fabricator,
                anchor,
                ..
            } => Some(anchor),
            _ => None,
        })
    };

    let opening = continuing.act(&state);
    assert_eq!(
        opening
            .iter()
            .filter(|command| matches!(command.command, Command::Harvest { .. }))
            .count(),
        3,
        "the scout and builder each preempt one opening harvest chore"
    );
    assert_eq!(
        opening
            .iter()
            .filter(|command| matches!(command.command, Command::Move { .. }))
            .count(),
        1,
        "the scout's exact claim must survive the same lowering pass"
    );
    let opening_anchor =
        fabricator_anchor(&opening).expect("capital construction preempts a routine harvest chore");

    // Leave the emitted command unapplied. The next audit should treat the
    // absent site as a refusal, while a fresh commander still prefers the
    // untouched geometry.
    for _ in 0..macro_cadence {
        state.tick(&[]);
    }
    assert_eq!(state.current_tick(), macro_cadence);

    let mut fresh = Brain::balanced(PlayerId(0), scenario.seed);
    let preferred = fabricator_anchor(&fresh.act(&state))
        .expect("a fresh commander dispatches its preferred Fabricator site");
    let after_refusal = fabricator_anchor(&continuing.act(&state))
        .expect("the continuing commander searches for another Fabricator site after refusal");

    assert_eq!(
        opening_anchor, preferred,
        "a fresh commander should still prefer the unapplied command's legal geometry"
    );
    assert_ne!(
        after_refusal, opening_anchor,
        "a dispatched Build that never appears must still blacklist its anchor"
    );
}

#[test]
fn a_brain_without_an_authored_aircraft_discovers_an_island_opponent() {
    let mut scenario = open_arena(
        (0..5)
            .map(|offset| unit(0, UnitKind::Harvester, 3 + offset, 3))
            .collect(),
    );
    for row in 1..scenario.map.len() - 1 {
        let mut bytes = scenario.map[row].as_bytes().to_vec();
        bytes[12] = b'#';
        scenario.map[row] = String::from_utf8(bytes).unwrap();
    }
    scenario.players[0].scrap = 2_000;
    scenario.buildings.extend([
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 5,
            y: 7,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Airworks,
            x: 8,
            y: 7,
        },
    ]);
    let mut state = scenario.build().expect("the island arena builds");
    let mut brain = Brain::balanced(PlayerId(0), scenario.seed);
    assert!(
        Observation::fog_honest(&state, PlayerId(0))
            .enemy_buildings
            .is_empty(),
        "the opposing shore starts outside vision"
    );
    let mut saw_no_route = false;
    let mut saw_scout_flyer = false;

    for _ in 0..1_200 {
        let commands = brain.act(&state);
        let report = state.tick(&commands);
        saw_no_route |= report.events.iter().any(|event| {
            matches!(
                event,
                oxide_sim::Event::OrderStalled {
                    player: PlayerId(0),
                    reason: oxide_sim::StallReason::NoRoute,
                    ..
                }
            )
        });
        saw_scout_flyer |= state.units().iter().any(|unit| {
            unit.player == PlayerId(0) && unit.kind.role() == oxide_sim::stats::Role::Scout
        });
        if !Observation::fog_honest(&state, PlayerId(0))
            .enemy_buildings
            .is_empty()
        {
            break;
        }
    }

    assert!(
        saw_no_route,
        "the ground sweep must prove the severed route"
    );
    assert!(
        saw_scout_flyer,
        "the Airworks must replace the stranded ground scout"
    );
    assert!(
        !Observation::fog_honest(&state, PlayerId(0))
            .enemy_buildings
            .is_empty(),
        "the replacement flyer must reveal the enemy shore"
    );
}

#[test]
fn the_army_lifecycle_stages_pushes_engages_and_withdraws() {
    // Four sentinels form an army, push into a hopeless fight (ten enemy
    // sentinels), and the executive pulls them out: Staging -> Pushing ->
    // Engaging -> Withdrawing, with the survivors sent home together.
    let mut units = vec![
        unit(0, UnitKind::Sentinel, 3, 2),
        unit(0, UnitKind::Sentinel, 4, 2),
        unit(0, UnitKind::Sentinel, 3, 3),
        unit(0, UnitKind::Sentinel, 5, 2),
    ];
    for i in 0..10 {
        units.push(unit(1, UnitKind::Sentinel, 15 + (i % 4), 7 + (i / 4)));
    }
    let mut state = open_arena(units).build().unwrap();
    let me = PlayerId(0);
    let mut exec = Executive::new();

    let think = |state: &State| Observation::omniscient(state, me);

    // Form at a staging point mid-map.
    let staging = TilePos::new(8, 4);
    let commands = exec.apply(me, &think(&state), &[Intent::FormArmy { staging, size: 4 }]);
    assert_eq!(exec.armies().len(), 1);
    assert_eq!(exec.armies()[0].members.len(), 4);
    assert_eq!(exec.armies()[0].state, ArmyState::Staging);
    let army = exec.armies()[0].id;
    state.tick(&commands);
    for _ in 0..200 {
        let obs = think(&state);
        let cmds = exec.maintain(me, &obs, TilePos::new(2, 2));
        state.tick(&cmds);
    }
    assert_eq!(
        exec.armies()[0].state,
        ArmyState::Staging,
        "gathered, waiting"
    );

    // Push into the enemy mass.
    let commands = exec.apply(
        me,
        &think(&state),
        &[Intent::PushArmy {
            army,
            target: TilePos::new(16, 8),
        }],
    );
    state.tick(&commands);
    let mut saw_engaging = false;
    let mut saw_withdrawing = false;
    for _ in 0..800 {
        let obs = think(&state);
        let cmds = exec.maintain(me, &obs, TilePos::new(2, 2));
        state.tick(&cmds);
        match exec.armies().first().map(|a| a.state) {
            Some(ArmyState::Engaging) => saw_engaging = true,
            Some(ArmyState::Withdrawing) => {
                saw_withdrawing = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_engaging, "the push made contact");
    assert!(
        saw_withdrawing,
        "a 4-vs-10 engagement must be abandoned, not fed"
    );
    // The survivors actually leave: within a few hundred ticks the army
    // re-stages (or died to the last machine trying).
    for _ in 0..600 {
        let obs = think(&state);
        let cmds = exec.maintain(me, &obs, TilePos::new(2, 2));
        state.tick(&cmds);
        if exec
            .armies()
            .first()
            .is_none_or(|a| a.state == ArmyState::Staging)
        {
            return;
        }
    }
    panic!("the withdrawal never resolved");
}

#[test]
fn qa_rear_line_stays_frozen_while_player_facing_releases_repaired_units() {
    // Executive semantics, pinned against a synthetic observation (the
    // executive is a pure function of what it is shown): a member below
    // the 35% pullback line and out of contact is Move-ordered to the
    // rear and dropped from the army; a wounded member still in a fight
    // is left in the line, and the rear reservation remains stable after
    // external healing.
    use oxide_sim::UnitId;
    use oxide_sim::bot::UnitObs;

    let me = PlayerId(0);
    let obs_with = |units: Vec<UnitObs>| Observation {
        version: oxide_sim::bot::observation::OBSERVATION_VERSION,
        tick: 0,
        me,
        scrap: 0,
        map_width: 24,
        map_height: 13,
        my_units: units,
        my_buildings: Vec::new(),
        my_queues: Vec::new(),
        ally_units: Vec::new(),
        ally_buildings: Vec::new(),
        enemy_units: Vec::new(),
        enemy_buildings: Vec::new(),
        visible: vec![true; 24 * 13],
        explored: vec![true; 24 * 13],
        known_scrap: Vec::new(),
        known_rock: Vec::new(),
        known_frames: Vec::new(),
        known_peaks: Vec::new(),
        known_wrecks: Vec::new(),
        salvage_incidents: Vec::new(),
        blips: Vec::new(),
        faction: oxide_sim::Faction::Ferrous,
        my_shells: 0,
        incoming_shells: Vec::new(),
    };
    let sentinel = |id: u32, player: u8, x: i32, y: i32, hp: u32| UnitObs {
        id: UnitId(id),
        player: PlayerId(player),
        kind: UnitKind::Sentinel,
        tile: TilePos::new(x, y),
        hp,
        idle: true,
        carrying: 0,
        cargo: 0,
        site: None,
        salvaging: None,
        founding: None,
        repairing: false,
        grounded: false,
    };

    let mut exec = Executive::new();
    let obs = obs_with(vec![sentinel(0, 0, 3, 2, 100), sentinel(1, 0, 4, 2, 100)]);
    let _ = exec.apply(
        me,
        &obs,
        &[Intent::FormArmy {
            staging: TilePos::new(4, 3),
            size: 2,
        }],
    );
    assert_eq!(exec.armies()[0].members.len(), 2);

    // Wounded but in contact: an armed enemy stands next to the line —
    // no rotation happens mid-fight.
    let mut contact = obs_with(vec![sentinel(0, 0, 4, 3, 10), sentinel(1, 0, 4, 2, 100)]);
    contact.enemy_units.push(sentinel(9, 1, 6, 3, 100));
    let _ = exec.maintain(me, &contact, TilePos::new(1, 1));
    assert!(
        exec.armies()[0].members.contains(&UnitId(0)),
        "no pullback while the fight is live"
    );

    // Same wound, enemy gone: the rotation fires, with a Move to the
    // rear tile — not to the army's staging point.
    let calm = obs_with(vec![sentinel(0, 0, 4, 3, 10), sentinel(1, 0, 4, 2, 100)]);
    let cmds = exec.maintain(me, &calm, TilePos::new(1, 1));
    assert!(
        !exec.armies().is_empty() && !exec.armies()[0].members.contains(&UnitId(0)),
        "the wounded member left the army"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            &c.command,
            Command::Move { units, goal, .. }
                if units == &vec![UnitId(0)] && *goal == TilePos::new(1, 1)
        )),
        "the wounded member was sent to the rear"
    );

    // Re-drafting skips the rear line even though the unit reads idle.
    let _ = exec.apply(
        me,
        &calm,
        &[Intent::FormArmy {
            staging: TilePos::new(5, 3),
            size: 5,
        }],
    );
    for army in exec.armies() {
        assert!(
            !army.members.contains(&UnitId(0)),
            "a wounded rear-line member stays out of drafts"
        );
    }

    // Maintenance retains even an externally healed rear member. Rear-line
    // reservation is part of the current scripted controller's behavior.
    let healed = obs_with(vec![sentinel(0, 0, 1, 1, 100), sentinel(1, 0, 4, 2, 100)]);
    let _ = exec.maintain(me, &healed, TilePos::new(1, 1));
    let _ = exec.apply(
        me,
        &healed,
        &[Intent::FormArmy {
            staging: TilePos::new(5, 3),
            size: 5,
        }],
    );
    assert!(
        exec.armies()
            .iter()
            .all(|army| !army.members.contains(&UnitId(0))),
        "QA maintenance retains the historical rear line"
    );

    let _ = exec.maintain_player_facing(me, &healed, TilePos::new(1, 1));
    let _ = exec.apply(
        me,
        &healed,
        &[Intent::FormArmy {
            staging: TilePos::new(6, 3),
            size: 5,
        }],
    );
    assert!(
        exec.armies()
            .iter()
            .any(|army| army.members.contains(&UnitId(0))),
        "the player-facing controller may redraft a genuinely repaired veteran"
    );
}

#[test]
fn scripted_brain_repairs_a_timed_out_rear_unit_before_redrafting_it() {
    let mut scenario = large_open_arena(vec![
        unit(0, UnitKind::Harvester, 3, 3),
        unit(0, UnitKind::Harvester, 4, 3),
        unit(0, UnitKind::Harvester, 5, 3),
        unit(0, UnitKind::Harvester, 6, 3),
        unit(0, UnitKind::Tender, 3, 8),
        unit(0, UnitKind::Sentinel, 4, 6),
        unit(0, UnitKind::Sentinel, 5, 6),
        unit(0, UnitKind::Sentinel, 6, 6),
        unit(0, UnitKind::Sentinel, 7, 6),
        unit(0, UnitKind::Sentinel, 8, 6),
        unit(0, UnitKind::Sentinel, 9, 6),
    ]);
    for y in 1..scenario.map.len() - 1 {
        let mut row = scenario.map[y].as_bytes().to_vec();
        row[20] = b'#';
        scenario.map[y] = String::from_utf8(row).unwrap();
    }
    scenario.players[0].scrap = 0;
    let state = scenario.build().expect("the divided repair arena builds");
    let wounded = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
        .expect("the wounded veteran exists")
        .id;
    let tender = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Tender)
        .expect("the mobile welder exists")
        .id;
    let mut document = serde_json::to_value(state).unwrap();
    for unit in document["units"].as_array_mut().unwrap() {
        if unit["id"] == serde_json::json!(wounded.0) {
            unit["hp"] = serde_json::json!(1);
        }
    }
    let mut state: State = serde_json::from_value(document).unwrap();
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_305);
    let mut brain = Brain::scripted(PlayerId(0), scenario.seed, config);
    let mut saw_initial_muster = false;
    let mut saw_retreat = false;

    while state.current_tick() <= 1_320 {
        let commands = brain.act(&state);
        saw_initial_muster |= commands.iter().any(|command| {
            matches!(
                &command.command,
                Command::AttackMove { units, .. } if units.contains(&wounded)
            )
        });
        saw_retreat |= commands.iter().any(|command| {
            matches!(
                &command.command,
                Command::Move { units, queue: false, .. } if units == &[wounded]
            )
        });
        assert!(commands.iter().all(|command| !matches!(
            &command.command,
            Command::RepairUnit { target, .. } if *target == wounded
        )));
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
            "the repair lifecycle emitted an illegal command: {:?}",
            report.events
        );
        if saw_retreat {
            assert!(
                brain
                    .executive()
                    .armies()
                    .iter()
                    .all(|army| !army.members.contains(&wounded))
            );
        }
    }
    assert!(saw_initial_muster, "the wounded unit entered the real army");
    assert!(saw_retreat, "the real Brain rotated it to the rear");
    assert_eq!(state.unit(wounded).unwrap().hp, 1);

    let mut document = serde_json::to_value(&state).unwrap();
    document["players"][0]["scrap"] = serde_json::json!(10_000);
    state = serde_json::from_value(document).unwrap();
    let threshold = UnitKind::Sentinel.stats().max_hp * 3 / 4;
    let mut saw_repair_order = false;
    let mut reached_repair_threshold = false;

    for _ in 0..4_000 {
        let commands = brain.act(&state);
        saw_repair_order |= commands.iter().any(|command| {
            matches!(
                &command.command,
                Command::RepairUnit { units, target, queue: false }
                    if units == &[tender] && *target == wounded
            )
        });
        let hp_before = state.unit(wounded).unwrap().hp;
        if saw_retreat && hp_before < threshold {
            assert!(
                brain
                    .executive()
                    .armies()
                    .iter()
                    .all(|army| !army.members.contains(&wounded))
            );
        }
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
            "the funded repair lifecycle emitted an illegal command: {:?}",
            report.events
        );
        reached_repair_threshold |= state.unit(wounded).unwrap().hp >= threshold;
        if reached_repair_threshold
            && brain
                .executive()
                .armies()
                .iter()
                .any(|army| army.members.contains(&wounded))
        {
            assert!(saw_repair_order);
            return;
        }
    }

    panic!(
        "the funded Brain never completed repair and redrafted its veteran: repair={saw_repair_order}, hp={}, armies={:?}",
        state.unit(wounded).unwrap().hp,
        brain.executive().armies()
    );
}

#[test]
fn scripted_brain_saves_a_visible_expansion_with_its_local_defenders() {
    let expansion_anchor = TilePos::new(24, 12);
    let mut scenario = large_open_arena(vec![
        unit(0, UnitKind::Harvester, 3, 3),
        unit(0, UnitKind::Harvester, 4, 3),
        unit(0, UnitKind::Harvester, 5, 3),
        unit(0, UnitKind::Harvester, 6, 3),
        unit(0, UnitKind::Sentinel, 24, 16),
        unit(0, UnitKind::Sentinel, 25, 16),
        unit(0, UnitKind::Sentinel, 26, 16),
        unit(0, UnitKind::Sentinel, 27, 16),
        unit(1, UnitKind::Scuttler, 28, 13),
    ]);
    scenario.players[0].scrap = 0;
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: expansion_anchor.x,
        y: expansion_anchor.y,
    });
    let mut state = scenario
        .build()
        .expect("the expansion defense arena builds");
    let expansion = state
        .buildings()
        .iter()
        .find(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Foundry
                && building.anchor == expansion_anchor
        })
        .expect("the remote Foundry stands")
        .id;
    let intruder = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Scuttler)
        .expect("the expansion intruder exists")
        .id;
    let local_defenders: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
        .map(|unit| unit.id)
        .collect();
    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_200);
    let mut brain = Brain::scripted(PlayerId(0), scenario.seed, config);
    let mut saw_local_muster = false;
    let intruder_max_hp = state.unit(intruder).unwrap().kind.stats().max_hp;
    let mut saw_intruder_damage = false;
    let mut last_intruder_tile = TilePos::containing(state.unit(intruder).unwrap().pos);
    let mut destroyed_at = None;

    for _ in 0..1_200 {
        if let Some(unit) = state.unit(intruder) {
            last_intruder_tile = TilePos::containing(unit.pos);
        }
        let commands = brain.act(&state);
        saw_local_muster |= commands.iter().any(|command| {
            matches!(
                &command.command,
                Command::AttackMove { units, goal, queue: false }
                    if units.iter().any(|unit| local_defenders.contains(unit))
                        && goal.chebyshev(expansion_anchor) <= 5
            )
        });
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
            "the expansion defense emitted an illegal command: {:?}",
            report.events
        );
        saw_intruder_damage |= state
            .unit(intruder)
            .is_none_or(|unit| unit.hp < intruder_max_hp);

        if state.unit(intruder).is_none() {
            destroyed_at.get_or_insert(state.current_tick());
            let stale_target_released = brain.executive().armies().iter().all(|army| {
                army.target
                    .is_none_or(|target| target.chebyshev(last_intruder_tile) > 4)
            });
            if stale_target_released {
                break;
            }
        }
    }

    assert!(
        saw_local_muster,
        "the nearby screen must form at the expansion"
    );
    assert!(
        saw_intruder_damage,
        "the local screen never engaged the intruder after mustering"
    );
    assert!(
        destroyed_at.is_some(),
        "the local defense never cleared the raid"
    );
    assert!(
        state
            .building(expansion)
            .is_some_and(|building| building.hp > 0),
        "the expansion was lost despite a four-to-one local defense"
    );
    assert!(
        brain.executive().armies().iter().all(|army| {
            army.target
                .is_none_or(|target| target.chebyshev(last_intruder_tile) > 4)
        }),
        "the defense kept prosecuting a cleared local contact"
    );
}

#[test]
fn scripted_brain_answers_visible_siege_before_it_enters_the_home_radius() {
    let mut units: Vec<_> = (0..12)
        .map(|index| unit(0, UnitKind::Sentinel, 4 + index % 4, 4 + index / 4))
        .collect();
    let home_anchor = TilePos::new(1, 1);
    let siege_tile = TilePos::new(16, 2);
    units.push(unit(0, UnitKind::Harvester, 3, 2));
    units.push(unit(0, UnitKind::Kestrel, 15, 2));
    units.push(unit(1, UnitKind::Avalanche, siege_tile.x, siege_tile.y));
    let mut scenario = large_open_arena(units);
    scenario.players[0].scrap = 0;
    scenario.players[1].scrap = 0;
    let mut state = scenario.build().expect("the siege defense arena builds");
    let defenders: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
        .map(|unit| unit.id)
        .collect();
    let observation = Observation::fog_honest(&state, PlayerId(0));
    assert!(observation.visible(siege_tile));
    assert!(siege_tile.chebyshev(home_anchor) > 8);

    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_200);
    let mut brain = Brain::scripted(PlayerId(0), scenario.seed, config);
    let mut saw_response = false;
    let mut emitted = Vec::new();
    for _ in 0..240 {
        let commands = brain.act(&state);
        saw_response |= commands.iter().any(|command| {
            matches!(
                &command.command,
                Command::AttackMove { units, goal, queue: false }
                    if *goal == siege_tile
                        && units.iter().any(|unit| defenders.contains(unit))
            )
        });
        emitted.extend(commands.iter().cloned());
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
            "siege defense emitted an illegal command: {:?}",
            report.events
        );
        if saw_response {
            break;
        }
    }

    assert!(
        saw_response,
        "Prime saw an Avalanche inside its firing envelope but never sent the standing army: armies={:?}, commands={emitted:?}",
        brain.executive().armies()
    );
}

#[test]
fn scripted_brain_completes_a_real_raid_and_releases_the_pair_for_reuse() {
    let target_tile = TilePos::new(14, 8);
    let mut scenario = large_open_arena(vec![
        unit(0, UnitKind::Scuttler, 4, 4),
        unit(0, UnitKind::Scuttler, 5, 4),
        unit(0, UnitKind::Sentinel, 3, 6),
        unit(0, UnitKind::Sentinel, 4, 6),
        unit(0, UnitKind::Sentinel, 5, 6),
        unit(0, UnitKind::Sentinel, 6, 6),
        unit(1, UnitKind::Harvester, target_tile.x, target_tile.y),
    ]);
    scenario.players[0].scrap = 0;
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Array,
        x: 10,
        y: 10,
    });
    let mut state = scenario.build().expect("the raid lifecycle arena builds");
    let target = state
        .units()
        .iter()
        .find(|unit| {
            unit.player == PlayerId(1)
                && unit.kind == UnitKind::Harvester
                && unit.tile() == target_tile
        })
        .expect("the exposed worker exists")
        .id;
    let mut raiders: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Scuttler)
        .map(|unit| unit.id)
        .collect();
    raiders.sort_unstable();

    let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_203);
    let profile = config.resolve_profile();
    assert_eq!(profile.primary, Specialty::Guile);
    assert!(profile.traits.guile >= 65);
    let mut brain = Brain::scripted(PlayerId(0), scenario.seed, config);
    let home = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .expect("the raiders have a home Foundry")
        .anchor;
    let mut strike_at = None;
    let mut destroyed_at = None;
    let mut saw_egress = false;
    let mut released_at = None;

    for _ in 0..8_000 {
        let tick = state.current_tick();
        let commands = brain.act(&state);
        for command in &commands {
            match &command.command {
                Command::Attack {
                    units,
                    target: oxide_sim::Target::Unit(candidate),
                    queue: false,
                } if units == &raiders && *candidate == target => {
                    strike_at.get_or_insert(tick);
                }
                Command::Move {
                    units,
                    goal,
                    queue: false,
                } if units == &raiders && goal.chebyshev(home) <= 2 && destroyed_at.is_some() => {
                    saw_egress = true;
                }
                _ => {}
            }
        }
        if destroyed_at.is_none() {
            assert!(
                brain
                    .executive()
                    .armies()
                    .iter()
                    .all(|army| army.members.iter().all(|unit| !raiders.contains(unit))),
                "the generic army stole a member of the active raid"
            );
        }
        let report = state.tick(&commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. })),
            "the raid lifecycle emitted an illegal command: {:?}",
            report.events
        );
        if destroyed_at.is_none() && state.unit(target).is_none() {
            destroyed_at = Some(state.current_tick());
        }
        if destroyed_at.is_some()
            && brain
                .executive()
                .armies()
                .iter()
                .any(|army| raiders.iter().all(|raider| army.members.contains(raider)))
        {
            released_at = Some(state.current_tick());
            break;
        }
    }

    let strike = strike_at.expect("the exact raiding pair attacked the exposed worker");
    let destroyed = destroyed_at.expect("the raid completed its objective");
    assert!(strike < destroyed);
    assert!(
        saw_egress,
        "the raiding pair never received its egress order"
    );
    let released = released_at.unwrap_or_else(|| {
        panic!(
            "the completed raid never released its pair for ordinary reuse: tick={}, units={:?}, armies={:?}",
            state.current_tick(),
            raiders
                .iter()
                .map(|id| state.unit(*id).map(|unit| (id, unit.tile(), &unit.order)))
                .collect::<Vec<_>>(),
            brain.executive().armies()
        )
    });
    assert!(destroyed < released);
}

/// A three-seat arena: seats 1 and 2 duel deep in seat 0's fog, so any
/// artillery echo reaching seat 0's observation is a leak, not sight.
fn artillery_arena(observer_units: Vec<UnitSpec>) -> Scenario {
    let mut units = observer_units;
    units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Bombard,
        x: 13,
        y: 9,
    });
    units.push(UnitSpec {
        player: 2,
        kind: UnitKind::Harvester,
        x: 20,
        y: 9,
    });
    Scenario {
        name: "artillery-fog-arena".into(),
        seed: 42,
        map: vec![
            "########################".into(),
            "#1.....................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#..............2.......#".into(),
            "#......................#".into(),
            "#....................3.#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: (0..3)
            .map(|seat| PlayerSpec {
                name: format!("Seat{seat}"),
                faction: if seat % 2 == 0 {
                    Faction::Ferrous
                } else {
                    Faction::Cupric
                },
                team: None,
                scrap: 500,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

#[test]
fn artillery_in_fog_cannot_touch_a_fog_honest_observation() {
    // The incoming_shells filter is the only thing separating this
    // field from an omniscient read of every shell in flight; a
    // mutation test proved the old fog suite never exercised it (no
    // test ever fired a weapon). Control and variant differ only by a
    // bombardment entirely inside seat 0's fog.
    let scenario = artillery_arena(vec![unit(0, UnitKind::Harvester, 4, 2)]);
    let mut control = scenario.build().unwrap();
    let mut variant = scenario.build().unwrap();
    let bombard = variant.units()[1].id;
    let target = variant.units()[2].id;

    variant.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![bombard],
            target: oxide_sim::Target::Unit(target),
            queue: false,
        },
    )]);
    control.tick(&[]);
    let mut shells_flew = false;
    for _ in 0..60u32 {
        variant.tick(&[]);
        control.tick(&[]);
        shells_flew |= !variant.shells().is_empty();
        let a = serde_json::to_string(&Observation::fog_honest(&control, PlayerId(0))).unwrap();
        let b = serde_json::to_string(&Observation::fog_honest(&variant, PlayerId(0))).unwrap();
        assert_eq!(a, b, "fog-honest observation echoed artillery in fog");
    }
    assert!(shells_flew, "test premise: shells actually flew");
    assert_ne!(
        control.hash(),
        variant.hash(),
        "test premise: the worlds actually diverged"
    );
}

#[test]
fn a_watched_bombardment_reports_its_impact_tiles() {
    // The positive half: with the impact inside seat 0's vision, the
    // observation names exactly the hostile impact tiles, canonically
    // ordered, and my_shells counts only the viewer's own fire.
    let scenario = artillery_arena(vec![unit(0, UnitKind::Harvester, 19, 8)]);
    let mut state = scenario.build().unwrap();
    let bombard = state.units()[1].id;
    let target = state.units()[2].id;

    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![bombard],
            target: oxide_sim::Target::Unit(target),
            queue: false,
        },
    )]);
    let mut asserted = false;
    for _ in 0..60u32 {
        state.tick(&[]);
        if state.shells().is_empty() {
            continue;
        }
        let obs = Observation::fog_honest(&state, PlayerId(0));
        let vision = state.vision(PlayerId(0));
        let mut expected: Vec<TilePos> = state
            .shells()
            .iter()
            .filter(|s| s.player != PlayerId(0))
            .map(|s| TilePos::containing(s.impact))
            .filter(|t| vision.visible(*t))
            .collect();
        expected.sort_by_key(|p| (p.y, p.x));
        if expected.is_empty() {
            continue;
        }
        assert_eq!(obs.incoming_shells, expected);
        assert_eq!(obs.my_shells, 0, "seat 0 fired nothing");
        asserted = true;
    }
    assert!(asserted, "test premise: a visible impact was observed");
}
