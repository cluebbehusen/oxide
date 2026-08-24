//! Army lifecycle, marching, contact, and strength assessment.

use super::*;

const PULLBACK_NUM: u32 = 35;
const PULLBACK_DEN: u32 = 100;

/// Withdraw only from catastrophe: below half the local enemy strength.
/// Nothing in this world outruns its pursuers, so a merely-losing fight
/// finished on the spot costs less than a rout — disengaging under fire
/// is free damage handed to the enemy.
const WITHDRAW_MARGIN_NUM: u32 = 1;
const WITHDRAW_MARGIN_DEN: u32 = 2;
/// A marching or withdrawing army that has not bettered its best
/// distance to its goal for this long is wedged — usually ordered
/// across terrain with no route — and re-stages where it stands.
/// Staging reopens the trained-legal verbs (Scout for staged members,
/// Push, reinforcement), so the operations head never goes dark
/// behind an unroutable order. Matches the recovery patience scale.
pub(super) const ARMY_PROGRESS_PATIENCE_TICKS: u64 = 1_200;

/// Radius (tiles) around the army centroid scored as "the fight".
const ENGAGE_RADIUS: i32 = 8;
/// A pushing army is engaged once enemies are inside this radius.
const CONTACT_RADIUS: i32 = 6;

impl Executive {
    /// Prune the dead, rotate the wounded to the rear, advance army
    /// states, and withdraw from fights that have turned. `rear` is
    /// behind the lines, not the army's rally (which may be the fight).
    pub fn maintain(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
    ) -> Vec<PlayerCommand> {
        let mut out = Vec::new();
        let alive = |id: UnitId| obs.my_units.iter().any(|u| u.id == id);
        self.rear.retain(|id| alive(*id));
        for army in &mut self.armies {
            army.members.retain(|id| alive(*id));
            if army.members.is_empty() {
                continue; // swept below
            }
            let in_contact = enemies_near(obs, &army.members, CONTACT_RADIUS);

            // Rotate the badly wounded out, but only between fights.
            // Mid-engagement a wounded machine still deals full damage,
            // and at equal speeds it cannot escape a pursuer anyway;
            // pulling it then just thins the line.
            if !in_contact {
                let mut pulled: Vec<UnitId> = Vec::new();
                army.members.retain(|id| {
                    let Some(u) = obs.my_units.iter().find(|u| u.id == *id) else {
                        return false;
                    };
                    let max = u.kind.stats().max_hp;
                    if u.hp * PULLBACK_DEN < max * PULLBACK_NUM {
                        pulled.push(*id);
                        false
                    } else {
                        true
                    }
                });
                if !pulled.is_empty() {
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Move {
                            units: pulled.clone(),
                            goal: rear,
                            queue: false,
                        },
                    });
                    self.rear.extend(pulled);
                }
            }
            if army.members.is_empty() {
                continue; // swept below
            }

            let centroid = centroid(&army.members, obs);
            match army.state {
                ArmyState::Staging => {
                    // A staged army can be attacked where it stands — the
                    // fight evaluation must not wait for a push order.
                    if in_contact {
                        army.state = ArmyState::Engaging;
                    }
                }
                ArmyState::Pushing => {
                    let vanguard = vanguard_centroid(&army.members, obs);
                    // Judged on the escorts, the units the march order
                    // actually names: artillery takes a separate routable
                    // side-move to staging, and counting it kept a refused
                    // march from ever reading as idle.
                    let all_idle = obs
                        .my_units
                        .iter()
                        .filter(|u| army.members.contains(&u.id) && !is_artillery(u))
                        .all(|u| u.idle);
                    // A march order the sim refused leaves every member
                    // idle exactly where it stood. Checked only on a LATER
                    // think than the order, since this think's commands
                    // have not executed yet.
                    let bounced = all_idle
                        && army.issued.is_some_and(|(at, from)| {
                            obs.tick > at && vanguard.chebyshev(from) <= 1
                        });
                    if in_contact {
                        army.state = ArmyState::Engaging;
                        army.progress = None;
                        army.issued = None;
                        army.bounces = 0;
                    } else if let Some(target) = army.target
                        && tiles_within(vanguard, target, 2)
                    {
                        // Arrived and nothing to fight: hold the ground
                        // taken — this rally is the staging point now.
                        army.state = ArmyState::Staging;
                        army.staging = target;
                        army.target = None;
                        army.progress = None;
                        army.issued = None;
                        army.bounces = 0;
                    } else if bounced {
                        // Two refused orders in a row are route testimony
                        // on the first think a wedge clock would only begin
                        // counting — an order refused at issue never
                        // marches, so it never stalls. Two immediate
                        // bounces are enough to stop repeating it.
                        army.issued = None;
                        army.bounces = army.bounces.saturating_add(1);
                        if army.bounces >= 2 && army.target.is_some() {
                            army.state = ArmyState::Staging;
                            army.staging = centroid;
                            army.target = None;
                            army.progress = None;
                            army.bounces = 0;
                        }
                    } else if let Some(target) = army.target
                        && wedged(&mut army.progress, vanguard.chebyshev(target), obs.tick)
                    {
                        // The march has not gained a tile in the whole
                        // patience window — usually an order across
                        // terrain with no route. Rally where it stands
                        // so the seat's verbs come back.
                        army.state = ArmyState::Staging;
                        army.staging = centroid;
                        army.target = None;
                        army.progress = None;
                        army.issued = None;
                        army.bounces = 0;
                    }
                }
                ArmyState::Engaging => {
                    let (mine, theirs) = local_strength(obs, &army.members);
                    if theirs == 0 {
                        // Fight's over here; march on if a target remains.
                        army.state = match army.target {
                            Some(_) => ArmyState::Pushing,
                            None => ArmyState::Staging,
                        };
                        army.focus = None;
                        army.progress = None;
                        if let Some(target) = army.target {
                            march(me, obs, army, target, &mut out);
                        }
                    } else if mine * u64::from(WITHDRAW_MARGIN_DEN)
                        < theirs * u64::from(WITHDRAW_MARGIN_NUM)
                    {
                        // Losing decisively: leave together, fighting.
                        // Nothing here outruns its pursuers, so an
                        // oblivious Move retreat is shot in the back for
                        // free the whole way home — the attack-move falls
                        // back along the same line but answers fire.
                        army.state = ArmyState::Withdrawing;
                        army.target = None;
                        army.focus = None;
                        army.progress = None;
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: army.members.clone(),
                                goal: army.staging,
                                queue: false,
                            },
                        });
                    } else {
                        // Concentrate fire: everyone on the weakest gun
                        // in the fight (ties toward the centroid, then
                        // id). Candidates stay inside contact radius so
                        // the sim's see-the-victim rule holds even for
                        // an omniscient policy. One command per change
                        // of focus — auto-acquire covers the seconds in
                        // between; churning orders every think costs
                        // shots.
                        let members: Vec<&UnitObs> = obs
                            .my_units
                            .iter()
                            .filter(|u| army.members.contains(&u.id))
                            .collect();
                        let near = |t: TilePos| {
                            members
                                .iter()
                                .map(|m| m.tile.chebyshev(t))
                                .min()
                                .unwrap_or(i32::MAX)
                        };
                        let focus = obs
                            .enemy_units
                            .iter()
                            .filter(|u| {
                                near(u.tile) <= CONTACT_RADIUS && u.kind.stats().can_fight()
                            })
                            .map(|u| (u.hp, near(u.tile), u.id))
                            .min()
                            .map(|(.., id)| id);
                        if let Some(target) = focus
                            && army.focus != Some(target)
                        {
                            army.focus = Some(target);
                            out.push(PlayerCommand {
                                player: me,
                                command: Command::Attack {
                                    units: army.members.clone(),
                                    target: crate::ids::Target::Unit(target),
                                    queue: false,
                                },
                            });
                        }
                    }
                }
                ArmyState::Withdrawing => {
                    if tiles_within(centroid, army.staging, 2) {
                        army.state = ArmyState::Staging;
                        army.progress = None;
                    } else if wedged(
                        &mut army.progress,
                        centroid.chebyshev(army.staging),
                        obs.tick,
                    ) {
                        // The way home is as unroutable as the way out.
                        // Rally here; Recall and Push become meaningful
                        // again instead of both being illegal forever.
                        army.state = ArmyState::Staging;
                        army.staging = centroid;
                        army.progress = None;
                    }
                }
            }
        }
        self.armies.retain(|a| !a.members.is_empty());
        out
    }
}

/// A long gun: ordered reach beyond its own eyes. It fires on the
/// team's sight, so it must never lead the march into what it cannot
/// see.
pub(super) fn is_artillery(u: &UnitObs) -> bool {
    let stats = u.kind.stats();
    stats
        .max_range_vs(crate::stats::Domain::Ground)
        .is_some_and(|r| r > chassis::fx::Fx::from_num(stats.vision))
}

/// How far short of the push target artillery parks — inside its own
/// reach of the target, outside a defending turret's.
const ARTY_STANDOFF: i32 = 7;

/// Marching orders for a push: escorts attack-move onto the target;
/// artillery holds a standoff point pulled back along the line of
/// advance — and without an escort quorum (a third of the army) the
/// guns stay at the staging ground instead. Nobody pushes blind
/// artillery.
pub(super) fn march(
    me: PlayerId,
    obs: &Observation,
    army: &Army,
    target: TilePos,
    out: &mut Vec<PlayerCommand>,
) {
    let (arty, escorts): (Vec<UnitId>, Vec<UnitId>) = army
        .members
        .iter()
        .partition(|id| obs.my_units.iter().any(|u| u.id == **id && is_artillery(u)));
    if !escorts.is_empty() {
        out.push(PlayerCommand {
            player: me,
            command: Command::AttackMove {
                units: escorts.clone(),
                goal: target,
                queue: false,
            },
        });
    }
    if arty.is_empty() {
        return;
    }
    if escorts.len() * 3 >= army.members.len() {
        let (dx, dy) = (army.staging.x - target.x, army.staging.y - target.y);
        let d = dx.abs().max(dy.abs());
        let stand = if d == 0 {
            target
        } else {
            let pull = ARTY_STANDOFF.min(d);
            TilePos::new(target.x + dx * pull / d, target.y + dy * pull / d)
        };
        out.push(PlayerCommand {
            player: me,
            command: Command::AttackMove {
                units: arty,
                goal: stand,
                queue: false,
            },
        });
    } else {
        out.push(PlayerCommand {
            player: me,
            command: Command::Move {
                units: arty,
                goal: army.staging,
                queue: false,
            },
        });
    }
}

/// The escorts' mean tile — artillery hanging back must not drag the
/// army's sense of "arrived" backward with it. Falls back to the whole
/// body for a pure-artillery force.
pub(super) fn vanguard_centroid(members: &[UnitId], obs: &Observation) -> TilePos {
    let escorts: Vec<UnitId> = members
        .iter()
        .copied()
        .filter(|id| obs.my_units.iter().any(|u| u.id == *id && !is_artillery(u)))
        .collect();
    if escorts.is_empty() {
        centroid(members, obs)
    } else {
        centroid(&escorts, obs)
    }
}

/// Mean member tile (integer division — a macro-scale center).
/// Advance a march's wedge clock: records a strictly better distance,
/// and reports true once the best has stood unimproved for the whole
/// patience window.
pub(super) fn wedged(progress: &mut Option<(i32, u64)>, distance: i32, tick: u64) -> bool {
    match progress {
        Some((best, _)) if distance < *best => {
            *progress = Some((distance, tick));
            false
        }
        Some((_, since)) => tick.saturating_sub(*since) >= ARMY_PROGRESS_PATIENCE_TICKS,
        None => {
            *progress = Some((distance, tick));
            false
        }
    }
}

fn centroid(members: &[UnitId], obs: &Observation) -> TilePos {
    let mut n = 0i32;
    let (mut sx, mut sy) = (0i64, 0i64);
    for u in obs.my_units.iter().filter(|u| members.contains(&u.id)) {
        sx += i64::from(u.tile.x);
        sy += i64::from(u.tile.y);
        n += 1;
    }
    if n == 0 {
        TilePos::new(0, 0)
    } else {
        TilePos::new((sx / i64::from(n)) as i32, (sy / i64::from(n)) as i32)
    }
}

/// Whether an army counts as fighting: a third of it (at least one
/// member) has an armed enemy inside `radius`. A lone straggler brushing
/// past an enemy is not the army's fight — quorum keeps the state
/// machine from being yanked around by grazing contact.
fn enemies_near(obs: &Observation, members: &[UnitId], radius: i32) -> bool {
    let touched = obs
        .my_units
        .iter()
        .filter(|u| members.contains(&u.id))
        .filter(|m| {
            obs.enemy_units
                .iter()
                .any(|e| mutually_relevant(m, e) && m.tile.chebyshev(e.tile) <= radius)
        })
        .count();
    touched > 0 && touched * 3 >= members.len()
}
/// Strength sums for an army's fight: every member counts (the army is
/// the fighting body wherever its parts stand), and the opposition is
/// every enemy within the engagement radius of a member that is itself
/// in contact. Anchoring on fighting members instead of a centroid keeps
/// the estimate stable when the line bends — a mean position can land in
/// empty ground and blind every radius test around it — while stragglers
/// don't sweep distant enemies into the count.
fn local_strength(obs: &Observation, members: &[UnitId]) -> (u64, u64) {
    use crate::stats::Domain;
    let mine_units: Vec<&UnitObs> = obs
        .my_units
        .iter()
        .filter(|u| members.contains(&u.id))
        .collect();
    let engaged: Vec<TilePos> = mine_units
        .iter()
        .filter(|m| {
            obs.enemy_units
                .iter()
                .any(|e| mutually_relevant(m, e) && m.tile.chebyshev(e.tile) <= CONTACT_RADIUS)
        })
        .map(|m| m.tile)
        .collect();
    let opposition: Vec<&UnitObs> = obs
        .enemy_units
        .iter()
        .filter(|e| engaged.iter().any(|m| m.chebyshev(e.tile) <= ENGAGE_RADIUS))
        .collect();
    // Matched pairs: each side is worth what it can actually apply to
    // the domains the other side fields. An interceptor over a pure
    // ground brawl contributes nothing to either column.
    let domains_of = |units: &[&UnitObs]| {
        let ground = units
            .iter()
            .any(|u| u.kind.stats().domain == Domain::Ground);
        let air = units.iter().any(|u| u.kind.stats().domain == Domain::Air);
        (ground, air)
    };
    let (their_ground, their_air) = domains_of(&opposition);
    let (my_ground, my_air) = domains_of(&mine_units);
    let applicable = |u: &UnitObs, ground: bool, air: bool| -> u64 {
        let g = if ground {
            strength_vs(u, Domain::Ground)
        } else {
            0
        };
        let a = if air { strength_vs(u, Domain::Air) } else { 0 };
        g.max(a)
    };
    let mine: u64 = mine_units
        .iter()
        .map(|u| applicable(u, their_ground, their_air))
        .sum();
    let theirs: u64 = opposition
        .iter()
        .map(|u| applicable(u, my_ground, my_air))
        .sum();
    (mine, theirs)
}

fn tiles_within(a: TilePos, b: TilePos, radius: i32) -> bool {
    a.chebyshev(b) <= radius
}
