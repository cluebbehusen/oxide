//! The legacy rule-cascade skirmish bot — kept intact as the benchmark
//! opponent every newer brain must beat, and as the
//! bot behind existing scenarios until the tiered brains land.
//!
//! Deliberately *outside* the sim's tick pipeline: a bot is just another
//! command source, reading [`State`] and emitting [`PlayerCommand`]s exactly
//! like a mouse or a debug socket would. Its commands are recorded into
//! replays like anyone else's, so replaying never needs the bot.
//!
//! It is deterministic — its only randomness is drawn at construction from
//! the scenario seed — and intentionally beatable: keep the economy going,
//! mass a squad, defend home, push. Mirror matches don't stalemate because
//! the attack threshold varies per player.
//!
//! Team-blind by design: this bot predates teams and reads allegiance
//! as `player != me`, so on a team scenario it would flag allies as
//! intruders and waste its commands on them (the sim rejects ally
//! attacks, so no friendly fire is possible — the seat just plays
//! badly). Every scenario that seats it must be a plain free-for-all;
//! team seats belong to the neural ladder via `bot_config`.

use crate::command::{Command, PlayerCommand};
use crate::ids::{PlayerId, Target};
use crate::state::{Order, State};
use crate::stats::UnitKind;
use chassis::fx::Fx;
use chassis::grid::TilePos;
use chassis::rng::Pcg32;

/// How many harvesters the bot wants alive or queued.
const HARVESTER_TARGET: usize = 4;
/// Bank level that triggers the Fabricator (cost plus a fighting reserve).
const FABRICATOR_AT: u32 = 220;
/// Most turrets the bot will pay for.
const TURRET_CAP: usize = 2;
/// Enemies inside this radius of home trigger a full defensive response.
const DEFENSE_RADIUS: Fx = Fx::lit("8");
/// The bot thinks every N ticks. All bots think on the same tick since
/// tick: commands have no cross-player coupling at application, and the
/// old per-seat stagger handed the later thinker one tick of fresher
/// information every cycle.
const CADENCE: u64 = 8;

/// One bot, driving one player.
#[derive(Debug, Clone)]
pub struct Bot {
    player: PlayerId,
    #[expect(dead_code, reason = "reserved for future tactical variation")]
    rng: Pcg32,
    attack_threshold: usize,
    /// Harvester count at the last think — a drop means someone is eating
    /// the economy, which buys a turret. Bot-local memory is legitimate:
    /// a bot is a command source, not sim state.
    harvesters_seen: usize,
    /// Set once a harvester died on this bot's watch; cleared when the
    /// turret answer has been placed.
    raided: bool,
    /// Harvest assignments from the last think: a unit idle again right
    /// after being sent means the node is unreachable — blacklist it
    /// instead of re-ordering forever.
    last_sent: Vec<(crate::ids::UnitId, TilePos)>,
    /// Nodes that bounced a harvester back.
    dead_nodes: Vec<TilePos>,
    /// Turret count at the last think; a new turret appearing is what
    /// clears [`Bot::raided`] — not the (possibly rejected) command.
    turrets_seen: usize,
}

impl Bot {
    /// Creates the bot for `player`, deriving behavior from the scenario
    /// seed (each player gets a distinct RNG stream).
    pub fn new(player: PlayerId, scenario_seed: u64) -> Self {
        let mut rng = Pcg32::new(scenario_seed, 1000 + u64::from(player.0));
        let attack_threshold = 4 + rng.next_below(3) as usize; // 4..=6
        Self {
            player,
            rng,
            attack_threshold,
            harvesters_seen: 0,
            raided: false,
            last_sent: Vec::new(),
            dead_nodes: Vec::new(),
            turrets_seen: 0,
        }
    }

    /// All bots a scenario asks for.
    pub fn for_scenario(scenario: &crate::Scenario) -> Vec<Bot> {
        scenario
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| p.bot)
            .map(|(i, _)| Bot::new(PlayerId(i as u8), scenario.seed))
            .collect()
    }

    /// The player this bot drives.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// Commands for this tick (usually none — the bot thinks on a cadence).
    pub fn act(&mut self, state: &State) -> Vec<PlayerCommand> {
        if state.result.is_some() || !state.tick.is_multiple_of(CADENCE) {
            return Vec::new();
        }
        let me = self.player;
        let mut commands = Vec::new();

        let Some(home) = state
            .buildings
            .iter()
            .filter(|b| b.player == me && b.kind == crate::stats::BuildingKind::Foundry)
            .min_by_key(|b| b.id)
        else {
            return Vec::new(); // eliminated; nothing to do but watch
        };
        let home_center = home.center();
        let home_id = home.id;

        // Retry damping: a harvester sent last think and idle again now
        // bounced off an unreachable node — never ask twice. A node it
        // honestly drained holds nothing and stays off the list (the
        // scrap filter already refuses it); blacklisting it would
        // poison the tile forever.
        for (id, node) in std::mem::take(&mut self.last_sent) {
            if state
                .unit(id)
                .is_some_and(|u| u.order == Order::Idle && u.hp > 0)
                && (state.map.scrap_at(node) > 0 || state.map.wreck_at(node) > 0)
                && !self.dead_nodes.contains(&node)
            {
                self.dead_nodes.push(node);
            }
        }

        // Economy: idle harvesters back to work.
        for unit in state
            .units
            .iter()
            .filter(|u| u.player == me && u.kind == UnitKind::Harvester && u.order == Order::Idle)
        {
            if let Some(node) = nearest_scrap(state, unit.tile(), &self.dead_nodes) {
                commands.push(self.cmd(Command::Harvest {
                    units: vec![unit.id],
                    node,
                    queue: false,
                }));
                self.last_sent.push((unit.id, node));
            }
        }

        // Census for every decision below. The bot reads full state on
        // purpose (classic cheating AI); its commands still validate.
        let mut harvesters_alive = 0;
        let (mut my_scuttlers, mut my_lancers) = (0, 0);
        let (mut enemy_harvesters, mut enemy_turrets) = (0, 0);
        for u in state.units.iter().filter(|u| u.hp > 0) {
            if u.player == me {
                match u.kind {
                    UnitKind::Harvester => harvesters_alive += 1,
                    UnitKind::Scuttler => my_scuttlers += 1,
                    UnitKind::Lancer => my_lancers += 1,
                    // The frozen legacy bot predates the wider roster; it
                    // counts only the kinds its rules ever reason about.
                    _ => {}
                }
            } else if u.kind == UnitKind::Harvester {
                enemy_harvesters += 1;
            }
        }
        let mut fabricator: Option<crate::ids::BuildingId> = None;
        let mut fabricator_pending = false;
        let mut my_turrets = 0;
        for b in state.buildings.iter() {
            if b.player == me {
                match b.kind {
                    crate::stats::BuildingKind::Fabricator => {
                        if b.built {
                            fabricator = Some(b.id);
                        } else {
                            fabricator_pending = true;
                        }
                    }
                    crate::stats::BuildingKind::Turret => my_turrets += 1,
                    _ => {}
                }
            } else if b.kind == crate::stats::BuildingKind::Turret && b.built {
                enemy_turrets += 1;
            }
        }

        // A shrinking harvest line means raiders: remember it until a
        // turret actually appears — the build command can bounce off an
        // empty bank, and clearing on emission would forget the raid.
        if harvesters_alive < self.harvesters_seen {
            self.raided = true;
        }
        self.harvesters_seen = harvesters_alive;
        if my_turrets > self.turrets_seen {
            self.raided = false;
        }
        self.turrets_seen = my_turrets;

        // Production: harvesters up to target, then a steady sentinel drip.
        let harvesters = harvesters_alive
            + state
                .buildings
                .iter()
                .filter(|b| b.player == me)
                .flat_map(|b| b.queue.iter())
                .filter(|k| **k == UnitKind::Harvester)
                .count();
        let queue_len = state
            .building(home_id)
            .map_or(usize::MAX, |b| b.queue.len());
        let bank = state.player(me).scrap;
        if queue_len < 2 {
            if harvesters < HARVESTER_TARGET && bank >= UnitKind::Harvester.stats().cost {
                commands.push(self.cmd(Command::Train {
                    building: home_id,
                    kind: UnitKind::Harvester,
                }));
            } else if bank >= UnitKind::Sentinel.stats().cost {
                commands.push(self.cmd(Command::Train {
                    building: home_id,
                    kind: UnitKind::Sentinel,
                }));
            }
        }

        // Orphaned sites get a relief builder: a dead or reassigned
        // harvester must not strand paid-for progress (Build on an
        // existing own site resumes it free of charge). Harvesters already
        // building — anywhere — are off limits, and each pick is reserved
        // within this think, or one worker would be assigned to every
        // orphan at once and oscillate between them forever.
        let mut reserved: Vec<crate::ids::UnitId> = Vec::new();
        for b in state.buildings.iter() {
            if b.player != me || b.built {
                continue;
            }
            let attended = state.units.iter().any(|u| {
                u.player == me
                    && u.hp > 0
                    && matches!(u.order, Order::Build { site } if site == b.id)
            });
            if !attended
                && let Some(builder) = nearest_free_harvester(state, me, b.anchor, &reserved)
            {
                reserved.push(builder);
                commands.push(self.cmd(Command::Build {
                    units: vec![builder],
                    kind: b.kind,
                    anchor: b.anchor,
                    queue: false,
                    defer: false,
                }));
            }
        }

        // Tech: one Fabricator, once the economy stands and the bank can
        // absorb it without starving the sentinel drip.
        if fabricator.is_none()
            && !fabricator_pending
            && harvesters_alive >= HARVESTER_TARGET.min(3)
            && bank >= FABRICATOR_AT
            && let Some(anchor) = placement_near(
                state,
                me,
                crate::stats::BuildingKind::Fabricator,
                home_center,
            )
            && let Some(builder) = nearest_free_harvester(state, me, anchor, &reserved)
        {
            reserved.push(builder);
            commands.push(self.cmd(Command::Build {
                units: vec![builder],
                kind: crate::stats::BuildingKind::Fabricator,
                anchor,
                queue: false,
                defer: false,
            }));
        }

        // A raid buys a turret over the harvest line (up to the cap).
        if self.raided
            && my_turrets < TURRET_CAP
            && bank >= 150
            && let Some(node) =
                nearest_scrap(state, TilePos::containing(home_center), &self.dead_nodes)
            && let Some(anchor) =
                placement_near(state, me, crate::stats::BuildingKind::Turret, node.center())
            && let Some(builder) = nearest_free_harvester(state, me, anchor, &reserved)
        {
            reserved.push(builder);
            commands.push(self.cmd(Command::Build {
                units: vec![builder],
                kind: crate::stats::BuildingKind::Turret,
                anchor,
                queue: false,
                defer: false,
            }));
        }

        // Advanced roster from the Fabricator: lancers to crack turtles,
        // scuttlers to eat exposed harvest lines.
        if let Some(fab) = fabricator
            && state.building(fab).is_some_and(|b| b.queue.len() < 2)
        {
            if enemy_turrets > my_lancers && bank >= UnitKind::Lancer.stats().cost {
                commands.push(self.cmd(Command::Train {
                    building: fab,
                    kind: UnitKind::Lancer,
                }));
            } else if my_scuttlers < 4
                && enemy_harvesters >= 2
                && bank >= UnitKind::Scuttler.stats().cost + UnitKind::Sentinel.stats().cost
            {
                commands.push(self.cmd(Command::Train {
                    building: fab,
                    kind: UnitKind::Scuttler,
                }));
            } else if bank >= UnitKind::Lancer.stats().cost + UnitKind::Sentinel.stats().cost {
                commands.push(self.cmd(Command::Train {
                    building: fab,
                    kind: UnitKind::Lancer,
                }));
            }
        }

        // Defense trumps everything: an enemy near home pulls every sentinel.
        let intruder = state
            .units
            .iter()
            .filter(|u| u.player != me && u.hp > 0)
            .map(|u| (home_center.dist_sq(u.pos), u.id))
            .filter(|(d, _)| *d <= DEFENSE_RADIUS * DEFENSE_RADIUS)
            .min()
            .map(|(_, id)| id);
        let my_sentinels: Vec<_> = state
            .units
            .iter()
            .filter(|u| {
                u.player == me && u.kind != UnitKind::Harvester && u.kind.stats().can_fight()
            })
            .collect();
        if let Some(intruder) = intruder {
            let defenders: Vec<_> = my_sentinels
                .iter()
                .filter(|s| {
                    s.order
                        != (Order::Attack {
                            target: Target::Unit(intruder),
                            resume: None,
                        })
                })
                .map(|s| s.id)
                .collect();
            if !defenders.is_empty() {
                commands.push(self.cmd(Command::Attack {
                    units: defenders,
                    target: Target::Unit(intruder),
                    queue: false,
                }));
            }
            return commands;
        }

        // Offense: enough idle fighters → attack-move at the nearest enemy
        // building (or their units, if they're homeless). Attack-move does
        // the fighting-on-the-way; no hand-holding needed.
        let idle_sentinels: Vec<_> = my_sentinels
            .iter()
            .filter(|s| s.order == Order::Idle)
            .map(|s| s.id)
            .collect();
        if idle_sentinels.len() >= self.attack_threshold {
            let march_goal = state
                .buildings
                .iter()
                .filter(|b| b.player != me)
                .map(|b| (home_center.dist_sq(b.center()), b.id, b.anchor))
                .min()
                .map(|(_, _, anchor)| anchor)
                .or_else(|| {
                    state
                        .units
                        .iter()
                        .filter(|u| u.player != me && u.hp > 0)
                        .map(|u| (home_center.dist_sq(u.pos), u.id, u.tile()))
                        .min()
                        .map(|(_, _, tile)| tile)
                });
            if let Some(goal) = march_goal {
                commands.push(self.cmd(Command::AttackMove {
                    units: idle_sentinels,
                    goal,
                    queue: false,
                }));
            }
        }
        commands
    }

    fn cmd(&self, command: Command) -> PlayerCommand {
        PlayerCommand {
            player: self.player,
            command,
        }
    }
}

/// First legal anchor for `kind` ring-scanned outward from `near` (3..=7
/// tiles) — deterministic, and honest: `can_place` applies the same rules
/// players get, including own-fog exploration.
fn placement_near(
    state: &State,
    me: PlayerId,
    kind: crate::stats::BuildingKind,
    near: chassis::fx::Vec2Fx,
) -> Option<TilePos> {
    let center = TilePos::containing(near);
    for r in 3i32..=7 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let anchor = center.offset(dx, dy);
                if state.can_place(me, kind, anchor) {
                    return Some(anchor);
                }
            }
        }
    }
    None
}

/// The closest own harvester to `anchor` that isn't already building
/// somewhere or reserved this think, ties to the lowest id. Pulling a
/// mining one is fine — it goes idle after the build and the economy loop
/// re-hires it; pulling an active *builder* is not, that's how sites end
/// up abandoned.
fn nearest_free_harvester(
    state: &State,
    me: PlayerId,
    anchor: TilePos,
    reserved: &[crate::ids::UnitId],
) -> Option<crate::ids::UnitId> {
    state
        .units
        .iter()
        .filter(|u| {
            u.player == me
                && u.kind == UnitKind::Harvester
                && u.hp > 0
                && !matches!(u.order, Order::Build { .. })
                && !reserved.contains(&u.id)
        })
        .map(|u| (u.pos.dist_sq(anchor.center()), u.id))
        .min()
        .map(|(_, id)| id)
}

/// Nearest tile holding scrap, keyed by (manhattan, y, x) for a unique
/// pick; `avoid` lists nodes that already bounced a harvester.
fn nearest_scrap(state: &State, from: TilePos, avoid: &[TilePos]) -> Option<TilePos> {
    state
        .map
        .iter()
        .filter(|(pos, tile)| tile.scrap > 0 && !avoid.contains(pos))
        .map(|(pos, _)| (pos.manhattan(from), pos.y, pos.x))
        .min()
        .map(|(_, y, x)| TilePos::new(x, y))
}
