//! Command fuzzing: the sim's command surface faces the debug socket, so
//! it must shrug off hostile input — garbage ids, repeated ids,
//! out-of-range players, coordinates at the integer extremes, queues
//! driven past their caps — without panicking, while every state it
//! reaches along the way still satisfies
//! [`oxide_sim::State::validate_invariants`], and the same seeded garbage
//! must produce the same bits on every run.
//!
//! Two properties are load-bearing about *how* the garbage is made. The
//! generator is exhaustive over [`Command`]: a new verb stops this file
//! compiling until it is fuzzed, which is how three commands escaped the
//! old hand-written arm list. And the stream is drawn from
//! [`chassis::rng::Pcg32`] alone, so a failure replays from its seed —
//! which the sweep names on any panic.
//!
//! Budget: [`SEEDS`] seeds x [`TICKS`] ticks, each seed run twice for the
//! reproducibility comparison and fanned across threads like the map
//! sweeps, with the checklist sampled every [`SAMPLE`] ticks — under half
//! a second of the workspace run's twenty, which leaves room to widen the
//! net rather than trim it. `FUZZ_SEEDS` raises the seed count for a soak
//! run without renumbering the default set.

use chassis::grid::TilePos;
use chassis::rng::Pcg32;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::stats::{BuildingKind, ORDER_QUEUE_CAP};
use oxide_sim::{
    BuildingId, Command, Event, GameResult, PlayerCommand, PlayerId, Scenario, State, Target,
    UnitId, UnitKind,
};

/// Ticks per run.
const TICKS: u64 = 5_000;
/// Seeds in the default sweep.
const SEEDS: u64 = 32;
/// How often the integrity checklist runs. Cheap (entities, not tiles),
/// but not free across a third of a million ticks.
const SAMPLE: u64 = 50;
/// The seed the whole sweep is derived from.
const BASE_SEED: u64 = 0xF022_DEC0DE;
/// A bank no honest match approaches; a `u32` refund that wrapped would
/// land three orders of magnitude past it.
const SCRAP_CEILING: u32 = 1 << 24;

/// Every [`Command`] variant, as a value the generator can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandTag {
    Move,
    Attack,
    AttackMove,
    Harvest,
    Patrol,
    Stop,
    Train,
    Build,
    Cancel,
    Repair,
    Salvage,
    CancelTrain,
    SetRally,
    Surrender,
    RepairUnit,
    Advance,
    FocusFire,
    CancelFound,
    UpgradeBuilding,
}

/// The draw pool. Paired with the exhaustive matches below, the array and
/// the variant list cannot drift apart — the old `next_below(10)` bound
/// against nine arms is exactly how `Repair`, `Salvage`, and
/// `CancelTrain` went unfuzzed.
const COMMAND_TAGS: [CommandTag; 19] = [
    CommandTag::Move,
    CommandTag::Attack,
    CommandTag::AttackMove,
    CommandTag::Harvest,
    CommandTag::Patrol,
    CommandTag::Stop,
    CommandTag::Train,
    CommandTag::Build,
    CommandTag::Cancel,
    CommandTag::Repair,
    CommandTag::Salvage,
    CommandTag::CancelTrain,
    CommandTag::SetRally,
    CommandTag::Surrender,
    CommandTag::RepairUnit,
    CommandTag::Advance,
    CommandTag::FocusFire,
    CommandTag::CancelFound,
    CommandTag::UpgradeBuilding,
];

/// How rarely a drawn [`CommandTag::Surrender`] is kept: one landed
/// surrender from a real seat freezes the rest of that seed's run
/// (the other seat wins on the spot), so an unweighted draw would end
/// every match within a few dozen ticks and starve the other verbs of
/// coverage. Rare-but-present keeps most seeds hot for the whole
/// budget while the sweep still exercises concession and the frozen
/// world behind it.
const SURRENDER_KEEP_ODDS: u32 = 2_048;

/// Contiguous indices, one arm per tag — the other half of the forcing
/// chain: a tag [`COMMAND_TAGS`] forgot fails the coverage test instead of
/// quietly never being drawn.
fn tag_index(tag: CommandTag) -> usize {
    match tag {
        CommandTag::Move => 0,
        CommandTag::Attack => 1,
        CommandTag::AttackMove => 2,
        CommandTag::Harvest => 3,
        CommandTag::Patrol => 4,
        CommandTag::Stop => 5,
        CommandTag::Train => 6,
        CommandTag::Build => 7,
        CommandTag::Cancel => 8,
        CommandTag::Repair => 9,
        CommandTag::Salvage => 10,
        CommandTag::CancelTrain => 11,
        CommandTag::SetRally => 12,
        CommandTag::Surrender => 13,
        CommandTag::RepairUnit => 14,
        CommandTag::Advance => 15,
        CommandTag::FocusFire => 16,
        CommandTag::CancelFound => 17,
        CommandTag::UpgradeBuilding => 18,
    }
}

/// One arm per [`Command`] variant: adding a verb breaks the build here.
fn tag_of(command: &Command) -> CommandTag {
    match command {
        Command::Move { .. } => CommandTag::Move,
        Command::Attack { .. } => CommandTag::Attack,
        Command::AttackMove { .. } => CommandTag::AttackMove,
        Command::Harvest { .. } => CommandTag::Harvest,
        Command::Patrol { .. } => CommandTag::Patrol,
        Command::Stop { .. } => CommandTag::Stop,
        Command::Train { .. } => CommandTag::Train,
        Command::Build { .. } => CommandTag::Build,
        Command::Cancel { .. } => CommandTag::Cancel,
        Command::Repair { .. } => CommandTag::Repair,
        Command::Salvage { .. } => CommandTag::Salvage,
        Command::CancelTrain { .. } => CommandTag::CancelTrain,
        Command::SetRally { .. } => CommandTag::SetRally,
        Command::Surrender => CommandTag::Surrender,
        Command::RepairUnit { .. } => CommandTag::RepairUnit,
        Command::Advance { .. } => CommandTag::Advance,
        Command::FocusFire { .. } => CommandTag::FocusFire,
        Command::UpgradeBuilding { .. } => CommandTag::UpgradeBuilding,
        Command::CancelFound { .. } => CommandTag::CancelFound,
    }
}

/// The whole roster, cross-faction kinds included — `apply_train` owes
/// every one of them a verdict. Exhaustive by the same rule as the verbs.
const UNIT_KINDS: [UnitKind; 11] = [
    UnitKind::Harvester,
    UnitKind::Sentinel,
    UnitKind::Scuttler,
    UnitKind::Lancer,
    UnitKind::Bombard,
    UnitKind::Flakhound,
    UnitKind::Stinger,
    UnitKind::Buzzard,
    UnitKind::Darter,
    UnitKind::Talon,
    UnitKind::Wisp,
];

fn unit_kind_index(kind: UnitKind) -> usize {
    match kind {
        UnitKind::Harvester => 0,
        UnitKind::Sentinel => 1,
        UnitKind::Scuttler => 2,
        UnitKind::Lancer => 3,
        UnitKind::Bombard => 4,
        UnitKind::Flakhound => 5,
        UnitKind::Stinger => 6,
        UnitKind::Buzzard => 7,
        UnitKind::Darter => 8,
        UnitKind::Talon => 9,
        UnitKind::Wisp => 10,
    }
}

/// Every building kind, frame-bound and tech-gated ones included —
/// hostile input must aim at all of them.
const BUILDING_KINDS: [BuildingKind; 11] = [
    BuildingKind::Foundry,
    BuildingKind::Turret,
    BuildingKind::Fabricator,
    BuildingKind::FlakTurret,
    BuildingKind::Bastion,
    BuildingKind::Array,
    BuildingKind::Reclaimer,
    BuildingKind::RepairBay,
    BuildingKind::Extractor,
    BuildingKind::Airworks,
    BuildingKind::Crucible,
];

fn building_kind_index(kind: BuildingKind) -> usize {
    match kind {
        BuildingKind::Foundry => 0,
        BuildingKind::Turret => 1,
        BuildingKind::Fabricator => 2,
        BuildingKind::FlakTurret => 3,
        BuildingKind::Bastion => 4,
        BuildingKind::Array => 5,
        BuildingKind::Reclaimer => 6,
        BuildingKind::RepairBay => 7,
        BuildingKind::Extractor => 8,
        BuildingKind::Airworks => 9,
        BuildingKind::Crucible => 10,
    }
}

/// A coordinate that is usually plausible and occasionally adversarial.
fn coord(rng: &mut Pcg32, edge: i32) -> i32 {
    match rng.next_below(10) {
        0 => i32::MAX,
        1 => i32::MIN,
        2 => -(rng.next_below(100) as i32),
        3 => edge + rng.next_below(100) as i32,
        _ => rng.next_below(edge as u32) as i32,
    }
}

fn tile(rng: &mut Pcg32, state: &State) -> TilePos {
    TilePos::new(
        coord(rng, state.map().width()),
        coord(rng, state.map().height()),
    )
}

/// A tile inside the map. Adversarial coordinates are the point of
/// [`tile`], but a route is only as long as its worst leg — one wild
/// waypoint refuses the whole circuit — so the deep-program cases need a
/// generator that stays on the board.
fn plausible_tile(rng: &mut Pcg32, state: &State) -> TilePos {
    TilePos::new(
        rng.next_below(state.map().width() as u32) as i32,
        rng.next_below(state.map().height() as u32) as i32,
    )
}

/// A patrol circuit: lengths straddle [`ORDER_QUEUE_CAP`] (a route the
/// queue cannot hold is a legal thing to ask for), and a third of them
/// stay on the board so the long ones actually survive validation.
fn waypoints(rng: &mut Pcg32, state: &State) -> Vec<TilePos> {
    let plausible = rng.next_below(3) == 0;
    (0..rng.next_below(ORDER_QUEUE_CAP as u32 + 2))
        .map(|_| {
            if plausible {
                plausible_tile(rng, state)
            } else {
                tile(rng, state)
            }
        })
        .collect()
}

/// A footprint anchor: usually anywhere (most of them are refused, which
/// is the point), but a third land beside a live machine, because a site
/// nobody can reach never exercises the build ramp at all.
fn anchor(rng: &mut Pcg32, state: &State) -> TilePos {
    let live = state.units();
    if live.is_empty() || rng.next_below(3) != 0 {
        return tile(rng, state);
    }
    let near = live[rng.next_below(live.len() as u32) as usize].tile();
    near.offset(rng.next_below(5) as i32 - 2, rng.next_below(5) as i32 - 2)
}

/// One unit id: usually a live one, so the handler bodies are reached
/// rather than bounced off the lookups; sometimes a neighbour of the live
/// range; sometimes pure garbage.
fn unit_id(rng: &mut Pcg32, state: &State) -> UnitId {
    let live = state.units();
    match rng.next_below(8) {
        0 => UnitId(rng.next_u32()),
        1 => UnitId(rng.next_below(64)),
        _ if live.is_empty() => UnitId(rng.next_below(64)),
        // Half the picks come from the head of the roster. Spread evenly
        // across a growing army every append finds an idle machine and
        // no order program ever deepens; concentrated, the queue caps.
        _ => {
            let span = if rng.next_below(2) == 0 {
                live.len().min(4)
            } else {
                live.len()
            };
            live[rng.next_below(span as u32) as usize].id
        }
    }
}

fn building_id(rng: &mut Pcg32, state: &State) -> BuildingId {
    let live = state.buildings();
    match rng.next_below(8) {
        0 => BuildingId(rng.next_u32()),
        1 => BuildingId(rng.next_below(16)),
        _ if live.is_empty() => BuildingId(rng.next_below(16)),
        _ => live[rng.next_below(live.len() as u32) as usize].id,
    }
}

/// A unit list, deliberately a multiset: a repeated id must buy nothing,
/// and this is where that rule meets garbage.
fn units(rng: &mut Pcg32, state: &State) -> Vec<UnitId> {
    let mut ids: Vec<UnitId> = Vec::new();
    for _ in 0..rng.next_below(6) {
        if !ids.is_empty() && rng.next_below(3) == 0 {
            let repeat = ids[rng.next_below(ids.len() as u32) as usize];
            ids.push(repeat);
        } else {
            ids.push(unit_id(rng, state));
        }
    }
    ids
}

/// A building multiset for the same canonicalization and atomic-validation
/// pressure as unit-bearing commands.
fn buildings(rng: &mut Pcg32, state: &State) -> Vec<BuildingId> {
    let mut ids = Vec::new();
    for _ in 0..rng.next_below(6) {
        if !ids.is_empty() && rng.next_below(3) == 0 {
            ids.push(ids[rng.next_below(ids.len() as u32) as usize]);
        } else {
            ids.push(building_id(rng, state));
        }
    }
    ids
}

fn target(rng: &mut Pcg32, state: &State) -> Target {
    if rng.next_below(2) == 0 {
        Target::Unit(unit_id(rng, state))
    } else {
        Target::Building(building_id(rng, state))
    }
}

/// Appending is the interesting half — it is the only way a duplicate id
/// or a long program can reach [`ORDER_QUEUE_CAP`] — so the coin is
/// loaded toward it.
fn queue(rng: &mut Pcg32) -> bool {
    rng.next_below(4) != 0
}

fn generate(tag: CommandTag, rng: &mut Pcg32, state: &State) -> Command {
    match tag {
        CommandTag::Move => Command::Move {
            units: units(rng, state),
            goal: tile(rng, state),
            queue: queue(rng),
        },
        CommandTag::Attack => Command::Attack {
            units: units(rng, state),
            target: target(rng, state),
            queue: queue(rng),
        },
        CommandTag::AttackMove => Command::AttackMove {
            units: units(rng, state),
            goal: tile(rng, state),
            queue: queue(rng),
        },
        CommandTag::Advance => Command::Advance {
            units: units(rng, state),
            goal: tile(rng, state),
            queue: queue(rng),
        },
        CommandTag::Harvest => Command::Harvest {
            units: units(rng, state),
            node: tile(rng, state),
            queue: queue(rng),
        },
        CommandTag::Patrol => Command::Patrol {
            units: units(rng, state),
            waypoints: waypoints(rng, state),
        },
        CommandTag::Stop => Command::Stop {
            units: units(rng, state),
        },
        CommandTag::Train => Command::Train {
            building: building_id(rng, state),
            kind: UNIT_KINDS[rng.next_below(UNIT_KINDS.len() as u32) as usize],
        },
        CommandTag::Build => Command::Build {
            units: units(rng, state),
            kind: BUILDING_KINDS[rng.next_below(BUILDING_KINDS.len() as u32) as usize],
            anchor: anchor(rng, state),
            queue: queue(rng),
            defer: false,
        },
        CommandTag::Cancel => Command::Cancel {
            building: building_id(rng, state),
        },
        CommandTag::Repair => Command::Repair {
            units: units(rng, state),
            building: building_id(rng, state),
            queue: queue(rng),
        },
        CommandTag::Salvage => Command::Salvage {
            units: units(rng, state),
            building: building_id(rng, state),
            queue: queue(rng),
        },
        CommandTag::CancelTrain => Command::CancelTrain {
            building: building_id(rng, state),
            index: rng.next_below(u32::from(u8::MAX) + 1) as u8,
        },
        CommandTag::SetRally => Command::SetRally {
            building: building_id(rng, state),
            rally: (rng.next_below(3) != 0).then(|| tile(rng, state)),
        },
        CommandTag::Surrender => Command::Surrender,
        CommandTag::RepairUnit => Command::RepairUnit {
            units: units(rng, state),
            target: unit_id(rng, state),
            queue: queue(rng),
        },
        CommandTag::FocusFire => Command::FocusFire {
            buildings: buildings(rng, state),
            target: target(rng, state),
        },
        CommandTag::CancelFound => Command::CancelFound {
            kind: BUILDING_KINDS[rng.next_below(BUILDING_KINDS.len() as u32) as usize],
            anchor: anchor(rng, state),
        },
        CommandTag::UpgradeBuilding => Command::UpgradeBuilding {
            units: units(rng, state),
            building: building_id(rng, state),
            queue: queue(rng),
        },
    }
}

/// What a run actually managed to touch. Garbage that never gets past
/// validation proves nothing, so the sweep insists on seeing the deep
/// shapes — saturated queues, claimed sites, stripped buildings — and a
/// reproducible run must reproduce these too, not just its final hash.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Reach {
    /// Commands offered to the sim.
    sent: u64,
    /// Commands it refused.
    rejected: u64,
    /// Verbs the generator actually drew, indexed like [`COMMAND_TAGS`].
    drawn: [u64; COMMAND_TAGS.len()],
    /// Deepest order program any unit carried.
    order_queue: usize,
    /// Deepest production queue any building carried.
    train_queue: usize,
    trained: u64,
    completed: u64,
    salvaged: u64,
    cancelled: u64,
    cancelled_found: u64,
}

impl Reach {
    fn absorb(&mut self, other: &Reach) {
        self.sent += other.sent;
        self.rejected += other.rejected;
        for (a, b) in self.drawn.iter_mut().zip(other.drawn) {
            *a += b;
        }
        self.order_queue = self.order_queue.max(other.order_queue);
        self.train_queue = self.train_queue.max(other.train_queue);
        self.trained += other.trained;
        self.completed += other.completed;
        self.salvaged += other.salvaged;
        self.cancelled += other.cancelled;
        self.cancelled_found += other.cancelled_found;
    }
}

/// The state hash a run ends on, plus what it touched getting there.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    hash: u64,
    reach: Reach,
}

/// Seeds derive from one constant through independent PCG streams, so a
/// soak run's extra seeds never renumber the default set.
fn seed_at(index: u64) -> u64 {
    Pcg32::new(BASE_SEED, index).next_u64()
}

/// Skirmish, opened up. Two edits, both because garbage never runs an
/// economy: the banks start deep, or the production and construction
/// handlers reject on funds within a few hundred ticks; and each seat
/// starts with a built Turret and Fabricator, or `Repair` and `Salvage`
/// never find a legal patient and their whole bodies — the billing
/// meter, the drain ledger, `purge_opposing_verb` — go unfuzzed.
fn arena() -> State {
    let standing =
        |player: u8, kind: BuildingKind, x: i32, y: i32| BuildingSpec { player, kind, x, y };
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.scrap = 100_000;
    }
    scenario.buildings = vec![
        standing(0, BuildingKind::Turret, 12, 3),
        standing(0, BuildingKind::Fabricator, 8, 12),
        standing(1, BuildingKind::Turret, 27, 20),
        standing(1, BuildingKind::Fabricator, 30, 10),
    ];
    scenario.build().expect("the fuzz arena builds")
}

fn found_claims(state: &State, player: PlayerId, kind: BuildingKind, anchor: TilePos) -> usize {
    let matches = |order: &oxide_sim::Order| {
        matches!(order, oxide_sim::Order::Found { kind: found_kind, anchor: found_anchor }
            if *found_kind == kind && *found_anchor == anchor)
    };
    state
        .units()
        .iter()
        .filter(|unit| unit.player == player)
        .map(|unit| {
            usize::from(matches(&unit.order))
                + unit.queue.iter().filter(|order| matches(order)).count()
        })
        .sum()
}

fn exercise_cancel_found_reach(state: &mut State) {
    let player = PlayerId(0);
    let builder = state
        .units()
        .iter()
        .find(|unit| unit.player == player && unit.kind == UnitKind::Harvester)
        .expect("fuzz arena has a Ferrous Harvester")
        .id;
    let kind = BuildingKind::Turret;
    let anchor = TilePos::new(12, 5);
    let build = state.tick(&[PlayerCommand {
        player,
        command: Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer: true,
        },
    }]);
    assert!(
        !build
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. })),
        "the fuzz reach fixture must establish a deferred claim"
    );
    assert_eq!(found_claims(state, player, kind, anchor), 1);

    let cancel = state.tick(&[PlayerCommand {
        player,
        command: Command::CancelFound { kind, anchor },
    }]);
    assert!(
        !cancel
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. })),
        "the fuzz reach fixture must land CancelFound"
    );
    assert_eq!(found_claims(state, player, kind, anchor), 0);
}

fn fuzz_run(seed: u64) -> Run {
    let mut state = arena();
    exercise_cancel_found_reach(&mut state);
    let mut rng = Pcg32::new(seed, 0xF022);
    let mut reach = Reach {
        cancelled_found: 1,
        ..Reach::default()
    };
    let mut decided: Option<GameResult> = None;
    let first_tick = state.current_tick();

    for offset in 0..TICKS {
        let tick = first_tick + offset;
        assert_eq!(
            state.current_tick(),
            tick,
            "seed {seed:#x}: the tick counter skipped"
        );
        let commands: Vec<PlayerCommand> = (0..rng.next_below(4))
            .map(|_| {
                let drawn = loop {
                    let d = rng.next_below(COMMAND_TAGS.len() as u32) as usize;
                    if COMMAND_TAGS[d] != CommandTag::Surrender
                        || rng.next_below(SURRENDER_KEEP_ODDS) == 0
                    {
                        break d;
                    }
                };
                reach.drawn[drawn] += 1;
                PlayerCommand {
                    // Players 0-3 on a two-player map: half the issuers
                    // don't exist.
                    player: PlayerId(rng.next_below(4) as u8),
                    command: generate(COMMAND_TAGS[drawn], &mut rng, &state),
                }
            })
            .collect();
        reach.sent += commands.len() as u64;

        let report = state.tick(&commands);
        for event in &report.events {
            match event {
                Event::CommandRejected { .. } => reach.rejected += 1,
                Event::UnitTrained { .. } => reach.trained += 1,
                Event::BuildingCompleted { .. } => reach.completed += 1,
                Event::BuildingSalvaged { .. } => reach.salvaged += 1,
                Event::BuildCancelled { .. } => reach.cancelled += 1,
                _ => {}
            }
        }
        reach.order_queue = reach.order_queue.max(
            state
                .units()
                .iter()
                .map(|u| u.queue.len())
                .max()
                .unwrap_or(0),
        );
        reach.train_queue = reach.train_queue.max(
            state
                .buildings()
                .iter()
                .map(|b| b.queue.len())
                .max()
                .unwrap_or(0),
        );

        // A decided match never un-decides, and no bank ever wraps.
        match (decided, state.result()) {
            (Some(was), now) => assert_eq!(
                Some(was),
                now,
                "seed {seed:#x}: tick {tick} rewrote a decided match"
            ),
            (None, now) => decided = now,
        }
        if let Some(rich) = state.players().iter().find(|p| p.scrap > SCRAP_CEILING) {
            panic!(
                "seed {seed:#x}: tick {tick} banked {} scrap — a refund wrapped",
                rich.scrap
            );
        }

        if tick.is_multiple_of(SAMPLE) {
            state.validate_invariants().unwrap_or_else(|err| {
                panic!("seed {seed:#x}: tick {tick} is a state the validator refuses: {err}")
            });
        }
    }
    state.validate_invariants().unwrap_or_else(|err| {
        panic!("seed {seed:#x}: the final state is one the validator refuses: {err}")
    });

    Run {
        hash: state.hash(),
        reach,
    }
}

/// How many seeds this run sweeps. `FUZZ_SEEDS` raises it for a soak.
fn seed_count() -> u64 {
    std::env::var("FUZZ_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(SEEDS)
}

#[test]
fn seeded_garbage_never_panics_and_reproduces() {
    let seeds: Vec<u64> = (0..seed_count()).map(seed_at).collect();
    let runs: Vec<Run> = std::thread::scope(|scope| {
        let handles: Vec<_> = seeds
            .iter()
            .map(|&seed| {
                scope.spawn(move || {
                    let first = fuzz_run(seed);
                    let second = fuzz_run(seed);
                    assert_eq!(
                        first, second,
                        "seed {seed:#x}: the same garbage took two different paths"
                    );
                    first
                })
            })
            .collect();
        seeds
            .iter()
            .zip(handles)
            .map(|(seed, handle)| {
                handle
                    .join()
                    .unwrap_or_else(|_| panic!("seed {seed:#x} broke the sim — rerun that seed"))
            })
            .collect()
    });

    // Distinct seeds must be distinct streams, or the sweep is one run
    // measured many times over.
    let mut hashes: Vec<u64> = runs.iter().map(|r| r.hash).collect();
    hashes.sort_unstable();
    hashes.dedup();
    assert_eq!(hashes.len(), runs.len(), "two seeds produced one world");

    let mut reach = Reach::default();
    for run in &runs {
        reach.absorb(&run.reach);
    }

    // The premise. Garbage that never lands proves only that validation
    // works, and a fuzzer whose reach quietly narrows is the failure this
    // file exists to prevent — it is how three verbs stayed unfuzzed
    // through two releases. Every row below is a shape the old generator
    // never reached; a sim change that makes one unreachable owes this
    // list an edit and an explanation.
    assert!(
        reach.rejected < reach.sent,
        "the sweep landed nothing ({} of {} refused)",
        reach.rejected,
        reach.sent
    );
    assert!(
        reach.drawn.iter().all(|n| *n > 0),
        "a verb was never drawn: {:?}",
        reach.drawn
    );
    assert_eq!(
        reach.order_queue, ORDER_QUEUE_CAP,
        "the sweep must drive an order program to its cap"
    );
    assert!(
        reach.train_queue > 1,
        "the sweep must stack a production queue ({} deep)",
        reach.train_queue
    );
    assert!(reach.trained > 0, "the sweep must train units");
    // Sites are claimed and refunded here; finishing one needs a builder
    // nobody re-commands for a hundred ticks, which is the behavior
    // suites' job, not garbage's.
    assert!(reach.cancelled > 0, "the sweep must cancel a site");
    assert!(
        reach.cancelled_found > 0,
        "the sweep must accept a deferred-site cancellation"
    );
    assert!(reach.salvaged > 0, "the sweep must strip a building");
}

/// The forcing chain, stated as a test: the draw pool is the whole tag
/// list, and every tag generates the verb it names. A new [`Command`]
/// variant stops `tag_of` compiling; its tag stops `tag_index` compiling;
/// a tag missing from [`COMMAND_TAGS`] fails here.
#[test]
fn every_command_variant_is_generated() {
    let mut covered: Vec<usize> = COMMAND_TAGS.into_iter().map(tag_index).collect();
    covered.sort_unstable();
    assert_eq!(
        covered,
        (0..COMMAND_TAGS.len()).collect::<Vec<_>>(),
        "the draw pool is not the tag list"
    );

    let state = arena();
    let mut rng = Pcg32::new(BASE_SEED, 7);
    for tag in COMMAND_TAGS {
        assert_eq!(
            tag_of(&generate(tag, &mut rng, &state)),
            tag,
            "{tag:?} generates a different verb"
        );
    }
}

/// The roster lists carry the same obligation as the verb list: a new
/// unit or building kind stops this file compiling until the fuzzer
/// offers it.
#[test]
fn every_kind_is_offered() {
    let mut units: Vec<usize> = UNIT_KINDS.into_iter().map(unit_kind_index).collect();
    units.sort_unstable();
    assert_eq!(units, (0..UNIT_KINDS.len()).collect::<Vec<_>>());
    let mut buildings: Vec<usize> = BUILDING_KINDS
        .into_iter()
        .map(building_kind_index)
        .collect();
    buildings.sort_unstable();
    assert_eq!(buildings, (0..BUILDING_KINDS.len()).collect::<Vec<_>>());
}
