//! Phase-A brain architecture tests: observation honesty and the
//! executive's army lifecycle, driven through the public API only.

use chassis::grid::TilePos;
use oxide_sim::bot::{ArmyState, Executive, Intent, Observation};
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::{Command, Faction, PlayerCommand, PlayerId, Scenario, State, UnitKind};

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
fn wounded_members_rejoin_after_full_repair() {
    // Executive semantics, pinned against a synthetic observation (the
    // executive is a pure function of what it is shown): a member below
    // the 35% pullback line and out of contact is Move-ordered to the
    // rear and dropped from the army; a wounded member still in a fight
    // is left in the line, and a fully healed rear member can be drafted
    // again.
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

    // The frozen scripted path retains even an externally healed rear
    // member, so its ladder behavior does not move.
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
        "scripted maintenance preserves the frozen rear line"
    );

    // A repair-capable policy releases the unit at full health. A
    // partial repair would leave it enlisted there and avoid oscillating
    // around the pullback threshold.
    let _ = exec.maintain_repair_capable(me, &healed, TilePos::new(1, 1));
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
            .any(|army| army.members.contains(&UnitId(0))),
        "a fully healed veteran returns to the draft pool"
    );
}
