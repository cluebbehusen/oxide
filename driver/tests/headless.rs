//! Driver-level headless checks: the runner's record/replay loop is the
//! same one the shell uses, so this proves the whole recording pipeline
//! without a window.

use oxide_driver::{pool, runner};
use oxide_sim::Scenario;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One seat's proof of life over a run, tallied from the tick reports.
#[derive(Clone, Default)]
struct SeatActivity {
    /// Units the seat's Foundries finished.
    produced: u32,
    /// Harvester deliveries.
    deposits: u32,
    /// Scrap those deliveries banked.
    banked: u32,
    /// Buildings the seat completed.
    built: u32,
    /// Shots the seat's units and turrets sent — the combat signal.
    attacks: u32,
    /// Last tick any of the above moved.
    last_progress: Option<u64>,
}

/// A finished liveness run: what every seat did, and how the match ended.
struct MatchActivity {
    seats: Vec<SeatActivity>,
    /// Whether the seat still owned a Foundry when the run stopped. A
    /// seat that lost its last one is eliminated, and its remaining
    /// machines play on masterless by design — it owes no activity.
    holds_foundry: Vec<bool>,
    /// Tick the match was decided on, if it was.
    decided_at: Option<u64>,
    /// The tick activity was still possible on: `State::tick` is a no-op
    /// once the result latches, so a decided match's silence afterwards
    /// is the sim working, not a stall.
    horizon: u64,
}

impl MatchActivity {
    /// Ticks between a seat's last progress and the horizon. `None` when
    /// the seat never did anything at all.
    fn stale_for(&self, seat: usize) -> Option<u64> {
        self.seats[seat]
            .last_progress
            .map(|t| self.horizon.saturating_sub(t))
    }

    /// Ticks the whole match went without any seat doing anything — the
    /// freeze detector. Deliberately match-wide: a seat crushed down to
    /// a bare Foundry legitimately goes quiet while the game plays on,
    /// but a map where *every* seat has gone quiet has stalled.
    fn match_stale(&self) -> u64 {
        self.seats
            .iter()
            .filter_map(|s| s.last_progress)
            .max()
            .map_or(self.horizon, |t| self.horizon.saturating_sub(t))
    }
}

/// Plays `scenario` for `ticks` bot-vs-bot ticks, tallying per-seat
/// activity out of the tick reports.
///
/// Events name a shooter by id and the shooter may be dead by the time
/// the report is read, so ownership is tracked in a ledger seeded from
/// the opening state: units enter it at spawn or on `UnitTrained`,
/// buildings at spawn or on `BuildingCompleted` (nothing fires before
/// it completes).
fn play_and_tally(scenario: &Scenario, ticks: u64) -> anyhow::Result<MatchActivity> {
    use oxide_sim::event::Event;

    let mut state = scenario.build()?;
    let mut bots = oxide_sim::bot::seat_bots(scenario);
    let mut seats = vec![SeatActivity::default(); scenario.players.len()];
    let mut unit_owner: BTreeMap<u32, usize> = state
        .units()
        .iter()
        .map(|u| (u.id.0, usize::from(u.player.0)))
        .collect();
    let mut building_owner: BTreeMap<u32, usize> = state
        .buildings()
        .iter()
        .map(|b| (b.id.0, usize::from(b.player.0)))
        .collect();
    let mut decided_at = None;

    for _ in 0..ticks {
        let report = runner::step(&mut state, &mut bots, None);
        for event in &report.events {
            let seat = match event {
                Event::UnitTrained { unit, player, .. } => {
                    let seat = usize::from(player.0);
                    unit_owner.insert(unit.0, seat);
                    seats[seat].produced += 1;
                    Some(seat)
                }
                Event::ScrapDeposited { player, amount } => {
                    let seat = usize::from(player.0);
                    seats[seat].deposits += 1;
                    seats[seat].banked += amount;
                    Some(seat)
                }
                Event::BuildingCompleted {
                    building, player, ..
                } => {
                    let seat = usize::from(player.0);
                    building_owner.insert(building.0, seat);
                    seats[seat].built += 1;
                    Some(seat)
                }
                Event::AttackHit { attacker, .. } => unit_owner.get(&attacker.0).copied(),
                Event::TurretFired { turret, .. } => building_owner.get(&turret.0).copied(),
                Event::ShellLaunched { player, .. } => Some(usize::from(player.0)),
                Event::GameOver { .. } => {
                    decided_at = Some(report.tick);
                    None
                }
                _ => None,
            };
            if let Some(seat) = seat {
                if matches!(
                    event,
                    Event::AttackHit { .. }
                        | Event::TurretFired { .. }
                        | Event::ShellLaunched { .. }
                ) {
                    seats[seat].attacks += 1;
                }
                seats[seat].last_progress = Some(report.tick);
            }
        }
    }

    let holds_foundry = (0..seats.len())
        .map(|seat| {
            state.buildings().iter().any(|b| {
                usize::from(b.player.0) == seat && b.kind == oxide_sim::BuildingKind::Foundry
            })
        })
        .collect();
    Ok(MatchActivity {
        seats,
        holds_foundry,
        decided_at,
        horizon: decided_at.unwrap_or(ticks),
    })
}

/// The shipped maps, biggest file first: the 4v4s are the sweep's
/// critical path and a last-scheduled Compass Grand would add its whole
/// runtime to the tail.
fn shipped_scenarios() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    assert!(
        paths.len() >= 4,
        "expected the shipped maps, found {}",
        paths.len()
    );
    paths.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
    paths
}

/// A shipped map with every seat flipped to the shipped default
/// opponent. A configless flip would field the team-blind classic bot,
/// which team maps reject.
fn all_bots(path: &std::path::Path) -> Scenario {
    let mut scenario =
        Scenario::load(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    for player in &mut scenario.players {
        player.bot = true;
        player
            .bot_config
            .get_or_insert(oxide_sim::scenario::BotConfig {
                level: oxide_sim::bot::Level::Medium,
                aggression: None,
            });
    }
    scenario
}

fn bot_skirmish() -> Scenario {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
    }
    scenario
}

#[test]
fn recorded_scenario_run_reproduces_from_its_replay() {
    let scenario = bot_skirmish();
    let outcome = runner::run_scenario(&scenario, 900, true, true).unwrap();
    let replay = outcome.replay.unwrap();
    assert_eq!(replay.meta.ticks, Some(900));
    assert!(!replay.commands.is_empty());

    let replayed = runner::run_replay(&replay, None, false).unwrap();
    assert_eq!(replayed.current_tick(), outcome.state.current_tick());
    assert_eq!(replayed.hash(), outcome.state.hash());
}

/// Ticks of bot-vs-bot play every shipped map must survive.
const LIVENESS_TICKS: u64 = 12_000;

/// Longest the whole match may go without a single seat doing anything.
/// Measured worst case across the roster is under 250 ticks, so this is
/// eight times the observed slack and still an order of magnitude under
/// the freeze it exists to catch.
const MATCH_STALE_TICKS: u64 = 2_000;

/// The per-format floors every seat that still holds a Foundry must
/// clear over [`LIVENESS_TICKS`].
struct Floors {
    produced: u32,
    deposits: u32,
    /// Whether the match as a whole must have fired a shot. Measured:
    /// every 2- and 4-seat map fights, and so do Trident Plateau and
    /// Compass Grand — but Causeway Verdict and Gatework Array see zero
    /// combat in 12k ticks, with not one casualty on either. Ten minutes
    /// of game time is march time on the big team maps, so the large
    /// formats prove their life through the economy alone.
    combat: bool,
}

/// Seat count decides the floors: more seats split one map's ground more
/// ways, so each seat's economy is a fraction of a duellist's. Every
/// number sits near a third of the measured minimum — see
/// `liveness_floors_calibration_table` for the reading these came from.
fn floors_for(seats: usize) -> Floors {
    match seats {
        0..=2 => Floors {
            produced: 4,
            deposits: 25,
            combat: true,
        },
        3..=4 => Floors {
            produced: 6,
            deposits: 40,
            combat: true,
        },
        _ => Floors {
            produced: 7,
            deposits: 45,
            combat: false,
        },
    }
}

/// The gate itself, as a decision over a played match: `Err` names the
/// first floor the run missed and reports the whole tally beside it.
fn liveness_verdict(map: &str, activity: &MatchActivity) -> Result<(), String> {
    let seats = activity.seats.len();
    let floors = floors_for(seats);
    let ending = match activity.decided_at {
        Some(tick) => format!("decided at tick {tick}"),
        None => format!("undecided after {} ticks", activity.horizon),
    };
    let attacks: u32 = activity.seats.iter().map(|s| s.attacks).sum();
    if floors.combat && attacks == 0 {
        return Err(format!(
            "{map}: {ending}, and not one shot was fired all match"
        ));
    }
    // The freeze detector the id proxy was reaching for. The horizon is
    // the decision tick when there is one: `State::tick` is a no-op once
    // the result latches, so a decided match's silence afterwards is the
    // sim working, not a stall.
    let stale = activity.match_stale();
    if stale > MATCH_STALE_TICKS {
        return Err(format!(
            "{map}: {ending}, and no seat did anything for the last {stale} ticks before the horizon"
        ));
    }
    for (seat, tally) in activity.seats.iter().enumerate() {
        // An eliminated seat legitimately does nothing: losing the last
        // Foundry rejects its commands and leaves its machines running
        // masterless.
        if !activity.holds_foundry[seat] {
            continue;
        }
        let short = if tally.produced < floors.produced {
            "a seat holding a Foundry must keep building units"
        } else if tally.deposits < floors.deposits {
            "a seat holding a Foundry must keep harvesting"
        } else {
            continue;
        };
        return Err(format!(
            "{map} seat {seat} of {seats}: {ending}; produced {}, deposits {} ({} scrap), \
             built {}, attacks {}, stale {:?} of horizon {} — {short}",
            tally.produced,
            tally.deposits,
            tally.banked,
            tally.built,
            tally.attacks,
            activity.stale_for(seat),
            activity.horizon,
        ));
    }
    Ok(())
}

#[test]
fn every_shipped_scenario_builds_and_plays() {
    // Playable means *alive*, not merely parseable. The old proxy — a
    // surviving unit id past the starting roster — was satisfied at tick
    // zero on every map that spawns 17 or more units, so a total economy
    // freeze on a 4v4 passed. The tick reports carry the real signal, so
    // every seat that still holds a Foundry must account for its own
    // production and deliveries, and the match as a whole must have
    // done something recently.
    //
    // Every map is an independent deterministic sim, so the sweep fans
    // out across the instruments' shared worker pool — 25 maps at 12k
    // ticks each would otherwise dominate the workspace suite's wall
    // clock.
    let paths = shipped_scenarios();
    let played = pool::fan_out(&paths, |path| {
        let scenario = all_bots(path);
        let activity = play_and_tally(&scenario, LIVENESS_TICKS)?;
        Ok((path.clone(), activity))
    })
    .unwrap();

    for (path, activity) in &played {
        if let Err(refusal) = liveness_verdict(&path.display().to_string(), activity) {
            panic!("{refusal}");
        }
    }
}

/// A seat comfortably clearing every floor, for the gate's own tests.
fn busy_seat(last_progress: u64) -> SeatActivity {
    SeatActivity {
        produced: 50,
        deposits: 200,
        banked: 2_000,
        built: 1,
        attacks: 300,
        last_progress: Some(last_progress),
    }
}

#[test]
fn the_liveness_gate_catches_a_frozen_seat() {
    // The failures the id heuristic could not see. A 4v4 where every
    // seat holds its Foundry and nothing happens at all trips the
    // match-wide freeze detector.
    let frozen = MatchActivity {
        seats: vec![SeatActivity::default(); 8],
        holds_foundry: vec![true; 8],
        decided_at: None,
        horizon: LIVENESS_TICKS,
    };
    let refusal = liveness_verdict("frozen", &frozen).unwrap_err();
    assert!(refusal.contains("no seat did anything"), "{refusal}");

    // One idle seat beside seven working ones trips the per-seat economy
    // floor instead — the failure no match-wide number can see, and the
    // one a starting roster of 32 units hid completely.
    let mut one_idle = frozen;
    for seat in &mut one_idle.seats[1..] {
        *seat = busy_seat(LIVENESS_TICKS - 10);
    }
    let refusal = liveness_verdict("one-idle", &one_idle).unwrap_err();
    assert!(refusal.contains("seat 0 of 8"), "{refusal}");
    assert!(refusal.contains("keep building units"), "{refusal}");
}

#[test]
fn the_liveness_gate_asks_nothing_of_an_eliminated_seat() {
    // An eliminated seat's machines play on masterless by design, so a
    // dead seat beside a working one is a finished match, not a stall.
    let decided = MatchActivity {
        seats: vec![SeatActivity::default(), busy_seat(5_000)],
        holds_foundry: vec![false, true],
        decided_at: Some(5_000),
        horizon: 5_000,
    };
    assert!(liveness_verdict("decided", &decided).is_ok());

    // The same numbers with both seats still holding a Foundry is the
    // stall the exemption must not launder.
    let mut stalled = decided;
    stalled.holds_foundry[0] = true;
    let refusal = liveness_verdict("stalled", &stalled).unwrap_err();
    assert!(refusal.contains("seat 0 of 2"), "{refusal}");
}

/// The calibration behind [`floors_for`]. Ignored by default — it plays
/// the same sweep the gate does and prints the table rather than
/// asserting on it. Re-run it (`--ignored --nocapture`) after any change
/// that moves bot behavior, and re-author the floors well under the
/// observed minima.
#[test]
#[ignore = "diagnostic: prints the liveness calibration table"]
fn liveness_floors_calibration_table() {
    let paths = shipped_scenarios();
    let played = pool::fan_out(&paths, |path| {
        let scenario = all_bots(path);
        let activity = play_and_tally(&scenario, LIVENESS_TICKS)?;
        Ok((path.clone(), activity))
    })
    .unwrap();

    println!(
        "{:<22} {:>5} {:>7} {:>4} {:>6} {:>8} {:>7} {:>6} {:>8} {:>7}",
        "map",
        "seats",
        "decided",
        "seat",
        "prod",
        "deposits",
        "banked",
        "built",
        "attacks",
        "stale"
    );
    // Per format: the tightest production and delivery any surviving
    // seat managed, which is what the floors must sit well under.
    let mut worst: BTreeMap<usize, (u32, u32)> = BTreeMap::new();
    let mut worst_match_stale = 0;
    for (path, activity) in &played {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let seats = activity.seats.len();
        let decided = activity
            .decided_at
            .map_or_else(|| "-".to_string(), |t| t.to_string());
        worst_match_stale = worst_match_stale.max(activity.match_stale());
        for (seat, tally) in activity.seats.iter().enumerate() {
            let held = activity.holds_foundry[seat];
            let stale = activity
                .stale_for(seat)
                .map_or_else(|| "never".to_string(), |s| s.to_string());
            println!(
                "{:<22} {:>5} {:>7} {:>4} {:>6} {:>8} {:>7} {:>6} {:>8} {:>7}{}",
                name,
                seats,
                decided,
                seat,
                tally.produced,
                tally.deposits,
                tally.banked,
                tally.built,
                tally.attacks,
                stale,
                if held { "" } else { "  (eliminated)" },
            );
            if held {
                let entry = worst.entry(seats).or_insert((u32::MAX, u32::MAX));
                entry.0 = entry.0.min(tally.produced);
                entry.1 = entry.1.min(tally.deposits);
            }
        }
    }
    println!("\nobserved minima over surviving seats (floors must sit well under these):");
    for (seats, (produced, deposits)) in &worst {
        println!("  {seats} seats: produced {produced}, deposits {deposits}");
    }
    println!("worst match-wide stale across the roster: {worst_match_stale}");
}

#[test]
fn run_without_bots_is_quiet_but_valid() {
    let outcome = runner::run_scenario(&Scenario::skirmish(), 100, false, true).unwrap();
    let replay = outcome.replay.unwrap();
    assert!(replay.commands.is_empty(), "nobody issued commands");
    assert_eq!(outcome.state.current_tick(), 100);
}

#[test]
fn forged_marathon_replays_are_refused() {
    use chassis::replay::Replay;
    use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario};
    let mut replay: Replay<Scenario, PlayerCommand> =
        Replay::new(SIM_VERSION, Scenario::skirmish());
    replay.meta.ticks = Some(u64::MAX - 1);
    let err = runner::run_replay(&replay, None, false).unwrap_err();
    assert!(err.to_string().contains("--allow-long"), "{err}");
}

#[test]
fn load_scenario_resolves_the_skirmish_shorthand() {
    assert_eq!(
        runner::load_scenario("skirmish").unwrap(),
        Scenario::skirmish(),
        "the bare word must resolve to the embedded map, not a file lookup"
    );
}

#[test]
fn load_scenario_names_the_path_when_it_cannot_be_read() {
    let err = runner::load_scenario("definitely/not/a/real/scenario.json").unwrap_err();
    assert!(
        err.to_string()
            .contains("definitely/not/a/real/scenario.json"),
        "the error should name the path it failed on: {err}"
    );
}

#[test]
fn run_scenario_surfaces_a_build_failure_with_context() {
    use oxide_sim::Faction;
    use oxide_sim::scenario::PlayerSpec;
    // Parses fine, but the extra seat has no Foundry anchor on the map, so
    // the build fails; the runner must wrap that, not swallow it.
    let mut scenario = Scenario::skirmish();
    scenario.players.push(PlayerSpec {
        name: "anchorless".into(),
        faction: Faction::Ferrous,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    });
    let err = match runner::run_scenario(&scenario, 10, false, false) {
        Ok(_) => panic!("an anchorless seat must fail the build"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("building scenario"), "{err}");
}

#[test]
fn an_unfought_match_reports_no_result() {
    let outcome = runner::run_scenario(&Scenario::skirmish(), 300, false, false).unwrap();
    assert!(
        outcome.state.result().is_none(),
        "nobody fought, so the match stays undecided"
    );
}

#[test]
fn a_decided_match_latches_its_result_and_keeps_ticking() {
    use oxide_sim::scenario::{PlayerSpec, UnitSpec};
    use oxide_sim::{Faction, GameResult, UnitKind};

    // A firing squad: seat 0's Sentinels sit inside aggro range of seat 1's
    // lone Foundry and grind it down with no orders at all; seat 1 has no
    // army to answer. The win lands well before the tick budget, which lets
    // us prove run_scenario keeps counting past the victory (frozen ticks
    // included) instead of returning early.
    let ground = ".".repeat(16);
    let mut anchored: Vec<char> = ground.chars().collect();
    anchored[1] = '1';
    anchored[11] = '2';
    let map = vec![
        ground.clone(),
        ground.clone(),
        anchored.into_iter().collect(),
        ground.clone(),
        ground.clone(),
        ground,
    ];
    let mut units = Vec::new();
    for x in [8, 9] {
        for y in [1, 2, 3, 4] {
            units.push(UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x,
                y,
            });
        }
    }
    let scenario = Scenario {
        name: "firing-squad".into(),
        seed: 7,
        map,
        players: vec![
            PlayerSpec {
                name: "attacker".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 100,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "victim".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 100,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    };

    let budget = 3_000;
    let outcome = runner::run_scenario(&scenario, budget, false, false).unwrap();
    assert_eq!(
        outcome.state.result(),
        Some(GameResult::Victory { team: 0 }),
        "seat 1's only Foundry should be rubble"
    );
    assert_eq!(
        outcome.state.current_tick(),
        budget,
        "a mid-run victory must not cut the requested tick count short"
    );
}

#[test]
fn a_version_mismatched_replay_is_refused_by_default() {
    use chassis::replay::Replay;
    use oxide_sim::PlayerCommand;
    let replay: Replay<Scenario, PlayerCommand> =
        Replay::new("0.0.0-not-this-sim", Scenario::skirmish());
    let err = runner::run_replay(&replay, None, false).unwrap_err();
    assert!(err.to_string().contains("recorded on sim"), "{err}");
}

#[test]
fn a_version_mismatched_replay_plays_when_the_mismatch_is_allowed() {
    use chassis::replay::Replay;
    use oxide_sim::PlayerCommand;
    let replay: Replay<Scenario, PlayerCommand> =
        Replay::new("0.0.0-not-this-sim", Scenario::skirmish());
    let state = runner::run_replay(&replay, None, true).unwrap();
    assert_eq!(
        state.current_tick(),
        0,
        "an empty replay loads to its opening state even across a version gap"
    );
}

#[test]
fn overriding_the_tick_count_below_the_commands_is_rejected() {
    use chassis::replay::Replay;
    use oxide_sim::{Command, PlayerCommand, PlayerId, SIM_VERSION, UnitId};
    let mut replay: Replay<Scenario, PlayerCommand> =
        Replay::new(SIM_VERSION, Scenario::skirmish());
    replay.record(
        100,
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Stop {
                units: vec![UnitId(0)],
            },
        },
    );
    replay.meta.ticks = Some(200);
    // The override stops playback at 50, stranding the tick-100 command; a
    // silent drop would desync a "resumed" session, so it must be an error.
    let err = runner::run_replay(&replay, Some(50), false).unwrap_err();
    assert!(err.to_string().contains("unconsumed"), "{err}");
}

#[test]
fn every_shipped_scenario_names_its_seats_uniquely() {
    // The banner, panel, and stats all address seats by name, and the
    // shell's launch refuses collisions — a duplicate authored name
    // crashed match setup in 0.11 (Trident and Compass shipped two
    // "West Ferrous" seats each).
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let scenario =
            Scenario::load(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let mut names: Vec<&str> = scenario.players.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "{}: seat names collide",
            path.display()
        );
    }
}
