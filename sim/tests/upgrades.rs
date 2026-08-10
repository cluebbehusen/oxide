//! In-place building upgrades: the tier ladder. An upgrade charges its
//! full price, takes the works offline as a site on the new tier's row,
//! ramps under ordinary harvester labor, and stands back up with the new
//! tier's numbers. Upgrades cannot be cancelled, and the deepest rungs
//! sit behind the Crucible.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, RECLAIMER_PERIOD};
use oxide_sim::{Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, UnitKind};

fn players(scrap: u32) -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
    ]
}

/// Seat 0 with a Fabricator, a Turret, and a harvester crew — the
/// Reclaimer joins only where its income cannot smudge exact-bank math.
fn arena(scrap: u32, crucible: bool, reclaimer: bool) -> Scenario {
    let mut buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 2,
            y: 5,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 8,
            y: 3,
        },
    ];
    if reclaimer {
        buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 6,
            y: 6,
        });
    }
    if crucible {
        buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Crucible,
            x: 11,
            y: 5,
        });
    }
    Scenario {
        name: "upgrade-arena".into(),
        seed: 3,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#...............2..#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: players(scrap),
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 6,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 5,
                y: 6,
            },
        ],
        buildings,
        meta: None,
    }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn upgrade(builder: oxide_sim::UnitId, building: oxide_sim::BuildingId) -> PlayerCommand {
    cmd(
        0,
        Command::UpgradeBuilding {
            units: vec![builder],
            building,
            queue: false,
        },
    )
}

fn find(state: &State, kind: BuildingKind) -> oxide_sim::BuildingId {
    state
        .buildings()
        .iter()
        .find(|b| b.kind == kind)
        .unwrap()
        .id
}

fn run_until_built(state: &mut State, building: oxide_sim::BuildingId, cap: u64) {
    for _ in 0..cap {
        state.tick(&[]);
        if state.building(building).is_some_and(|b| b.built) {
            return;
        }
    }
    panic!("the upgrade never completed within {cap} ticks");
}

#[test]
fn the_ladder_lifts_and_the_numbers_follow() {
    let mut state = arena(2_000, true, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let builder = state.units()[0].id;

    let bank = state.player(PlayerId(0)).scrap;
    let heavy = BuildingKind::Turret.upgrade_from(0).unwrap();
    state.tick(&[upgrade(builder, turret)]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank - heavy.cost,
        "the upgrade charges its full price on commitment"
    );
    {
        let b = state.building(turret).unwrap();
        assert!(!b.built, "the works goes offline as a site");
        assert_eq!(b.tier, 1, "the tier bumps at commitment");
    }
    run_until_built(&mut state, turret, 2_000);
    {
        let b = state.building(turret).unwrap();
        assert_eq!(b.stats().max_hp, 500, "heavy turret hit points");
        assert_eq!(b.stats().weapons[0].damage, 20, "heavy turret gun");
        assert_eq!(b.hp, b.stats().max_hp, "the ramp tops out at the new max");
    }

    // The second rung: Bulwark, behind the Crucible (present here).
    state.tick(&[upgrade(builder, turret)]);
    run_until_built(&mut state, turret, 3_000);
    let b = state.building(turret).unwrap();
    assert_eq!(b.tier, 2);
    assert_eq!(b.stats().max_hp, 900, "bulwark hit points");
    assert_eq!(b.stats().weapons[0].damage, 60, "bulwark gun");
    assert_eq!(b.kind.tier_name(b.tier), "bulwark");
}

#[test]
fn the_deepest_rung_waits_for_the_crucible() {
    let mut state = arena(2_000, false, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let builder = state.units()[0].id;
    state.tick(&[upgrade(builder, turret)]);
    run_until_built(&mut state, turret, 2_000);

    let report = state.tick(&[upgrade(builder, turret)]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::MissingPrerequisite,
                ..
            }
        )),
        "the Bulwark waits for the Crucible"
    );
    assert_eq!(state.building(turret).unwrap().tier, 1, "nothing moved");
}

#[test]
fn upgrades_refuse_the_broke_the_topped_out_and_the_cancel() {
    let mut state = arena(60, true, false).build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let fabricator = find(&state, BuildingKind::Fabricator);
    let builder = state.units()[0].id;

    // Too poor for the 150-scrap Heavy Turret.
    let report = state.tick(&[upgrade(builder, turret)]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::NotEnoughScrap,
            ..
        }
    )));

    // A kind with no ladder refuses as an invalid target.
    let report = state.tick(&[upgrade(builder, fabricator)]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::InvalidTarget,
            ..
        }
    )));
}

#[test]
fn an_upgrading_works_is_committed_offline_and_mortal() {
    let mut scenario = arena(2_000, false, false);
    scenario.units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Sentinel,
        x: 12,
        y: 3,
    });
    let mut state = scenario.build().unwrap();
    let turret = find(&state, BuildingKind::Turret);
    let builder = state.units()[0].id;
    let raider = state.units()[2].id;
    state.tick(&[upgrade(builder, turret)]);
    // Call the crew off: the works stays an offline site for the whole
    // fight (attended, it would legitimately finish mid-battle and
    // shoot back — that race is the defender's gamble, not this test).
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 2),
            queue: false,
        },
    )]);

    // No cancel: the machine is committed until it stands.
    let report = state.tick(&[cmd(0, Command::Cancel { building: turret })]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "an upgrade cannot be cancelled"
    );

    // Offline: the site never fires while the raider grinds it down.
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![raider],
            target: oxide_sim::Target::Building(turret),
            queue: false,
        },
    )]);
    let mut destroyed = false;
    for _ in 0..4_000 {
        let report = state.tick(&[]);
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::TurretFired { turret: t, .. } if *t == turret)),
            "an upgrading works must not fire"
        );
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { building, .. } if *building == turret))
        {
            destroyed = true;
            break;
        }
    }
    assert!(destroyed, "the vulnerability window is real");
}

#[test]
fn a_refinery_grinds_faster_than_its_reclaimer() {
    let mut state = arena(2_000, false, true).build().unwrap();
    let reclaimer = find(&state, BuildingKind::Reclaimer);
    let builder = state.units()[1].id;
    state.tick(&[upgrade(builder, reclaimer)]);
    run_until_built(&mut state, reclaimer, 2_000);
    assert_eq!(state.building(reclaimer).unwrap().tier, 1);

    // The refinery credits on the shared Reclaimer clock but at its own
    // richer rate — measure one long window against the base rate.
    let bank = state.player(PlayerId(0)).scrap;
    let window = 240u64;
    for _ in 0..window {
        state.tick(&[]);
    }
    let earned = state.player(PlayerId(0)).scrap - bank;
    let base_rate = (window / RECLAIMER_PERIOD) as u32;
    assert!(
        earned > base_rate,
        "a refinery out-earns the reclaimer clock ({earned} vs base {base_rate})"
    );
}
