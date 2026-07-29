//! Fog placement Part B (0.13): the gym bot emits deferred founding
//! where the intent predicate allows — the reclaim-parity rule, the
//! bot's Build reaching remembered ground exactly like the human's
//! armed click — and a walking founder is spoken for: the labor
//! choosers keep off it, and the Scout arm honors the think's claims
//! on the gym path. The scripted `Brain` tiers keep the strict instant
//! claim and the unconditional Scout arm — they are the ladder's
//! anchors and yardsticks, and their lowering must not move.

use chassis::grid::TilePos;
use oxide_sim::bot::{Action, GymBot};
use oxide_sim::{BuildingKind, Command, Order, PlayerCommand, PlayerId, Scenario, UnitKind};

/// A yard with one remembered prize: the only scrap sits ~20 tiles
/// east of home, far outside every own vision disc. A harvester must
/// walk out, see it, and walk home before the bot can want a turret
/// there — at which point the ground is remembered, not seen.
fn remembered_ridge() -> Scenario {
    let map = [
        "########################################",
        "#......................................#",
        "#......................................#",
        "#..1...................................#",
        "#......................................#",
        "#......................................#",
        "#.........................s............#",
        "#......................................#",
        "#......................................#",
        "#......................................#",
        "#..................................2...#",
        "#......................................#",
        "#......................................#",
        "########################################",
    ];
    let json = serde_json::json!({
        "name": "Remembered Ridge",
        "seed": 11,
        "players": [
            {"name": "Founder", "faction": "ferrous", "scrap": 500, "bot": false},
            {"name": "Idle", "faction": "cupric", "scrap": 0, "bot": false}
        ],
        "map": map,
        "units": [
            {"player": 0, "kind": "harvester", "x": 6, "y": 4},
            {"player": 0, "kind": "harvester", "x": 7, "y": 4}
        ]
    });
    Scenario::from_json(&json.to_string()).expect("the remembered ridge parses")
}

fn move_cmd(unit: u32, goal: TilePos) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(0),
        command: Command::Move {
            units: vec![oxide_sim::UnitId(unit)],
            goal,
            queue: false,
        },
    }
}

/// Walks unit 0 out to see the far node, then home again, so the node
/// is remembered but no longer visible. Returns the state at that
/// moment.
fn state_with_remembered_node() -> oxide_sim::State {
    let node = TilePos::new(26, 6);
    let mut state = remembered_ridge()
        .build()
        .expect("the remembered ridge builds");
    state.tick(&[move_cmd(0, TilePos::new(23, 6))]);
    for _ in 0..2_000u32 {
        state.tick(&[]);
        if state.vision(PlayerId(0)).visible(node) {
            break;
        }
    }
    assert!(
        state.vision(PlayerId(0)).visible(node),
        "the scout walk never brought the node into sight"
    );
    state.tick(&[move_cmd(0, TilePos::new(6, 5))]);
    for _ in 0..2_000u32 {
        state.tick(&[]);
        if !state.vision(PlayerId(0)).visible(node) {
            break;
        }
    }
    for _ in 0..2_000u32 {
        // Let the walk finish so the think below starts from rest.
        if state
            .units()
            .iter()
            .all(|u| u.player != PlayerId(0) || u.order == Order::Idle)
        {
            break;
        }
        state.tick(&[]);
    }
    assert!(
        !state.vision(PlayerId(0)).visible(node),
        "the node should be remembered, not seen"
    );
    assert!(
        state.vision(PlayerId(0)).remembered_scrap(node) > 0,
        "the node should be remembered"
    );
    state
}

/// The heart of Part B: a turret wanted at the remembered node lowers
/// to a deferred Build — the strict predicate refuses the unseen
/// footprint, the intent predicate allows it — the founder takes
/// `Order::Found`, walks out, and the turret actually stands.
#[test]
fn the_gym_bot_founds_on_remembered_ground() {
    let mut state = state_with_remembered_node();
    let mut gym = GymBot::new(PlayerId(0));

    let commands = gym.step(&state, Action::BuildTurret);
    let (anchor, defer) = commands
        .iter()
        .find_map(|pc| match &pc.command {
            Command::Build {
                kind: BuildingKind::Turret,
                anchor,
                defer,
                ..
            } => Some((*anchor, *defer)),
            _ => None,
        })
        .expect("the action staged its turret");
    assert!(
        defer,
        "a footprint outside current sight must lower with defer — the \
         same judgment the shell's armed click makes"
    );
    assert!(
        state
            .place_refusal(PlayerId(0), BuildingKind::Turret, anchor)
            .is_some(),
        "the pin is vacuous unless the strict predicate refuses this anchor"
    );
    assert!(
        state
            .place_intent_refusal(PlayerId(0), BuildingKind::Turret, anchor)
            .is_none(),
        "the bot may only defer where the intent predicate allows"
    );

    state.tick(&commands);
    assert!(
        state.units().iter().any(|u| u.order
            == Order::Found {
                kind: BuildingKind::Turret,
                anchor
            }),
        "the deferred command should hand the founder Order::Found"
    );

    for _ in 0..6_000u32 {
        let commands = if state.current_tick().is_multiple_of(16) {
            gym.step(&state, Action::Idle)
        } else {
            Vec::new()
        };
        state.tick(&commands);
        if state
            .buildings()
            .iter()
            .any(|b| b.anchor == anchor && b.kind == BuildingKind::Turret && b.built)
        {
            return;
        }
    }
    panic!("the walking claim never became a standing turret");
}

/// Walking founders are spoken for in the MASKS too: once every
/// harvester walks a Found, the verbs that need a free builder must
/// mask OFF. A mask that promises a verb the executive then refuses
/// feeds the blunder picker untrained logits, and the silent build
/// no-op would poison the pending-site ledger for a blameless anchor.
#[test]
fn walking_founders_mask_the_builder_verbs_off() {
    let mut state = state_with_remembered_node();
    let mut gym = GymBot::new(PlayerId(0));

    // One harvester walks a deferred claim to the remembered node; the
    // other takes a visible home-ring build and stands on its site —
    // every builder busy, each through a different guard (founding for
    // the walker, the site for the builder).
    // Stand the paid home site first. The capital planner deliberately
    // permits only one unpaid walking claim at a time, so reversing
    // these two orders would correctly reserve the distant Turret
    // before accepting another project.
    let first = gym.step(&state, Action::BuildRepairBay);
    state.tick(&first);
    let second = gym.step(&state, Action::BuildTurret);
    state.tick(&second);
    let founders = state
        .units()
        .iter()
        .filter(|u| matches!(u.order, Order::Found { .. }))
        .count();
    let builders = state
        .units()
        .iter()
        .filter(|u| matches!(u.order, Order::Build { .. }))
        .count();
    assert_eq!(
        (founders, builders),
        (1, 1),
        "one walking claim and one standing site must occupy both hands"
    );

    let decision = gym.decision(&state);
    for action in [
        Action::BuildFabricator,
        Action::BuildTurret,
        Action::BuildRepairBay,
    ] {
        assert!(
            !decision.mask[action as usize],
            "{action:?} masked legal while every builder walks a Found — \
             the mask and the lowering must share one judgment"
        );
    }
    // The lowering refuses symmetrically: forcing a build emits nothing
    // and records no pending site for audit_sites to poison later.
    let pending_before = state
        .units()
        .iter()
        .filter(|u| matches!(u.order, Order::Found { .. }))
        .count();
    let forced = gym.step(&state, Action::BuildRepairBay);
    assert!(
        forced
            .iter()
            .all(|pc| !matches!(pc.command, Command::Build { .. })),
        "a build without a free builder must lower to nothing"
    );
    state.tick(&forced);
    assert_eq!(
        state
            .units()
            .iter()
            .filter(|u| matches!(u.order, Order::Found { .. }))
            .count(),
        pending_before,
        "the refused lowering must not have disturbed the walking claims"
    );
}

/// A walking founder is spoken for: the Scout action must not strip
/// its `Order::Found` program (a plain Move replaces the whole
/// program, and the claim pays on arrival — losing it loses the site).
#[test]
fn the_scout_never_strips_a_walking_founder() {
    let mut state = remembered_ridge()
        .build()
        .expect("the remembered ridge builds");
    // A deferred build on visible ground is still a walk-and-claim:
    // hand unit 0 a Found program directly.
    let anchor = TilePos::new(10, 3);
    state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Build {
            units: vec![oxide_sim::UnitId(0)],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: true,
        },
    }]);
    let founder = oxide_sim::UnitId(0);
    assert!(
        state.units().iter().any(|u| u.id == founder
            && u.order
                == Order::Found {
                    kind: BuildingKind::Turret,
                    anchor
                }),
        "the fixture's founder never took its Found program"
    );

    let mut gym = GymBot::new(PlayerId(0));
    let commands = gym.step(&state, Action::Scout);
    let scouted = commands.iter().find_map(|pc| match &pc.command {
        Command::Move { units, .. } => units.first().copied(),
        _ => None,
    });
    assert!(
        scouted.is_some_and(|id| id != founder),
        "the scout pick took the walking founder (or nobody): {commands:?}"
    );
    state.tick(&commands);
    assert!(
        state.units().iter().any(|u| u.id == founder
            && u.order
                == Order::Found {
                    kind: BuildingKind::Turret,
                    anchor
                }),
        "the founder lost its claim to a scout order"
    );
}

/// Executive-level pins for the per-path lowering rules: the gym path
/// guards the Scout arm against the think's claims and keeps labor
/// choosers off walking founders; the scripted path keeps the exact
/// unconditional lowering the ladder anchors were measured under.
mod lowering_rules {
    use super::*;
    use oxide_sim::UnitId;
    use oxide_sim::bot::observation::OBSERVATION_VERSION;
    use oxide_sim::bot::{Executive, Intent, LoweringRules, Observation, UnitObs};

    fn obs_with(units: Vec<UnitObs>) -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 500,
            map_width: 32,
            map_height: 20,
            my_units: units,
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_wrecks: Vec::new(),
            blips: Vec::new(),
            faction: oxide_sim::Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }

    fn harvester(id: u32, x: i32, y: i32) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(x, y),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: true,
            carrying: 0,
            site: None,
            salvaging: None,
            founding: None,
        }
    }

    /// The labor-claims trap, pinned from both sides: a Scout naming
    /// the machine an earlier Build just bought is dropped under the
    /// gym rules and emitted verbatim under the scripted baseline —
    /// the one-line shared guard measurably inverts both ladder gates,
    /// which is why the paths differ on purpose.
    #[test]
    fn the_scout_arm_honors_claims_on_the_gym_path_only() {
        let obs = obs_with(vec![harvester(0, 5, 5)]);
        let intents = [
            Intent::Build {
                kind: BuildingKind::Fabricator,
                anchor: TilePos::new(6, 5),
            },
            Intent::Scout {
                unit: UnitId(0),
                to: TilePos::new(20, 15),
            },
        ];

        let never = |_: BuildingKind, _: TilePos| false;
        let commands =
            Executive::new().apply_with(PlayerId(0), &obs, &intents, &LoweringRules::gym(&never));
        assert!(
            commands
                .iter()
                .all(|pc| !matches!(pc.command, Command::Move { .. })),
            "the gym Scout arm re-tasked the claimed builder: {commands:?}"
        );

        let commands = Executive::new().apply(PlayerId(0), &obs, &intents);
        assert!(
            commands
                .iter()
                .any(|pc| matches!(pc.command, Command::Move { .. })),
            "the scripted Scout arm changed: the ladder anchors were \
             measured under the unconditional emission"
        );
    }

    /// The builder chooser skips a walking founder even when it is the
    /// nearest harvester — its claim pays on arrival, and re-tasking
    /// it drops the promise silently.
    #[test]
    fn the_builder_chooser_keeps_off_walking_founders() {
        let mut near = harvester(0, 9, 4);
        near.idle = false;
        near.founding = Some((BuildingKind::Turret, TilePos::new(10, 3)));
        let far = harvester(1, 2, 2);
        let obs = obs_with(vec![near, far]);
        let intents = [Intent::Build {
            kind: BuildingKind::Fabricator,
            anchor: TilePos::new(10, 5),
        }];
        let commands = Executive::new().apply(PlayerId(0), &obs, &intents);
        let builder = commands
            .iter()
            .find_map(|pc| match &pc.command {
                Command::Build { units, .. } => units.first().copied(),
                _ => None,
            })
            .expect("the build lowered");
        assert_eq!(
            builder,
            UnitId(1),
            "the chooser took the walking founder instead of the free hand"
        );
    }
}
