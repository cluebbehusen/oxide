//! The built-in skirmish bot.
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

use crate::command::{Command, PlayerCommand};
use crate::ids::{PlayerId, Target};
use crate::state::{Order, State};
use crate::stats::UnitKind;
use chassis::fx::Fx;
use chassis::grid::TilePos;
use chassis::rng::Pcg32;

/// How many harvesters the bot wants alive or queued.
const HARVESTER_TARGET: usize = 4;
/// Enemies inside this radius of home trigger a full defensive response.
const DEFENSE_RADIUS: Fx = Fx::lit("8");
/// While marching on a building, enemies inside this radius get priority.
const SKIRMISH_RADIUS: Fx = Fx::lit("3");
/// The bot thinks every N ticks (staggered per player so two bots never act
/// on the same tick).
const CADENCE: u64 = 8;

/// One bot, driving one player.
#[derive(Debug, Clone)]
pub struct Bot {
    player: PlayerId,
    #[expect(dead_code, reason = "reserved for future tactical variation")]
    rng: Pcg32,
    attack_threshold: usize,
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
        if state.result.is_some() || state.tick % CADENCE != u64::from(self.player.0) {
            return Vec::new();
        }
        let me = self.player;
        let mut commands = Vec::new();

        let Some(home) = state
            .buildings
            .iter()
            .filter(|b| b.player == me)
            .min_by_key(|b| b.id)
        else {
            return Vec::new(); // eliminated; nothing to do but watch
        };
        let home_center = home.center();
        let home_id = home.id;

        // Economy: idle harvesters back to work.
        for unit in state
            .units
            .iter()
            .filter(|u| u.player == me && u.kind == UnitKind::Harvester && u.order == Order::Idle)
        {
            if let Some(node) = nearest_scrap(state, unit.tile()) {
                commands.push(self.cmd(Command::Harvest {
                    units: vec![unit.id],
                    node,
                }));
            }
        }

        // Production: harvesters up to target, then a steady sentinel drip.
        let harvesters = state
            .units
            .iter()
            .filter(|u| u.player == me && u.kind == UnitKind::Harvester)
            .count()
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
            .filter(|u| u.player == me && u.kind == UnitKind::Sentinel)
            .collect();
        if let Some(intruder) = intruder {
            let defenders: Vec<_> = my_sentinels
                .iter()
                .filter(|s| {
                    s.order
                        != (Order::Attack {
                            target: Target::Unit(intruder),
                        })
                })
                .map(|s| s.id)
                .collect();
            if !defenders.is_empty() {
                commands.push(self.cmd(Command::Attack {
                    units: defenders,
                    target: Target::Unit(intruder),
                }));
            }
            return commands;
        }

        // Offense: enough idle sentinels → march on the nearest enemy
        // building (or unit, if they're homeless).
        let idle_sentinels: Vec<_> = my_sentinels
            .iter()
            .filter(|s| s.order == Order::Idle)
            .map(|s| s.id)
            .collect();
        if idle_sentinels.len() >= self.attack_threshold {
            let building_target = state
                .buildings
                .iter()
                .filter(|b| b.player != me)
                .map(|b| (home_center.dist_sq(b.center()), b.id))
                .min()
                .map(|(_, id)| Target::Building(id));
            let target = building_target.or_else(|| {
                state
                    .units
                    .iter()
                    .filter(|u| u.player != me && u.hp > 0)
                    .map(|u| (home_center.dist_sq(u.pos), u.id))
                    .min()
                    .map(|(_, id)| Target::Unit(id))
            });
            if let Some(target) = target {
                commands.push(self.cmd(Command::Attack {
                    units: idle_sentinels,
                    target,
                }));
            }
        }

        // Marching sentinels shouldn't ignore defenders shooting at them.
        for sentinel in &my_sentinels {
            if !matches!(
                sentinel.order,
                Order::Attack {
                    target: Target::Building(_)
                }
            ) {
                continue;
            }
            let nearby_enemy = state
                .units
                .iter()
                .filter(|u| u.player != me && u.hp > 0)
                .map(|u| (sentinel.pos.dist_sq(u.pos), u.id))
                .filter(|(d, _)| *d <= SKIRMISH_RADIUS * SKIRMISH_RADIUS)
                .min()
                .map(|(_, id)| id);
            if let Some(enemy) = nearby_enemy {
                commands.push(self.cmd(Command::Attack {
                    units: vec![sentinel.id],
                    target: Target::Unit(enemy),
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

/// Nearest tile holding scrap, keyed by (manhattan, y, x) for a unique pick.
fn nearest_scrap(state: &State, from: TilePos) -> Option<TilePos> {
    state
        .map
        .iter()
        .filter(|(_, tile)| tile.scrap > 0)
        .map(|(pos, _)| (pos.manhattan(from), pos.y, pos.x))
        .min()
        .map(|(_, y, x)| TilePos::new(x, y))
}
