//! Bot observation honesty and executive army-lifecycle contracts,
//! driven through the public API.

use chassis::grid::TilePos;
use oxide_sim::bot::{ArmyState, Brain, Executive, Intent, Observation};
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
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
        let a = serde_json::to_string(&Observation::fog_honest(&control, PlayerId(0))).unwrap();
        let b = serde_json::to_string(&Observation::fog_honest(&variant, PlayerId(0))).unwrap();
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
    assert!(fog.explored(peak));
    assert!(!fog.explored(TilePos::new(20, 10)));
    assert!(!fog.explored(TilePos::new(-1, peak.y)));
    assert!(!fog.explored(TilePos::new(fog.map_width, peak.y)));
    assert!(fog.known_rock.contains(&peak));
    assert!(fog.known_rock.contains(&rock));
    assert_eq!(fog.known_peaks, vec![peak]);

    let omniscient = Observation::omniscient(&state, PlayerId(0));
    assert!(omniscient.explored(TilePos::new(20, 10)));
    assert!(omniscient.known_rock.contains(&rock));
    assert!(omniscient.known_peaks.contains(&peak));
    assert!(!omniscient.known_peaks.contains(&rock));
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
fn wounded_members_remain_reserved_after_full_repair() {
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
        explored: vec![true; 24 * 13],
        known_scrap: Vec::new(),
        known_rock: Vec::new(),
        known_frames: Vec::new(),
        known_peaks: Vec::new(),
        known_wrecks: Vec::new(),
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
        "scripted maintenance retains the rear line"
    );
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
