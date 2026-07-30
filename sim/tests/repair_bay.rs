//! The Repair Bay's welding aura: billed sustain in a ring, through the
//! same buffered resolver as every unit heal. Headless scenarios through
//! the public API only, like `repair_unit.rs` — billing exactness against
//! the hp-anchored meter, broke-owner starvation, the ring's edge, air
//! patients, own-units-only scope, and fire winning the tick.

use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::{
    BuildingKind, Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, UnitId,
    UnitKind,
};

/// Bay footprint anchored at (4,4): its rect spans world [4,6]x[4,6], so
/// a tile-centered patient sits at distance k + 0.5 from the east face —
/// 3.5 tiles at x=9 (inside the 4.0 ring), 4.5 at x=10 (outside).
const BAY_ANCHOR: (i32, i32) = (4, 4);

/// In the ring, one tile off the bay's east face.
const RING: TilePos = TilePos { x: 7, y: 4 };

/// Well outside the ring — where the wounding happens, so no pulse
/// bills while the raider works.
const FAR: TilePos = TilePos { x: 14, y: 4 };

fn arena(units: Vec<UnitSpec>, factions: [Faction; 2], scrap: u32, bay: bool) -> Scenario {
    let buildings = if bay {
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::RepairBay,
            x: BAY_ANCHOR.0,
            y: BAY_ANCHOR.1,
        }]
    } else {
        Vec::new()
    };
    Scenario {
        name: "bay-arena".into(),
        seed: 42,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#................2.#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Owner".into(),
                faction: factions[0],
                team: None,
                scrap,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Raider".into(),
                faction: factions[1],
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings,
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

fn walk(player: u8, units: Vec<UnitId>, goal: TilePos) -> PlayerCommand {
    cmd(
        player,
        Command::Move {
            units,
            goal,
            queue: false,
        },
    )
}

fn run_until(
    state: &mut State,
    max_ticks: u64,
    mut stop: impl FnMut(&State, &[Event]) -> bool,
) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..max_ticks {
        let report = state.tick(&[]);
        let done = stop(state, &report.events);
        all.extend(report.events);
        if done {
            return all;
        }
    }
    panic!("condition not reached within {max_ticks} ticks");
}

/// Walks the raider beside the patient at FAR, lets auto-acquire gnaw to
/// at most `floor` hp, then pulls the raider back to its corner. Returns
/// the wound.
fn wound(state: &mut State, patient: UnitId, raider: UnitId, floor: u32) -> u32 {
    let max = state.unit(patient).unwrap().kind.stats().max_hp;
    state.tick(&[walk(1, vec![raider], TilePos::new(FAR.x, FAR.y - 2))]);
    run_until(state, 3_000, |s, _| s.unit(patient).unwrap().hp <= floor);
    state.tick(&[walk(1, vec![raider], TilePos::new(17, 10))]);
    run_until(state, 800, |s, _| {
        s.unit(raider).unwrap().tile() == TilePos::new(17, 10)
    });
    let hp = state.unit(patient).unwrap().hp;
    assert!(
        hp > 0 && hp < max,
        "test premise: the gnawing must leave a live patient (hp {hp})"
    );
    hp
}

/// The aura's whole bill for healing `from..=max` hp of a kind, in
/// scrap: the hp-anchored milli-scrap meter's ceiling telescoped across
/// the span — the same integer arithmetic the sim runs.
fn aura_bill(kind: UnitKind, from: u32, to: u32) -> u32 {
    let stats = kind.stats();
    let millis = |hp: u32| -> u64 {
        u64::from(hp) * u64::from(stats.cost) * oxide_sim::stats::REPAIR_COST_PERMILLE
            / u64::from(stats.max_hp)
    };
    (millis(to).div_ceil(1000) - millis(from).div_ceil(1000)) as u32
}

fn wounded_ring_patient(kind: UnitKind, hp: u32, scrap: u32, overlap: bool) -> State {
    let pos = if overlap {
        TilePos::new(BAY_ANCHOR.0 + 2, BAY_ANCHOR.1 + 3)
    } else {
        RING
    };
    let mut scenario = arena(
        vec![unit(0, kind, pos.x, pos.y)],
        [Faction::Ferrous, Faction::Cupric],
        scrap,
        true,
    );
    if overlap {
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::RepairBay,
            x: BAY_ANCHOR.0 + 3,
            y: BAY_ANCHOR.1,
        });
    }
    let mut json = serde_json::to_value(scenario.build().unwrap()).unwrap();
    json["units"][0]["hp"] = serde_json::json!(hp);
    serde_json::from_value(json).unwrap()
}

#[test]
fn the_aura_heals_the_ring_to_whole_and_bills_the_welders_exact_price() {
    let units = vec![
        unit(0, UnitKind::Harvester, FAR.x, FAR.y), // patient
        unit(1, UnitKind::Scuttler, 17, 10),        // raider
    ];
    let mut state = arena(units, [Faction::Ferrous, Faction::Cupric], 500, true)
        .build()
        .unwrap();
    let (patient, raider) = (state.units()[0].id, state.units()[1].id);
    let hurt = wound(&mut state, patient, raider, 30);
    let bank = state.player(PlayerId(0)).scrap;
    state.tick(&[walk(0, vec![patient], RING)]);
    run_until(&mut state, 4_000, |s, _| {
        s.unit(patient).unwrap().hp == UnitKind::Harvester.stats().max_hp
    });
    let max = UnitKind::Harvester.stats().max_hp;
    let billed = bank - state.player(PlayerId(0)).scrap;
    assert_eq!(
        billed,
        aura_bill(UnitKind::Harvester, hurt, max),
        "a full aura heal must telescope to the meter's exact price"
    );
    // The dearest-verb doctrine holds in the ring: sustaining hp costs
    // strictly more per hp than salvage's 800 permille refunds, so no
    // heal-then-liquidate loop can profit.
    let healed = max - hurt;
    let salvage_value = (u64::from(healed)
        * u64::from(UnitKind::Harvester.stats().cost)
        * oxide_sim::stats::SALVAGE_REFUND_PERMILLE
        / (1000 * u64::from(max))) as u32;
    assert!(
        billed > salvage_value,
        "aura sustain ({billed}) must out-price a salvage-permille valuation ({salvage_value})"
    );
    // Full means done: the bank holds still once nothing is wounded.
    let settled = state.player(PlayerId(0)).scrap;
    for _ in 0..64 {
        state.tick(&[]);
    }
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        settled,
        "a whole ring bills nothing"
    );
}

#[test]
fn overlapping_bays_stack_the_heal_and_telescope_the_bill_once() {
    // Two bays whose auras both cover the patient's parking spot: the
    // heals stack (2 hp per pulse) and the bill must telescope across
    // them as ONE meter — each bay pricing from start-of-tick hp
    // double-charged (or skipped) the shared interval.
    let mut scenario = arena(
        vec![
            unit(0, UnitKind::Harvester, FAR.x, FAR.y),
            unit(1, UnitKind::Scuttler, 17, 10),
        ],
        [Faction::Ferrous, Faction::Cupric],
        500,
        true,
    );
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::RepairBay,
        x: BAY_ANCHOR.0 + 3,
        y: BAY_ANCHOR.1,
    });
    let mut state = scenario.build().unwrap();
    assert_eq!(
        state
            .buildings()
            .iter()
            .filter(|b| b.kind == BuildingKind::RepairBay && b.built)
            .count(),
        2,
        "the second bay must actually stand"
    );
    let (patient, raider) = (state.units()[0].id, state.units()[1].id);
    let hurt = wound(&mut state, patient, raider, 20);
    let bank = state.player(PlayerId(0)).scrap;
    // Park between the two footprints, inside both auras.
    let overlap = TilePos::new(BAY_ANCHOR.0 + 2, BAY_ANCHOR.1 + 3);
    state.tick(&[walk(0, vec![patient], overlap)]);
    // The march grazes single-coverage fringe (auras heal walkers
    // too), so anchor the stacking measurement to ARRIVAL, then to a
    // pulse boundary.
    run_until(&mut state, 2_000, |s, _| {
        s.unit(patient).unwrap().tile() == overlap
            && s.unit(patient).unwrap().order == oxide_sim::Order::Idle
    });
    run_until(&mut state, 32, |s, _| {
        s.current_tick()
            .is_multiple_of(oxide_sim::stats::REPAIR_BAY_PERIOD)
    });
    // One full pulse window with both auras engaged heals two steps.
    let h0 = state.unit(patient).unwrap().hp;
    let max = UnitKind::Harvester.stats().max_hp;
    if h0 + 2 * oxide_sim::stats::REPAIR_BAY_STEP <= max {
        for _ in 0..oxide_sim::stats::REPAIR_BAY_PERIOD {
            state.tick(&[]);
        }
        assert_eq!(
            state.unit(patient).unwrap().hp,
            h0 + 2 * oxide_sim::stats::REPAIR_BAY_STEP,
            "both auras must stack on the shared patient"
        );
    }
    run_until(&mut state, 4_000, |s, _| s.unit(patient).unwrap().hp == max);
    let billed = bank - state.player(PlayerId(0)).scrap;
    assert_eq!(
        billed,
        aura_bill(UnitKind::Harvester, hurt, max),
        "overlapping auras must bill the meter exactly once end to end"
    );
}

#[test]
fn a_broke_owner_gets_no_healing() {
    let units = vec![
        unit(0, UnitKind::Harvester, FAR.x, FAR.y),
        unit(1, UnitKind::Scuttler, 17, 10),
    ];
    let mut state = arena(units, [Faction::Ferrous, Faction::Cupric], 0, true)
        .build()
        .unwrap();
    let (patient, raider) = (state.units()[0].id, state.units()[1].id);
    // The scuttler's 3-damage bites land the harvester on 18 hp, where
    // the next step's ceiling-diff is a whole scrap — no free fractional
    // step for a broke bank to ride.
    let hurt = wound(&mut state, patient, raider, 20);
    assert_eq!(aura_bill(UnitKind::Harvester, hurt, hurt + 1), 1);
    state.tick(&[walk(0, vec![patient], RING)]);
    run_until(&mut state, 800, |s, _| {
        s.unit(patient).unwrap().tile() == RING
    });
    for _ in 0..200 {
        state.tick(&[]);
    }
    assert_eq!(
        state.unit(patient).unwrap().hp,
        hurt,
        "no scrap, no welding"
    );
    assert_eq!(state.player(PlayerId(0)).scrap, 0);
}

#[test]
fn the_aura_cannot_spend_the_emergency_harvester_reserve() {
    // Put a wounded non-Harvester directly in the ring through the
    // validated state boundary. On tick zero the Foundry supplies the
    // 50th scrap before the aura runs; the automatic repair must leave
    // that coin and the patient alone.
    let scenario = arena(
        vec![unit(0, UnitKind::Sentinel, RING.x, RING.y)],
        [Faction::Ferrous, Faction::Cupric],
        UnitKind::Harvester.stats().cost - 1,
        true,
    );
    let state = scenario.build().unwrap();
    let max = UnitKind::Sentinel.stats().max_hp;
    let hurt = (1..max)
        .find(|hp| aura_bill(UnitKind::Sentinel, *hp, *hp + 1) == 1)
        .expect("some Sentinel hp step costs one scrap");
    let mut json = serde_json::to_value(state).unwrap();
    json["units"][0]["hp"] = serde_json::json!(hurt);
    let mut state: State = serde_json::from_value(json).unwrap();

    state.tick(&[]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        UnitKind::Harvester.stats().cost,
        "the 50th recovery scrap must survive the same tick's aura"
    );
    assert_eq!(
        state.units()[0].hp,
        hurt,
        "paid sustain pauses while the economy is stranded"
    );
}

#[test]
fn multi_scrap_pulses_preserve_the_whole_harvester_reserve() {
    let reserve = UnitKind::Harvester.stats().cost;
    // Bombard hp steps cost either two or three scrap. One scrap less
    // than the required surplus must block the whole pulse; exactly the
    // required surplus may be spent down to the intact reserve.
    for (hp, due) in [(1, 2), (8, 3)] {
        assert_eq!(aura_bill(UnitKind::Bombard, hp, hp + 1), due);

        let mut blocked = wounded_ring_patient(UnitKind::Bombard, hp, reserve + due - 1, false);
        blocked.tick(&[]);
        assert_eq!(blocked.units()[0].hp, hp);
        assert_eq!(
            blocked.player(PlayerId(0)).scrap,
            reserve + due - 1,
            "a {due}-scrap pulse must not borrow from the reserve"
        );

        let mut funded = wounded_ring_patient(UnitKind::Bombard, hp, reserve + due, false);
        funded.tick(&[]);
        assert_eq!(funded.units()[0].hp, hp + 1);
        assert_eq!(
            funded.player(PlayerId(0)).scrap,
            reserve,
            "the exact surplus remains spendable"
        );
    }
}

#[test]
fn overlapping_bays_recheck_the_reserve_after_each_charge() {
    let reserve = UnitKind::Harvester.stats().cost;
    let hp = 1;
    assert_eq!(aura_bill(UnitKind::Bombard, hp, hp + 1), 2);
    assert_eq!(aura_bill(UnitKind::Bombard, hp + 1, hp + 2), 2);

    // The first bay may spend two of the three surplus scrap. The second
    // must price from that post-charge bank and leave the remaining 51
    // untouched instead of taking it below the replacement price.
    let mut state = wounded_ring_patient(UnitKind::Bombard, hp, reserve + 3, true);
    state.tick(&[]);
    assert_eq!(state.units()[0].hp, hp + 1);
    assert_eq!(state.player(PlayerId(0)).scrap, reserve + 1);
}

#[test]
fn partial_scrap_heals_the_earliest_id_first_then_starves() {
    let units = vec![
        unit(0, UnitKind::Harvester, FAR.x, FAR.y), // first patient
        unit(0, UnitKind::Harvester, FAR.x, FAR.y + 2), // second patient
        unit(1, UnitKind::Scuttler, 17, 10),
    ];
    let mut state = arena(units, [Faction::Ferrous, Faction::Cupric], 5, true)
        .build()
        .unwrap();
    let (a, b, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt_a = wound(&mut state, a, raider, 20);
    let hurt_b = {
        state.tick(&[walk(1, vec![raider], TilePos::new(FAR.x, FAR.y + 4))]);
        run_until(&mut state, 3_000, |s, _| s.unit(b).unwrap().hp <= 20);
        state.tick(&[walk(1, vec![raider], TilePos::new(17, 10))]);
        run_until(&mut state, 800, |s, _| {
            s.unit(raider).unwrap().tile() == TilePos::new(17, 10)
        });
        state.unit(b).unwrap().hp
    };
    state.tick(&[walk(0, vec![a], RING)]);
    state.tick(&[walk(0, vec![b], TilePos::new(RING.x, RING.y + 1))]);
    // Five scrap buys a handful of pulses split in id order, then the
    // bank is dry and every whole-coin step skips its patient.
    run_until(&mut state, 2_000, |s, _| s.player(PlayerId(0)).scrap == 0);
    for _ in 0..200 {
        state.tick(&[]);
    }
    let (hp_a, hp_b) = (state.unit(a).unwrap().hp, state.unit(b).unwrap().hp);
    let max = UnitKind::Harvester.stats().max_hp;
    assert!(hp_a > hurt_a, "the first coins reach the earliest id");
    assert!(hp_a < max && hp_b < max, "five scrap fully heals nobody");
    assert!(
        hp_a - hurt_a >= hp_b - hurt_b,
        "id order spends the last coins on the earlier patient (a: {hurt_a}->{hp_a}, b: {hurt_b}->{hp_b})"
    );
}

#[test]
fn the_ring_ends_where_the_radius_says() {
    let units = vec![
        unit(0, UnitKind::Harvester, FAR.x, FAR.y),
        unit(1, UnitKind::Scuttler, 17, 10),
    ];
    let mut state = arena(units, [Faction::Ferrous, Faction::Cupric], 500, true)
        .build()
        .unwrap();
    let (patient, raider) = (state.units()[0].id, state.units()[1].id);
    let hurt = wound(&mut state, patient, raider, 20);
    // 4.5 tiles off the bay's east face: outside the 4.0 ring, starved.
    let outside = TilePos::new(BAY_ANCHOR.0 + 6, RING.y);
    state.tick(&[walk(0, vec![patient], outside)]);
    run_until(&mut state, 800, |s, _| {
        s.unit(patient).unwrap().tile() == outside
    });
    for _ in 0..200 {
        state.tick(&[]);
    }
    assert_eq!(
        state.unit(patient).unwrap().hp,
        hurt,
        "4.5 tiles out is outside the ring"
    );
    // One tile closer — 3.5 out — is inside, and the torch lights.
    let inside = TilePos::new(BAY_ANCHOR.0 + 5, RING.y);
    state.tick(&[walk(0, vec![patient], inside)]);
    run_until(&mut state, 800, |s, _| {
        s.unit(patient).unwrap().tile() == inside
    });
    run_until(&mut state, 200, |s, _| s.unit(patient).unwrap().hp > hurt);
}

#[test]
fn the_aura_serves_the_sky_too() {
    // The wisp parks over the bay itself — ground occupancy means
    // nothing to a flyer, and the ring reads pure distance.
    let units = vec![
        unit(0, UnitKind::Wisp, FAR.x, FAR.y),
        unit(1, UnitKind::Stinger, 17, 10),
    ];
    let mut state = arena(units, [Faction::Cupric, Faction::Cupric], 500, true)
        .build()
        .unwrap();
    let (patient, raider) = (state.units()[0].id, state.units()[1].id);
    let hurt = wound(&mut state, patient, raider, 25);
    state.tick(&[walk(
        0,
        vec![patient],
        TilePos::new(BAY_ANCHOR.0, BAY_ANCHOR.1),
    )]);
    run_until(&mut state, 1_000, |s, _| s.unit(patient).unwrap().hp > hurt);
}

#[test]
fn the_ring_welds_own_machines_only() {
    let units = vec![
        unit(0, UnitKind::Scuttler, 17, 10),        // seat 0's wounder
        unit(1, UnitKind::Harvester, FAR.x, FAR.y), // seat 1's wounded
    ];
    let mut state = arena(units, [Faction::Ferrous, Faction::Cupric], 500, true)
        .build()
        .unwrap();
    let (wounder, foreign) = (state.units()[0].id, state.units()[1].id);
    // Gnaw seat 1's harvester with seat 0's scuttler, then send both
    // ways: the scuttler home, the foreign patient into the ring.
    state.tick(&[walk(0, vec![wounder], TilePos::new(FAR.x, FAR.y - 2))]);
    run_until(&mut state, 3_000, |s, _| s.unit(foreign).unwrap().hp <= 30);
    state.tick(&[walk(0, vec![wounder], TilePos::new(17, 10))]);
    run_until(&mut state, 800, |s, _| {
        s.unit(wounder).unwrap().tile() == TilePos::new(17, 10)
    });
    let hurt = state.unit(foreign).unwrap().hp;
    let bank = state.player(PlayerId(0)).scrap;
    state.tick(&[walk(1, vec![foreign], RING)]);
    run_until(&mut state, 800, |s, _| {
        s.unit(foreign).unwrap().tile() == RING
    });
    for _ in 0..200 {
        state.tick(&[]);
    }
    assert_eq!(
        state.unit(foreign).unwrap().hp,
        hurt,
        "a hostile machine in the ring is not a patient"
    );
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank,
        "and nobody was billed for it"
    );
}

#[test]
fn an_unbuilt_bay_is_inert() {
    let units = vec![
        unit(0, UnitKind::Harvester, FAR.x, FAR.y), // patient
        unit(0, UnitKind::Harvester, 2, 6),         // founder
        unit(1, UnitKind::Scuttler, 17, 10),
    ];
    let mut state = arena(units, [Faction::Ferrous, Faction::Cupric], 500, false)
        .build()
        .unwrap();
    let (patient, founder, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    // Found the bay, then call the founder off: the site stands at a
    // fifth of max hp, blind and inert, forever.
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![founder],
            kind: BuildingKind::RepairBay,
            anchor: TilePos::new(BAY_ANCHOR.0, BAY_ANCHOR.1),
            queue: false,
            defer: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![founder],
        },
    )]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::RepairBay && !b.built),
        "test premise: the site stands unbuilt"
    );
    let hurt = wound(&mut state, patient, raider, 20);
    let bank = state.player(PlayerId(0)).scrap;
    state.tick(&[walk(0, vec![patient], RING)]);
    run_until(&mut state, 800, |s, _| {
        s.unit(patient).unwrap().tile() == RING
    });
    for _ in 0..200 {
        state.tick(&[]);
    }
    assert_eq!(
        state.unit(patient).unwrap().hp,
        hurt,
        "scaffolding heals nothing"
    );
    assert_eq!(state.player(PlayerId(0)).scrap, bank);
}

#[test]
fn fire_wins_the_tick_and_the_dead_forfeit_the_aura() {
    let units = vec![
        unit(0, UnitKind::Harvester, FAR.x, FAR.y),
        unit(1, UnitKind::Scuttler, 17, 10),
        unit(1, UnitKind::Scuttler, 15, 10),
    ];
    let mut state = arena(units, [Faction::Ferrous, Faction::Cupric], 500, true)
        .build()
        .unwrap();
    let (patient, r1, r2) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt = wound(&mut state, patient, r1, 30);
    state.tick(&[walk(0, vec![patient], RING)]);
    run_until(&mut state, 400, |s, _| s.unit(patient).unwrap().hp > hurt);
    // Two gnawing scuttlers (20 hp/s) walk into the ring and out-pace
    // the aura (2.5 hp/s): buffered heals land only on machines the
    // volley left standing, so the ring never outbids the guns on the
    // tick they win.
    for raider in [r1, r2] {
        state.tick(&[walk(1, vec![raider], TilePos::new(RING.x + 1, RING.y))]);
    }
    run_until(&mut state, 2_000, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == patient))
    });
    assert!(state.unit(patient).is_none(), "nothing resurrects");
    // A corpse leaves the ring's books: the bank holds still once the
    // raiders are the only machines near the bay.
    let bank = state.player(PlayerId(0)).scrap;
    for _ in 0..100 {
        state.tick(&[]);
    }
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank,
        "the aura bills no dead patient"
    );
}
