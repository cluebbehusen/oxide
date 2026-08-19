//! Token-efficient narrative digest of a replay: an event timeline, per-seat
//! digests at intervals, and coarse ASCII minimaps.
//!
//! This is the review instrument for reading a bot game without screenshots.
//! It re-executes the replay once through the same execution path as the rest
//! of the driver and never touches simulation behavior. Determinism is by
//! construction: every internal computation is integer math over the sim's
//! own deterministic event stream, so the same replay and options yield the
//! same report, byte for byte.
//!
//! Known v1 caveats: boarded cargo leaves [`oxide_sim::State::units`], so
//! army value dips while transports fly; building loss values use tier-0
//! construction cost (the destruction event names no tier); validation is
//! strict to the current [`oxide_sim::SIM_VERSION`] — a cross-version replay
//! would narrate a divergent ghost game, so it is refused instead.

use crate::runner::{GameReplay, MAX_REPLAY_TICKS};
use anyhow::{Context, Result};
use chassis::grid::TilePos;
use oxide_sim::{
    BuildingId, BuildingKind, Event, Faction, GameResult, Order, PlayerId, SIM_VERSION, State,
    TICKS_PER_SECOND, Target, UnitId, UnitKind,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Version of the serialized [`SummaryReport`] contract.
pub const REPLAY_SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Space gate: a loss joins an active battle when within this many tiles of
/// its running centroid. Above the longest direct-fire range in the game
/// (the Bombard's 9.5 tiles), so both ends of an exchange cluster together;
/// well below audited spawn spacing, so two bases never merge.
const BATTLE_RADIUS_TILES: i64 = 12;

/// Time gate: a battle closes after this many ticks (30 seconds) without a
/// nearby loss. Reinforcement waves re-engage well inside the window;
/// assaults on one base minutes apart read as separate battles.
const BATTLE_QUIET_TICKS: u64 = 600;

/// Battles totaling less lost value than this fold into the digest window's
/// skirmish counter instead of a timeline line (~two line units).
const BATTLE_VALUE_FLOOR: u64 = 200;

/// Minimap cell army value at or above which the seat letter capitalizes.
const MINIMAP_MASS_THRESHOLD: u64 = 300;

/// Kinds loud enough for the timeline: the tech gates that change what a
/// seat can threaten. Everything else lands in the closing reach table.
const LOUD_BUILDINGS: &[BuildingKind] = &[BuildingKind::Airworks, BuildingKind::Crucible];
/// Tier-3 breakthroughs and the transport — see [`LOUD_BUILDINGS`].
const LOUD_UNITS: &[UnitKind] = &[
    UnitKind::Breaker,
    UnitKind::Avalanche,
    UnitKind::Condor,
    UnitKind::Moth,
    UnitKind::Skyhook,
];

/// Options controlling one summary pass.
#[derive(Debug, Clone, Copy)]
pub struct SummaryOptions {
    /// Stop after this state tick (tick `N` = before commands stamped `N`
    /// execute); clamped to the replay's recorded duration.
    pub until: Option<u64>,
    /// Digest cadence in ticks; `None` picks duration/16 clamped to
    /// `[2000, 10000]`.
    pub every: Option<u64>,
    /// Which digests carry a minimap.
    pub minimaps: MinimapMode,
}

/// Minimap emission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinimapMode {
    /// A minimap on every digest.
    All,
    /// A minimap on every fourth digest and the final one.
    Sparse,
    /// No minimaps.
    None,
}

/// The whole digest: header facts, timeline, digests, closing summary.
#[derive(Debug, Serialize)]
pub struct SummaryReport {
    /// Serialization contract version.
    pub schema_version: u32,
    /// Reproducible starting-match facts.
    pub scenario: ScenarioLine,
    /// Seats in player-id order.
    pub seats: Vec<SeatLine>,
    /// Narrative moments sorted by tick.
    pub timeline: Vec<TimelineEntry>,
    /// Per-seat state at each digest boundary.
    pub digests: Vec<Digest>,
    /// Every first-of-kind moment per seat, sorted by tick.
    pub tech_reach: Vec<SeatTechReach>,
    /// How the recorded match ended.
    pub outcome: OutcomeLine,
}

/// Scenario facts useful when orienting a summary.
#[derive(Debug, Serialize)]
pub struct ScenarioLine {
    /// Scenario display name.
    pub name: String,
    /// Deterministic scenario seed.
    pub seed: u64,
    /// Map width in tiles.
    pub map_width: i32,
    /// Map height in tiles.
    pub map_height: i32,
    /// Ticks the summary executed (after any `--until` clamp).
    pub effective_ticks: u64,
    /// Ticks the replay records in total.
    pub total_ticks: u64,
    /// Digest cadence in ticks after defaulting.
    pub every: u64,
}

/// One seat's identity line.
#[derive(Debug, Serialize)]
pub struct SeatLine {
    /// Player id.
    pub seat: u8,
    /// Display name.
    pub name: String,
    /// Unit roster and sprite tint.
    pub faction: Faction,
    /// Normalized team id used by the simulation.
    pub team: u8,
    /// Whether the scenario assigns this seat to a built-in bot.
    pub bot: bool,
}

/// One timeline moment; `tick` lives on the wrapper so rendering and sorting
/// never match on the payload.
#[derive(Debug, Serialize)]
pub struct TimelineEntry {
    /// Completed state tick the moment belongs to.
    pub tick: u64,
    /// The tick as a `m:ss` clock.
    pub clock: String,
    /// What happened.
    #[serde(flatten)]
    pub kind: TimelineKind,
}

/// The timeline vocabulary.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineKind {
    /// The first two-sided combat event between a pair of hostile teams.
    FirstContact {
        /// The two team ids, smaller first.
        teams: (u8, u8),
        /// Tile of the struck target.
        at: [i32; 2],
    },
    /// A space-time cluster of losses worth at least the value floor.
    Battle {
        /// First loss tick.
        from_tick: u64,
        /// Last loss tick.
        to_tick: u64,
        /// Loss centroid tile.
        at: [i32; 2],
        /// Per participating seat, heaviest first.
        losses: Vec<SeatLoss>,
        /// `"even"`, or `"favors team N"` for the lightest-losing team.
        verdict: String,
    },
    /// A seat completed a Foundry beyond its starting count.
    Expansion {
        /// The expanding seat.
        seat: u8,
        /// The new Foundry's center tile, when it survived its own
        /// completion tick.
        at: Option<[i32; 2]>,
    },
    /// A seat restored a derelict Extractor frame.
    ExtractorOnline {
        /// The claiming seat.
        seat: u8,
        /// The Extractor's center tile, when it survived its own
        /// completion tick.
        at: Option<[i32; 2]>,
    },
    /// First of a curated loud kind for a seat: a tech-gate building or a
    /// tier-3 unit. Every other first lands only in the reach table.
    TechFirst {
        /// The seat reaching the kind.
        seat: u8,
        /// The kind's stable name.
        name: String,
    },
    /// A seat's Foundry was destroyed by enemy action.
    FoundryLost {
        /// The owner.
        seat: u8,
        /// Where it stood.
        at: [i32; 2],
    },
    /// The sim recorded the seat as eliminated.
    Elimination {
        /// The eliminated seat.
        seat: u8,
    },
    /// The seat conceded.
    Resignation {
        /// The conceding seat.
        seat: u8,
    },
    /// Consecutive digest windows with zero combat events after first
    /// contact.
    Lull {
        /// First quiet tick.
        from_tick: u64,
        /// Last quiet tick.
        to_tick: u64,
    },
    /// The match decided.
    GameOver {
        /// The sim's verdict.
        result: GameResult,
        /// Seats on the winning team, empty for a draw.
        winner_seats: Vec<u8>,
    },
}

/// One seat's losses inside a battle.
#[derive(Debug, Clone, Serialize)]
pub struct SeatLoss {
    /// The losing seat.
    pub seat: u8,
    /// Total lost value in scrap (tier-0 construction cost for buildings).
    pub value: u64,
    /// Units lost.
    pub units: u32,
    /// Buildings lost (never-completed sites included, at value zero).
    pub buildings: u32,
}

/// Per-seat state at one digest boundary.
#[derive(Debug, Serialize)]
pub struct Digest {
    /// Completed state tick of this digest.
    pub tick: u64,
    /// The tick as a `m:ss` clock.
    pub clock: String,
    /// Whether the match had already decided before this digest.
    pub post_game: bool,
    /// Sub-floor battles that closed inside this window.
    pub skirmishes: u32,
    /// One row per seat, in seat order.
    pub rows: Vec<SeatDigestRow>,
    /// Downsampled map, when the emission mode selects this digest.
    pub minimap: Option<Vec<String>>,
}

/// One seat's digest row.
#[derive(Debug, Serialize)]
pub struct SeatDigestRow {
    /// Player id.
    pub seat: u8,
    /// Living units.
    pub units: u32,
    /// Sum of living units' costs.
    pub army_value: u64,
    /// Top three unit kinds by count.
    pub top_kinds: Vec<(String, u32)>,
    /// Harvest-capable units.
    pub harvesters: u32,
    /// Harvest-capable units standing idle.
    pub harvesters_idle: u32,
    /// Scrap bank.
    pub bank: u32,
    /// Scrap hauled home by harvesters since the previous digest. Passive
    /// credits (Foundry drip, Extractor yield, Reclaimer trickle, recovery,
    /// salvage refunds) post to the bank without an event — the bank column
    /// carries them.
    pub hauled: u64,
    /// Standing (built) buildings.
    pub buildings: u32,
    /// Standing Foundries.
    pub foundries: u32,
    /// Explored share of the map in whole percent (vision is team-shared,
    /// so teammates repeat the same figure).
    pub explored_pct: u32,
    /// Commands the sim rejected since the previous digest.
    pub rejections: u32,
    /// Rejection counts by reason since the previous digest.
    pub rejection_reasons: BTreeMap<String, u32>,
    /// Orders that stalled since the previous digest.
    pub stalls: u32,
    /// Stall counts by reason since the previous digest.
    pub stall_reasons: BTreeMap<String, u32>,
}

/// One seat's first-of-kind ledger.
#[derive(Debug, Serialize)]
pub struct SeatTechReach {
    /// Player id.
    pub seat: u8,
    /// `(kind name, completed tick)` pairs sorted by tick.
    pub firsts: Vec<TechFirstRecord>,
}

/// One first-of-kind moment.
#[derive(Debug, Clone, Serialize)]
pub struct TechFirstRecord {
    /// The kind's stable name.
    pub name: String,
    /// Tick of the first training or completion.
    pub tick: u64,
    /// The tick as a `m:ss` clock.
    pub clock: String,
}

/// How the recorded match ended.
#[derive(Debug, Serialize)]
pub struct OutcomeLine {
    /// The sim's verdict, or `None` when the recording ends undecided.
    pub result: Option<GameResult>,
    /// Seats on the winning team.
    pub winner_seats: Vec<u8>,
    /// Tick the match decided.
    pub decided_at: Option<u64>,
    /// Ticks the recording continued past the decision.
    pub post_game_ticks: u64,
}

/// Ownership side tables the event stream cannot carry: deaths name no
/// killer, destroyed buildings name no kind, and shooters are ids.
struct Ledgers {
    unit_owner: BTreeMap<UnitId, u8>,
    building: BTreeMap<BuildingId, (u8, BuildingKind)>,
}

impl Ledgers {
    fn seed(state: &State) -> Self {
        Self {
            unit_owner: state
                .units()
                .iter()
                .map(|unit| (unit.id, unit.player.0))
                .collect(),
            building: state
                .buildings()
                .iter()
                .map(|building| (building.id, (building.player.0, building.kind)))
                .collect(),
        }
    }

    /// Resolves a target's owner, falling back to the live state for ids
    /// the event stream never introduced (construction sites placed
    /// mid-match emit no event until they complete).
    fn target_seat(&self, target: &Target, state: &State) -> Option<u8> {
        match target {
            Target::Unit(id) => self
                .unit_owner
                .get(id)
                .copied()
                .or_else(|| state.unit(*id).map(|unit| unit.player.0)),
            Target::Building(id) => self
                .building
                .get(id)
                .map(|(seat, _)| *seat)
                .or_else(|| state.building(*id).map(|building| building.player.0)),
        }
    }
}

/// One battle being accumulated.
struct BattleCluster {
    from_tick: u64,
    last_loss_tick: u64,
    sum_x: i64,
    sum_y: i64,
    n: i64,
    losses: BTreeMap<u8, SeatLoss>,
}

impl BattleCluster {
    fn centroid(&self) -> [i32; 2] {
        [(self.sum_x / self.n) as i32, (self.sum_y / self.n) as i32]
    }

    fn total_value(&self) -> u64 {
        self.losses.values().map(|loss| loss.value).sum()
    }
}

/// A closed battle above the value floor.
struct Battle {
    from_tick: u64,
    to_tick: u64,
    at: [i32; 2],
    losses: Vec<SeatLoss>,
}

/// Streaming space-time clusterer over loss events. Pure integer math and
/// first-match-in-insertion-order keep it bit-deterministic.
#[derive(Default)]
struct BattleClusterer {
    active: Vec<BattleCluster>,
    battles: Vec<Battle>,
    /// `(closing tick, 1)` per sub-floor battle, for the window counters.
    skirmishes: Vec<u64>,
}

impl BattleClusterer {
    fn note_loss(&mut self, tick: u64, seat: u8, tile: TilePos, value: u64, building: bool) {
        self.expire(tick);
        let x = i64::from(tile.x);
        let y = i64::from(tile.y);
        let joined = self.active.iter_mut().find(|cluster| {
            let dx = cluster.n * x - cluster.sum_x;
            let dy = cluster.n * y - cluster.sum_y;
            let radius = BATTLE_RADIUS_TILES * cluster.n;
            dx * dx + dy * dy <= radius * radius
        });
        let cluster = match joined {
            Some(cluster) => cluster,
            None => {
                self.active.push(BattleCluster {
                    from_tick: tick,
                    last_loss_tick: tick,
                    sum_x: 0,
                    sum_y: 0,
                    n: 0,
                    losses: BTreeMap::new(),
                });
                self.active.last_mut().expect("just pushed")
            }
        };
        cluster.sum_x += x;
        cluster.sum_y += y;
        cluster.n += 1;
        cluster.last_loss_tick = tick;
        let loss = cluster.losses.entry(seat).or_insert(SeatLoss {
            seat,
            value: 0,
            units: 0,
            buildings: 0,
        });
        loss.value += value;
        if building {
            loss.buildings += 1;
        } else {
            loss.units += 1;
        }
    }

    fn expire(&mut self, tick: u64) {
        let mut index = 0;
        while index < self.active.len() {
            if self.active[index].last_loss_tick + BATTLE_QUIET_TICKS < tick {
                let cluster = self.active.remove(index);
                self.close(cluster);
            } else {
                index += 1;
            }
        }
    }

    fn finish(&mut self) {
        while let Some(cluster) = self.active.pop() {
            self.close(cluster);
        }
        self.battles.sort_by_key(|battle| battle.from_tick);
        self.skirmishes.sort_unstable();
    }

    fn close(&mut self, cluster: BattleCluster) {
        if cluster.total_value() >= BATTLE_VALUE_FLOOR {
            let at = cluster.centroid();
            let mut losses: Vec<SeatLoss> = cluster.losses.into_values().collect();
            losses.sort_by(|a, b| b.value.cmp(&a.value).then(a.seat.cmp(&b.seat)));
            self.battles.push(Battle {
                from_tick: cluster.from_tick,
                to_tick: cluster.last_loss_tick,
                at,
                losses,
            });
        } else {
            self.skirmishes.push(cluster.last_loss_tick);
        }
    }
}

/// `"even"` when the two heaviest TEAM losses are within 25% of each other,
/// else the lightest-losing team is favored. Aggregating by team keeps team
/// maps honest: two allies each losing half an army must not read as an
/// even three-way trade against their lone opponent.
fn battle_verdict(losses: &[SeatLoss], seat_team: &[u8]) -> String {
    let mut team_losses: BTreeMap<u8, u64> = BTreeMap::new();
    for loss in losses {
        let team = seat_team
            .get(loss.seat as usize)
            .copied()
            .unwrap_or(loss.seat);
        *team_losses.entry(team).or_default() += loss.value;
    }
    let mut ordered: Vec<(u64, u8)> = team_losses
        .into_iter()
        .map(|(team, value)| (value, team))
        .collect();
    // Heaviest first; value ties go to the lower team id.
    ordered.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    match ordered.as_slice() {
        [] => "even".to_owned(),
        [(_, team)] => format!("one-sided against team {team}"),
        [(heaviest, _), (second, _), ..] if second * 4 >= heaviest * 3 => "even".to_owned(),
        _ => {
            let lightest = ordered
                .iter()
                .min_by_key(|(value, team)| (*value, *team))
                .expect("at least two teams");
            format!("favors team {}", lightest.1)
        }
    }
}

/// Windowed per-seat accumulators, reset at each emitted digest.
#[derive(Default, Clone)]
struct SeatWindow {
    hauled: u64,
    rejections: u32,
    rejection_reasons: BTreeMap<String, u32>,
    stalls: u32,
    stall_reasons: BTreeMap<String, u32>,
}

/// Re-executes `replay` once and returns the digest. Deterministic: the same
/// replay and options yield the same report, byte for byte.
pub fn summarize(replay: &GameReplay, opts: &SummaryOptions) -> Result<SummaryReport> {
    replay.validate(Some(SIM_VERSION))?;
    let total = replay_duration(replay);
    anyhow::ensure!(
        total <= MAX_REPLAY_TICKS,
        "replay spans {total} ticks, beyond the {MAX_REPLAY_TICKS}-tick bound"
    );
    let effective = opts.until.map_or(total, |until| until.min(total));
    let every = opts
        .every
        .unwrap_or_else(|| (effective / 16).clamp(2_000, 10_000))
        .max(1);

    let mut state = replay.setup.build().context("building replay setup")?;
    let seats: Vec<SeatLine> = replay
        .setup
        .players
        .iter()
        .enumerate()
        .map(|(seat, spec)| SeatLine {
            seat: seat as u8,
            name: spec.name.clone(),
            faction: spec.faction,
            team: state.players()[seat].team,
            bot: spec.bot,
        })
        .collect();
    let seat_count = seats.len();
    let seat_team: Vec<u8> = seats.iter().map(|seat| seat.team).collect();

    let mut ledgers = Ledgers::seed(&state);
    let mut clusterer = BattleClusterer::default();
    let mut timeline: Vec<TimelineEntry> = Vec::new();
    let mut digests: Vec<Digest> = Vec::new();

    // Tick-0 seeds: authored buildings never emit completion events, and a
    // kind a seat starts with is not a tech first.
    let mut completed_ids: BTreeSet<BuildingId> = BTreeSet::new();
    let tick0_foundries: Vec<u32> = (0..seat_count)
        .map(|seat| built_count(&state, seat as u8, BuildingKind::Foundry))
        .collect();
    let mut unit_reach: Vec<BTreeSet<UnitKind>> = (0..seat_count)
        .map(|seat| {
            state
                .units()
                .iter()
                .filter(|unit| unit.player.0 as usize == seat)
                .map(|unit| unit.kind)
                .collect()
        })
        .collect();
    let mut building_reach: Vec<BTreeSet<BuildingKind>> = (0..seat_count)
        .map(|seat| {
            state
                .buildings()
                .iter()
                .filter(|building| building.player.0 as usize == seat && building.built)
                .map(|building| building.kind)
                .collect()
        })
        .collect();
    let mut tech_firsts: Vec<Vec<TechFirstRecord>> = vec![Vec::new(); seat_count];
    let mut eliminated: Vec<bool> = (0..seat_count)
        .map(|seat| state.players()[seat].eliminated_at.is_some())
        .collect();
    let mut contacted: BTreeSet<(u8, u8)> = BTreeSet::new();

    let mut windows: Vec<SeatWindow> = vec![SeatWindow::default(); seat_count];
    let mut window_combat: u64 = 0;
    let mut quiet_run: Option<QuietRun> = None;
    let mut prev_boundary: u64 = 0;

    let mut game_over_tick: Option<u64> = None;

    let mut cursor = replay.cursor();
    for tick in 0..effective {
        let commands: Vec<oxide_sim::PlayerCommand> = cursor
            .take_tick(tick)
            .iter()
            .map(|timed| timed.command.clone())
            .collect();
        let report = state.tick(&commands);
        let now = state.current_tick();

        for event in &report.events {
            match event {
                Event::UnitTrained { unit, kind, player } => {
                    ledgers.unit_owner.insert(*unit, player.0);
                    note_tech_first(
                        &mut unit_reach[player.0 as usize],
                        &mut tech_firsts[player.0 as usize],
                        *kind,
                        kind.name(),
                        LOUD_UNITS.contains(kind),
                        player.0,
                        now,
                        &mut timeline,
                    );
                }
                Event::UnitDied {
                    kind, player, pos, ..
                } => {
                    window_combat += 1;
                    clusterer.note_loss(
                        now,
                        player.0,
                        TilePos::containing(*pos),
                        u64::from(kind.stats().cost),
                        false,
                    );
                }
                Event::BuildingDestroyed {
                    building,
                    player,
                    pos,
                } => {
                    let tile = TilePos::containing(*pos);
                    let known = ledgers.building.get(building).map(|(_, kind)| *kind);
                    let value = known
                        .and_then(|kind| kind.base_stats().construction)
                        .map_or(0, |construction| u64::from(construction.cost));
                    clusterer.note_loss(now, player.0, tile, value, true);
                    if known == Some(BuildingKind::Foundry) {
                        timeline.push(entry(
                            now,
                            TimelineKind::FoundryLost {
                                seat: player.0,
                                at: [tile.x, tile.y],
                            },
                        ));
                    }
                }
                Event::BuildingCompleted {
                    building,
                    player,
                    kind,
                } => {
                    ledgers.building.insert(*building, (player.0, *kind));
                    if completed_ids.insert(*building) {
                        let seat = player.0;
                        note_tech_first(
                            &mut building_reach[seat as usize],
                            &mut tech_firsts[seat as usize],
                            *kind,
                            kind.name(),
                            LOUD_BUILDINGS.contains(kind),
                            seat,
                            now,
                            &mut timeline,
                        );
                        let at = state
                            .building(*building)
                            .map(|b| TilePos::containing(b.center()))
                            .map(|tile| [tile.x, tile.y]);
                        match kind {
                            BuildingKind::Foundry
                                if built_count(&state, seat, BuildingKind::Foundry)
                                    > tick0_foundries[seat as usize] =>
                            {
                                timeline.push(entry(now, TimelineKind::Expansion { seat, at }));
                            }
                            BuildingKind::Extractor => {
                                timeline
                                    .push(entry(now, TimelineKind::ExtractorOnline { seat, at }));
                            }
                            _ => {}
                        }
                    }
                }
                Event::AttackHit {
                    attacker,
                    target,
                    target_pos,
                    ..
                } => {
                    window_combat += 1;
                    let shooter = ledgers.unit_owner.get(attacker).copied();
                    note_contact(
                        &state,
                        shooter,
                        ledgers.target_seat(target, &state),
                        TilePos::containing(*target_pos),
                        now,
                        &mut contacted,
                        &mut timeline,
                    );
                }
                Event::TurretFired {
                    turret,
                    target,
                    target_pos,
                    ..
                } => {
                    window_combat += 1;
                    let shooter = ledgers.building.get(turret).map(|(seat, _)| *seat);
                    note_contact(
                        &state,
                        shooter,
                        ledgers.target_seat(target, &state),
                        TilePos::containing(*target_pos),
                        now,
                        &mut contacted,
                        &mut timeline,
                    );
                }
                Event::ShellLaunched {
                    target, player, to, ..
                } => {
                    window_combat += 1;
                    note_contact(
                        &state,
                        Some(player.0),
                        ledgers.target_seat(target, &state),
                        TilePos::containing(*to),
                        now,
                        &mut contacted,
                        &mut timeline,
                    );
                }
                Event::ChargeDetonated { .. } => {
                    window_combat += 1;
                }
                Event::ScrapDeposited { player, amount } => {
                    windows[player.0 as usize].hauled += u64::from(*amount);
                }
                Event::CommandRejected { player, reason } => {
                    let window = &mut windows[player.0 as usize];
                    window.rejections += 1;
                    *window
                        .rejection_reasons
                        .entry(format!("{reason:?}"))
                        .or_default() += 1;
                }
                Event::OrderStalled { player, reason, .. } => {
                    let window = &mut windows[player.0 as usize];
                    window.stalls += 1;
                    *window
                        .stall_reasons
                        .entry(format!("{reason:?}"))
                        .or_default() += 1;
                }
                Event::PlayerResigned { player } => {
                    timeline.push(entry(now, TimelineKind::Resignation { seat: player.0 }));
                }
                Event::GameOver { result } => {
                    game_over_tick = Some(now);
                    timeline.push(entry(
                        now,
                        TimelineKind::GameOver {
                            result: *result,
                            winner_seats: state.winners().into_iter().map(|seat| seat.0).collect(),
                        },
                    ));
                }
                _ => {}
            }
        }

        for (seat, done) in eliminated.iter_mut().enumerate() {
            if !*done && state.players()[seat].eliminated_at.is_some() {
                *done = true;
                timeline.push(entry(now, TimelineKind::Elimination { seat: seat as u8 }));
            }
        }

        clusterer.expire(now);

        let is_boundary = now.is_multiple_of(every) || now == effective;
        if is_boundary {
            // Construction sites emit no event until completion; learn them
            // here so a later destruction still resolves kind and value.
            for building in state.buildings() {
                ledgers
                    .building
                    .entry(building.id)
                    .or_insert((building.player.0, building.kind));
            }
            let post_game = game_over_tick.is_some_and(|over| now > over);
            // A decided match keeps only its closing digest: nothing moves
            // after victory, so intermediate post-game digests are noise.
            let emit = !post_game || now == effective;
            if !post_game {
                track_lull(
                    &mut quiet_run,
                    &mut timeline,
                    window_combat,
                    !contacted.is_empty(),
                    prev_boundary,
                    now,
                );
            }
            if emit {
                let with_minimap = match opts.minimaps {
                    MinimapMode::All => true,
                    MinimapMode::Sparse => {
                        now == effective || (digests.len() + 1).is_multiple_of(4)
                    }
                    MinimapMode::None => false,
                };
                digests.push(capture_digest(
                    &state,
                    now,
                    post_game,
                    &mut windows,
                    with_minimap,
                ));
            }
            window_combat = 0;
            prev_boundary = now;
        }
    }
    if effective == total {
        anyhow::ensure!(
            cursor.is_finished(),
            "playback of {total} ticks left recorded commands unconsumed"
        );
    }
    flush_lull(&mut quiet_run, &mut timeline);
    clusterer.finish();

    if digests.is_empty() {
        // A zero-tick replay still gets its opening state.
        digests.push(capture_digest(
            &state,
            state.current_tick(),
            false,
            &mut windows,
            opts.minimaps != MinimapMode::None,
        ));
    }

    for battle in &clusterer.battles {
        timeline.push(entry(
            battle.from_tick,
            TimelineKind::Battle {
                from_tick: battle.from_tick,
                to_tick: battle.to_tick,
                at: battle.at,
                losses: battle.losses.clone(),
                verdict: battle_verdict(&battle.losses, &seat_team),
            },
        ));
    }
    timeline.sort_by_key(|moment| moment.tick);

    // Map each sub-floor battle to the digest window it closed inside.
    for &closed_at in &clusterer.skirmishes {
        if let Some(digest) = digests.iter_mut().find(|digest| digest.tick >= closed_at) {
            digest.skirmishes += 1;
        } else if let Some(last) = digests.last_mut() {
            last.skirmishes += 1;
        }
    }

    let tech_reach = tech_firsts
        .into_iter()
        .enumerate()
        .map(|(seat, mut firsts)| {
            firsts.sort_by_key(|first| first.tick);
            SeatTechReach {
                seat: seat as u8,
                firsts,
            }
        })
        .collect();

    let outcome = OutcomeLine {
        result: state.result(),
        winner_seats: state.winners().into_iter().map(|seat| seat.0).collect(),
        decided_at: game_over_tick,
        post_game_ticks: game_over_tick.map_or(0, |over| effective.saturating_sub(over)),
    };

    Ok(SummaryReport {
        schema_version: REPLAY_SUMMARY_SCHEMA_VERSION,
        scenario: ScenarioLine {
            name: replay.setup.name.clone(),
            seed: replay.setup.seed,
            map_width: state.map().width(),
            map_height: state.map().height(),
            effective_ticks: effective,
            total_ticks: total,
            every,
        },
        seats,
        timeline,
        digests,
        tech_reach,
        outcome,
    })
}

fn entry(tick: u64, kind: TimelineKind) -> TimelineEntry {
    TimelineEntry {
        tick,
        clock: clock(tick),
        kind,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a per-event fold's working set, not an API"
)]
fn note_tech_first<K: Ord + Copy>(
    reach: &mut BTreeSet<K>,
    firsts: &mut Vec<TechFirstRecord>,
    kind: K,
    name: &'static str,
    loud: bool,
    seat: u8,
    tick: u64,
    timeline: &mut Vec<TimelineEntry>,
) {
    if !reach.insert(kind) {
        return;
    }
    firsts.push(TechFirstRecord {
        name: name.to_owned(),
        tick,
        clock: clock(tick),
    });
    if loud {
        timeline.push(entry(
            tick,
            TimelineKind::TechFirst {
                seat,
                name: name.to_owned(),
            },
        ));
    }
}

fn note_contact(
    state: &State,
    shooter: Option<u8>,
    victim: Option<u8>,
    at: TilePos,
    tick: u64,
    contacted: &mut BTreeSet<(u8, u8)>,
    timeline: &mut Vec<TimelineEntry>,
) {
    let (Some(shooter), Some(victim)) = (shooter, victim) else {
        return;
    };
    if !state.hostile(PlayerId(shooter), PlayerId(victim)) {
        return;
    }
    let team_a = state.players()[shooter as usize].team;
    let team_b = state.players()[victim as usize].team;
    let teams = (team_a.min(team_b), team_a.max(team_b));
    if contacted.insert(teams) {
        timeline.push(entry(
            tick,
            TimelineKind::FirstContact {
                teams,
                at: [at.x, at.y],
            },
        ));
    }
}

/// A run of quiet digest windows: `(first quiet tick, last quiet tick,
/// window count)`.
type QuietRun = (u64, u64, u64);

/// Folds one closed digest window into the running lull state. A quiet
/// window extends the run; a loud one flushes it (two or more quiet
/// windows make a timeline lull). The opening build-up before first
/// contact is deliberately not a lull.
fn track_lull(
    quiet_run: &mut Option<QuietRun>,
    timeline: &mut Vec<TimelineEntry>,
    window_combat: u64,
    contact_made: bool,
    window_from: u64,
    window_to: u64,
) {
    if window_combat == 0 && contact_made {
        match quiet_run {
            Some((_, last_to, windows)) => {
                *last_to = window_to;
                *windows += 1;
            }
            None => *quiet_run = Some((window_from, window_to, 1)),
        }
    } else {
        flush_lull(quiet_run, timeline);
    }
}

/// Emits the pending quiet run as a lull when it spans two or more windows.
fn flush_lull(quiet_run: &mut Option<QuietRun>, timeline: &mut Vec<TimelineEntry>) {
    if let Some((from, to, windows)) = quiet_run.take()
        && windows >= 2
    {
        timeline.push(entry(
            from,
            TimelineKind::Lull {
                from_tick: from,
                to_tick: to,
            },
        ));
    }
}

fn built_count(state: &State, seat: u8, kind: BuildingKind) -> u32 {
    state
        .buildings()
        .iter()
        .filter(|building| building.player.0 == seat && building.kind == kind && building.built)
        .count() as u32
}

fn capture_digest(
    state: &State,
    tick: u64,
    post_game: bool,
    windows: &mut [SeatWindow],
    with_minimap: bool,
) -> Digest {
    let seat_count = state.players().len();
    // Vision is team-shared: compute each team's explored share once.
    let mut team_explored: BTreeMap<u8, u32> = BTreeMap::new();
    let map = state.map();
    let tiles = u64::from(map.width().unsigned_abs()) * u64::from(map.height().unsigned_abs());
    for seat in 0..seat_count {
        let team = state.players()[seat].team;
        team_explored.entry(team).or_insert_with(|| {
            let vision = state.vision(PlayerId(seat as u8));
            let mut explored: u64 = 0;
            for y in 0..map.height() {
                for x in 0..map.width() {
                    if vision.explored(TilePos::new(x, y)) {
                        explored += 1;
                    }
                }
            }
            ((explored * 100) / tiles.max(1)) as u32
        });
    }

    let rows = (0..seat_count)
        .map(|seat| {
            let seat_id = seat as u8;
            let mut units: u32 = 0;
            let mut army_value: u64 = 0;
            let mut harvesters: u32 = 0;
            let mut harvesters_idle: u32 = 0;
            let mut kind_counts: BTreeMap<&'static str, u32> = BTreeMap::new();
            for unit in state.units() {
                if unit.player.0 != seat_id {
                    continue;
                }
                units += 1;
                army_value += u64::from(unit.kind.stats().cost);
                *kind_counts.entry(unit.kind.name()).or_default() += 1;
                if unit.kind.stats().harvest.is_some() {
                    harvesters += 1;
                    if matches!(unit.order, Order::Idle) {
                        harvesters_idle += 1;
                    }
                }
            }
            let mut kinds: Vec<(&'static str, u32)> = kind_counts.into_iter().collect();
            kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            kinds.truncate(3);
            let buildings = state
                .buildings()
                .iter()
                .filter(|building| building.player.0 == seat_id && building.built)
                .count() as u32;
            let window = std::mem::take(&mut windows[seat]);
            SeatDigestRow {
                seat: seat_id,
                units,
                army_value,
                top_kinds: kinds
                    .into_iter()
                    .map(|(name, count)| (name.to_owned(), count))
                    .collect(),
                harvesters,
                harvesters_idle,
                bank: state.players()[seat].scrap,
                hauled: window.hauled,
                buildings,
                foundries: built_count(state, seat_id, BuildingKind::Foundry),
                explored_pct: team_explored[&state.players()[seat].team],
                rejections: window.rejections,
                rejection_reasons: window.rejection_reasons,
                stalls: window.stalls,
                stall_reasons: window.stall_reasons,
            }
        })
        .collect();

    Digest {
        tick,
        clock: clock(tick),
        post_game,
        skirmishes: 0,
        rows,
        minimap: with_minimap.then(|| minimap(state)),
    }
}

/// Downsampled whole-map view: `≤46` columns, one text row per `2×cell`
/// tile rows (terminal glyphs are ~2:1 tall). Per cell, precedence
/// top-down: a seat's Foundry digit (the immobile strategic anchor), the
/// dominant seat's letter (UPPERCASE at or above the mass threshold),
/// `$` for remaining scrap, then the most frequent terrain.
fn minimap(state: &State) -> Vec<String> {
    let map = state.map();
    let ceil_div = |a: i32, b: i32| (a + b - 1) / b;
    let width = map.width().max(1);
    let height = map.height().max(1);
    let cell = ceil_div(width, 46).max(ceil_div(height, 32)).max(1);
    let cols = ceil_div(width, cell) as usize;
    let rows = ceil_div(height, 2 * cell) as usize;
    let seat_count = state.players().len();

    let index = |tile: TilePos| -> usize {
        let cx = (tile.x / cell) as usize;
        let cy = (tile.y / (2 * cell)) as usize;
        cy.min(rows - 1) * cols + cx.min(cols - 1)
    };

    // Terrain census: Ground, Rock, Peak, Pit (ties break in that order).
    let mut terrain = vec![[0u32; 4]; cols * rows];
    let mut scrap = vec![false; cols * rows];
    for (tile, info) in map.iter() {
        let slot = &mut terrain[index(tile)];
        match info.terrain {
            oxide_sim::map::Terrain::Ground => slot[0] += 1,
            oxide_sim::map::Terrain::Rock => slot[1] += 1,
            oxide_sim::map::Terrain::Peak => slot[2] += 1,
            oxide_sim::map::Terrain::Pit => slot[3] += 1,
        }
        if info.scrap > 0 {
            scrap[index(tile)] = true;
        }
    }

    let mut value = vec![vec![0u64; seat_count]; cols * rows];
    let mut presence = vec![vec![false; seat_count]; cols * rows];
    for unit in state.units() {
        let seat = unit.player.0 as usize;
        let at = index(unit.tile());
        value[at][seat] += u64::from(unit.kind.stats().cost);
        presence[at][seat] = true;
    }
    let mut foundry: Vec<Option<u8>> = vec![None; cols * rows];
    for building in state.buildings() {
        if !building.built {
            continue;
        }
        let seat = building.player.0;
        let at = index(TilePos::containing(building.center()));
        presence[at][seat as usize] = true;
        if building.kind == BuildingKind::Foundry {
            let slot = &mut foundry[at];
            *slot = Some(slot.map_or(seat, |existing| existing.min(seat)));
        }
    }

    (0..rows)
        .map(|cy| {
            (0..cols)
                .map(|cx| {
                    let at = cy * cols + cx;
                    if let Some(seat) = foundry[at] {
                        // Seats past the digit range (scenarios allow up to
                        // 16) share a generic marker rather than colliding
                        // with letters or punctuation.
                        return if seat < 10 {
                            char::from(b'0' + seat)
                        } else {
                            '+'
                        };
                    }
                    let dominant = (0..seat_count)
                        .filter(|&seat| presence[at][seat])
                        .max_by_key(|&seat| (value[at][seat], std::cmp::Reverse(seat)));
                    if let Some(seat) = dominant {
                        let letter = b'a' + seat as u8;
                        return if value[at][seat] >= MINIMAP_MASS_THRESHOLD {
                            char::from(letter.to_ascii_uppercase())
                        } else {
                            char::from(letter)
                        };
                    }
                    if scrap[at] {
                        return '$';
                    }
                    let census = &terrain[at];
                    let best = (0..4).max_by_key(|&kind| (census[kind], std::cmp::Reverse(kind)));
                    match best {
                        Some(1) => '#',
                        Some(2) => '^',
                        Some(3) => '~',
                        _ => '·',
                    }
                })
                .collect()
        })
        .collect()
}

fn replay_duration(replay: &GameReplay) -> u64 {
    replay
        .meta
        .ticks
        .unwrap_or_else(|| replay.commands.last().map_or(0, |command| command.tick + 1))
}

/// Ticks as a `m:ss` clock (minutes roll past 59 rather than into hours).
fn clock(ticks: u64) -> String {
    let seconds = ticks / u64::from(TICKS_PER_SECOND);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl SummaryReport {
    /// The human rendering; JSON carries the same data.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let scenario = &self.scenario;
        let _ = writeln!(
            out,
            "{} — seed {}, {}x{}, {} ticks ({}), digest every {} ({})",
            scenario.name,
            scenario.seed,
            scenario.map_width,
            scenario.map_height,
            scenario.effective_ticks,
            clock(scenario.effective_ticks),
            scenario.every,
            clock(scenario.every),
        );
        if scenario.effective_ticks < scenario.total_ticks {
            let _ = writeln!(
                out,
                "  (truncated: the replay records {} ticks)",
                scenario.total_ticks
            );
        }
        for seat in &self.seats {
            let _ = writeln!(
                out,
                "  seat {}: {}  {:?}  team {}  {}",
                seat.seat,
                seat.name,
                seat.faction,
                seat.team,
                if seat.bot { "bot" } else { "human" },
            );
        }
        let _ = writeln!(out, "timeline:");
        for moment in &self.timeline {
            let _ = writeln!(out, "  [{}] {}", moment.clock, render_moment(&moment.kind));
        }
        for digest in &self.digests {
            let mut header = format!("[{}] digest t={}", digest.clock, digest.tick);
            if digest.post_game {
                header.push_str(" (post-game)");
            }
            if digest.skirmishes > 0 {
                let _ = write!(header, "  skirmishes: {}", digest.skirmishes);
            }
            let _ = writeln!(out, "{header}");
            for row in &digest.rows {
                let kinds = row
                    .top_kinds
                    .iter()
                    .map(|(name, count)| format!("{name} x{count}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let reasons = |map: &BTreeMap<String, u32>| -> String {
                    if map.is_empty() {
                        String::new()
                    } else {
                        let parts = map
                            .iter()
                            .map(|(reason, count)| format!("{reason} x{count}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(" [{parts}]")
                    }
                };
                let _ = writeln!(
                    out,
                    "  s{}: {}u val {} ({})  harv {}/{} idle  bank {} +{}  bld {} ({} foundry)  expl {}%  rej {}{} stall {}{}",
                    row.seat,
                    row.units,
                    row.army_value,
                    kinds,
                    row.harvesters,
                    row.harvesters_idle,
                    row.bank,
                    row.hauled,
                    row.buildings,
                    row.foundries,
                    row.explored_pct,
                    row.rejections,
                    reasons(&row.rejection_reasons),
                    row.stalls,
                    reasons(&row.stall_reasons),
                );
            }
            if let Some(minimap) = &digest.minimap {
                let _ = writeln!(
                    out,
                    "  map: a-{} units (CAPS massed ≥{}) 0-{} foundry · ground # rock ^ peak ~ pit $ scrap",
                    char::from(b'a' + (self.seats.len().saturating_sub(1)) as u8),
                    MINIMAP_MASS_THRESHOLD,
                    self.seats.len().saturating_sub(1),
                );
                for row in minimap {
                    let _ = writeln!(out, "  {row}");
                }
            }
        }
        let _ = writeln!(out, "tech reach:");
        for reach in &self.tech_reach {
            let firsts = reach
                .firsts
                .iter()
                .map(|first| format!("{}@{}", first.name, first.clock))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                out,
                "  s{}: {}",
                reach.seat,
                if firsts.is_empty() { "—" } else { &firsts }
            );
        }
        match (&self.outcome.result, self.outcome.decided_at) {
            (Some(result), Some(decided)) => {
                let _ = writeln!(
                    out,
                    "result: {} — decided at {} (t={}), {} post-game ticks",
                    render_result(result, &self.outcome.winner_seats),
                    clock(decided),
                    decided,
                    self.outcome.post_game_ticks,
                );
            }
            (Some(result), None) => {
                let _ = writeln!(
                    out,
                    "result: {}",
                    render_result(result, &self.outcome.winner_seats)
                );
            }
            _ => {
                let _ = writeln!(out, "result: undecided at the recording's end");
            }
        }
        out
    }
}

fn render_moment(kind: &TimelineKind) -> String {
    match kind {
        TimelineKind::FirstContact { teams, at } => format!(
            "first contact: team {} <-> team {} at ({},{})",
            teams.0, teams.1, at[0], at[1]
        ),
        TimelineKind::Battle {
            from_tick,
            to_tick,
            at,
            losses,
            verdict,
        } => {
            let parts = losses
                .iter()
                .map(|loss| {
                    let mut part = format!("seat {} lost {}", loss.seat, loss.value);
                    let mut counts = Vec::new();
                    if loss.units > 0 {
                        counts.push(format!("{}u", loss.units));
                    }
                    if loss.buildings > 0 {
                        counts.push(format!("{}b", loss.buildings));
                    }
                    if !counts.is_empty() {
                        let _ = write!(part, " ({})", counts.join("+"));
                    }
                    part
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "battle {}-{} at ({},{}): {parts} — {verdict}",
                clock(*from_tick),
                clock(*to_tick),
                at[0],
                at[1],
            )
        }
        TimelineKind::Expansion { seat, at } => match at {
            Some(at) => format!("expansion: seat {seat} foundry at ({},{})", at[0], at[1]),
            None => format!("expansion: seat {seat} foundry"),
        },
        TimelineKind::ExtractorOnline { seat, at } => match at {
            Some(at) => format!("extractor online: seat {seat} at ({},{})", at[0], at[1]),
            None => format!("extractor online: seat {seat}"),
        },
        TimelineKind::TechFirst { seat, name } => format!("tech first: seat {seat} {name}"),
        TimelineKind::FoundryLost { seat, at } => {
            format!("foundry lost: seat {seat} at ({},{})", at[0], at[1])
        }
        TimelineKind::Elimination { seat } => format!("eliminated: seat {seat}"),
        TimelineKind::Resignation { seat } => format!("resigned: seat {seat}"),
        TimelineKind::Lull { from_tick, to_tick } => {
            format!("lull: no combat {}-{}", clock(*from_tick), clock(*to_tick))
        }
        TimelineKind::GameOver {
            result,
            winner_seats,
        } => format!("game over: {}", render_result(result, winner_seats)),
    }
}

fn render_result(result: &GameResult, winner_seats: &[u8]) -> String {
    match result {
        GameResult::Victory { team } => {
            let seats = winner_seats
                .iter()
                .map(|seat| seat.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("victory team {team} (seats: {seats})")
        }
        GameResult::Draw => "draw".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chassis::replay::Replay;
    use oxide_sim::{Command, PlayerCommand, Scenario};

    fn stop(player: u8, unit: UnitId) -> PlayerCommand {
        PlayerCommand {
            player: PlayerId(player),
            command: Command::Stop { units: vec![unit] },
        }
    }

    fn fixture() -> GameReplay {
        let scenario = Scenario::skirmish();
        let state = scenario.build().expect("skirmish builds");
        let unit = |seat| {
            state
                .units()
                .iter()
                .find(|unit| unit.player == PlayerId(seat))
                .expect("each skirmish seat starts with a unit")
                .id
        };
        let mut replay = Replay::new(SIM_VERSION, scenario);
        replay.record(0, stop(0, unit(0)));
        replay.record(3, stop(1, unit(1)));
        replay.record(5, stop(0, unit(0)));
        replay.record(8, stop(1, unit(1)));
        replay.record(10, stop(0, unit(0)));
        replay.meta.ticks = Some(12);
        replay
    }

    fn opts(until: Option<u64>, every: Option<u64>) -> SummaryOptions {
        SummaryOptions {
            until,
            every,
            minimaps: MinimapMode::None,
        }
    }

    #[test]
    fn clock_formats_minutes_and_seconds() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(1_800), "1:30");
        assert_eq!(clock(160_000), "133:20");
    }

    #[test]
    fn nearby_losses_merge_and_distant_losses_split() {
        let mut clusterer = BattleClusterer::default();
        clusterer.note_loss(100, 0, TilePos::new(10, 10), 90, false);
        clusterer.note_loss(200, 1, TilePos::new(18, 10), 110, false);
        clusterer.note_loss(250, 0, TilePos::new(40, 40), 500, false);
        clusterer.finish();
        assert_eq!(clusterer.battles.len(), 2);
        let merged = &clusterer.battles[0];
        assert_eq!(merged.from_tick, 100);
        assert_eq!(merged.to_tick, 200);
        assert_eq!(merged.losses.len(), 2);
        assert_eq!(clusterer.battles[1].losses[0].value, 500);
    }

    #[test]
    fn a_quiet_gap_closes_the_battle() {
        let mut clusterer = BattleClusterer::default();
        clusterer.note_loss(100, 0, TilePos::new(10, 10), 300, false);
        clusterer.note_loss(
            100 + BATTLE_QUIET_TICKS + 1,
            1,
            TilePos::new(10, 10),
            300,
            false,
        );
        clusterer.finish();
        assert_eq!(clusterer.battles.len(), 2);
    }

    #[test]
    fn sub_floor_battles_fold_into_skirmishes() {
        let mut clusterer = BattleClusterer::default();
        clusterer.note_loss(100, 0, TilePos::new(10, 10), BATTLE_VALUE_FLOOR - 20, false);
        clusterer.note_loss(
            2_000,
            0,
            TilePos::new(10, 10),
            BATTLE_VALUE_FLOOR + 10,
            false,
        );
        clusterer.finish();
        assert_eq!(clusterer.battles.len(), 1);
        assert_eq!(clusterer.skirmishes, vec![100]);
    }

    #[test]
    fn verdicts_read_the_loss_gap_by_team() {
        let loss = |seat, value| SeatLoss {
            seat,
            value,
            units: 1,
            buildings: 0,
        };
        let duel = &[0u8, 1];
        assert_eq!(battle_verdict(&[loss(0, 400), loss(1, 300)], duel), "even");
        assert_eq!(
            battle_verdict(&[loss(0, 400), loss(1, 100)], duel),
            "favors team 1"
        );
        assert_eq!(
            battle_verdict(&[loss(0, 400)], duel),
            "one-sided against team 0"
        );
        // Two allies each losing "less than" their lone opponent still lost
        // the exchange as a team.
        let two_on_one = &[0u8, 1, 0];
        assert_eq!(
            battle_verdict(&[loss(1, 400), loss(0, 300), loss(2, 300)], two_on_one),
            "favors team 1"
        );
    }

    #[test]
    fn every_timeline_moment_renders() {
        let loss = SeatLoss {
            seat: 0,
            value: 300,
            units: 3,
            buildings: 1,
        };
        let cases: Vec<(TimelineKind, &str)> = vec![
            (
                TimelineKind::FirstContact {
                    teams: (0, 1),
                    at: [3, 4],
                },
                "first contact: team 0 <-> team 1 at (3,4)",
            ),
            (
                TimelineKind::Battle {
                    from_tick: 0,
                    to_tick: 600,
                    at: [5, 6],
                    losses: vec![loss],
                    verdict: "even".into(),
                },
                "battle 0:00-0:30 at (5,6): seat 0 lost 300 (3u+1b) — even",
            ),
            (
                TimelineKind::Expansion {
                    seat: 2,
                    at: Some([7, 8]),
                },
                "expansion: seat 2 foundry at (7,8)",
            ),
            (
                TimelineKind::ExtractorOnline { seat: 1, at: None },
                "extractor online: seat 1",
            ),
            (
                TimelineKind::TechFirst {
                    seat: 3,
                    name: "avalanche".into(),
                },
                "tech first: seat 3 avalanche",
            ),
            (
                TimelineKind::FoundryLost {
                    seat: 4,
                    at: [9, 1],
                },
                "foundry lost: seat 4 at (9,1)",
            ),
            (TimelineKind::Elimination { seat: 4 }, "eliminated: seat 4"),
            (TimelineKind::Resignation { seat: 5 }, "resigned: seat 5"),
            (
                TimelineKind::Lull {
                    from_tick: 1_200,
                    to_tick: 4_800,
                },
                "lull: no combat 1:00-4:00",
            ),
            (
                TimelineKind::GameOver {
                    result: GameResult::Victory { team: 1 },
                    winner_seats: vec![1, 3],
                },
                "game over: victory team 1 (seats: 1, 3)",
            ),
        ];
        for (kind, expected) in cases {
            assert_eq!(render_moment(&kind), expected);
        }
        assert_eq!(render_result(&GameResult::Draw, &[]), "draw");
    }

    #[test]
    fn minimap_fits_bounds_and_marks_foundries_and_scrap() {
        let state = Scenario::skirmish().build().expect("skirmish builds");
        let map = minimap(&state);
        let repeat = minimap(&state);
        assert_eq!(map, repeat, "minimap must be deterministic");
        assert!(map.len() <= 16);
        assert!(map.iter().all(|row| row.chars().count() <= 46));
        let all: String = map.concat();
        assert!(all.contains('0'), "seat 0 foundry digit missing:\n{all}");
        assert!(all.contains('1'), "seat 1 foundry digit missing:\n{all}");
        assert!(all.contains('$'), "scrap marker missing:\n{all}");
    }

    #[test]
    fn digests_land_on_the_stride_plus_the_closing_tick() {
        let report = summarize(&fixture(), &opts(None, Some(5))).expect("fixture summarizes");
        assert_eq!(
            report
                .digests
                .iter()
                .map(|digest| digest.tick)
                .collect::<Vec<_>>(),
            vec![5, 10, 12]
        );
        assert_eq!(report.scenario.effective_ticks, 12);
        assert_eq!(report.digests.len(), 3);
    }

    #[test]
    fn until_truncates_without_an_unconsumed_commands_error() {
        let report = summarize(&fixture(), &opts(Some(5), Some(4))).expect("truncation is fine");
        assert_eq!(
            report
                .digests
                .iter()
                .map(|digest| digest.tick)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
        assert_eq!(report.scenario.effective_ticks, 5);
        assert_eq!(report.scenario.total_ticks, 12);
    }

    #[test]
    fn render_is_deterministic_and_carries_the_header() {
        let report = summarize(&fixture(), &opts(None, None)).expect("fixture summarizes");
        let text = report.render();
        let again = summarize(&fixture(), &opts(None, None))
            .expect("fixture summarizes")
            .render();
        assert_eq!(text, again);
        assert!(text.contains("Skirmish Basin"));
        assert!(text.contains("digest t=12"));
    }
}
