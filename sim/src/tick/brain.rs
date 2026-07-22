//! Phase 3: unit brains — intent becomes action.
//!
//! Units decide strictly in id order, but damage is *buffered*: every shot
//! this tick is recorded and applied only after all brains (and turrets)
//! have acted, so everyone decides against the same start-of-tick world.
//! Two machines can kill each other in the same tick — that's the point:
//! before 0.6, inline damage gave whichever seat held the higher unit ids
//! a same-tick reaction edge that decided every mirror match. Every
//! selection a brain makes (targets, doorstep tiles, replacement nodes) is
//! ordered by an explicit key ending in an id or a position, so there is
//! exactly one possible choice.

use super::{rect_adjacent_tiles, route_for, tile_adjacent_to_rect};
use crate::event::{Event, StallReason};
use crate::ids::{PlayerId, Target, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::{Domain, RETARGET_RADIUS, WeaponStats};
use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;

/// A shot decided this tick, applied after every brain has acted.
struct PendingHit {
    attacker: Target,
    victim: Target,
    damage: u32,
}

/// A buffered hp gain — construction progress or repair welding —
/// applied *after* damage: the documented rule is that a building zeroed
/// by fire is dead even if its crew acted the same tick — the shooter
/// aimed at the start-of-tick world, where the hit was lethal. Completion
/// buffers too: a site whose final tick coincides with a lethal volley
/// must never come online — no free turret shot, no "online" fanfare
/// before death.
struct PendingHpGain {
    site: crate::ids::BuildingId,
    step: u32,
    max_hp: u32,
    completes: bool,
    player: crate::ids::PlayerId,
    kind: crate::stats::BuildingKind,
}

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    let mut hits: Vec<PendingHit> = Vec::new();
    let mut builds: Vec<PendingHpGain> = Vec::new();
    let mut launches: Vec<crate::state::Shell> = Vec::new();
    // Alternate direction by tick parity: sequential phases must not hand
    // one seat a standing first-mover edge (with damage buffered, the
    // remaining coupling is small — shared scrap, own-side order state —
    // but in a zero-noise mirror match, any fixed order decides).
    let mut ids: Vec<UnitId> = state.units.iter().map(|u| u.id).collect();
    if state.tick % 2 == 1 {
        ids.reverse();
    }
    for id in ids {
        let Some(unit) = state.unit(id) else { continue };
        if unit.hp == 0 {
            continue; // dead since a previous tick but not yet swept
        }
        if let Some(unit) = state.unit_mut(id) {
            for cd in &mut unit.cooldowns {
                *cd = cd.saturating_sub(1);
            }
        }
        let order = state.unit(id).expect("just seen").order;
        match order {
            Order::Idle => idle(state, id),
            Order::Move { goal } => walk(state, id, goal, events),
            Order::Harvest { node } => harvest(state, id, node, events),
            Order::Attack { target, resume } => {
                attack(state, id, target, resume, events, &mut hits, &mut launches)
            }
            Order::AttackMove { goal } => attack_move(state, id, goal, events),
            Order::Build { site } => build(state, id, site, events, &mut builds),
            Order::Repair { building } => repair(state, id, building, events, &mut builds),
        }
    }
    turret_fire(state, events, &mut hits, &mut launches);
    // Arrivals join this tick's volley; launches land on later ticks
    // (flight is at least one tick), so ordering here cannot matter.
    land_shells(state, &mut hits, events);
    state.shells.extend(launches);
    resolve_hits(state, hits, builds, events);
}

/// The other half of simultaneity: buffered shots land now, in the order
/// they were decided (unit-id order, then turret-id order). Damage first —
/// all of it — then retaliation, so a machine that died this tick answers
/// nothing and a survivor answers its earliest attacker *that survived
/// resolution*: turning to face a corpse would waste the answer and let a
/// living shooter keep firing unopposed.
fn resolve_hits(
    state: &mut State,
    hits: Vec<PendingHit>,
    builds: Vec<PendingHpGain>,
    events: &mut Vec<Event>,
) {
    for hit in &hits {
        match hit.victim {
            Target::Unit(uid) => {
                if let Some(v) = state.unit_mut(uid) {
                    v.hp = v.hp.saturating_sub(hit.damage);
                }
            }
            Target::Building(bid) => {
                if let Some(b) = state.building_mut(bid) {
                    b.hp = b.hp.saturating_sub(hit.damage);
                }
            }
        }
    }
    // Construction gains — and completions — land only on sites that
    // survived the volley.
    for gain in &builds {
        if let Some(b) = state.building_mut(gain.site)
            && b.hp > 0
        {
            b.hp = (b.hp + gain.step).min(gain.max_hp);
            if gain.completes {
                b.built = true;
                b.progress = 0;
                events.push(Event::BuildingCompleted {
                    building: gain.site,
                    player: gain.player,
                    kind: gain.kind,
                });
            }
        }
    }
    for hit in &hits {
        if let Target::Unit(uid) = hit.victim
            && target_standing(state, hit.attacker)
        {
            retaliate(state, uid, hit.attacker);
        }
    }
}

/// The movement domain a target occupies (buildings sit on the ground).
fn target_domain(state: &State, target: Target) -> Domain {
    match target {
        Target::Unit(uid) => state
            .unit(uid)
            .map_or(Domain::Ground, |u| u.kind.stats().domain),
        Target::Building(_) => Domain::Ground,
    }
}

/// Whether full terrain cover applies to a shot: only direct fire between
/// two ground parties traces rock and buildings — rock reaches nobody in
/// the air, and indirect shells arc over it. Peaks are checked on every
/// shot regardless (see the `shot_open` closures): a mountain outreaches
/// any arc.
fn traces_terrain(weapon: &WeaponStats, shooter: Domain, victim: Domain) -> bool {
    !weapon.indirect && shooter == Domain::Ground && victim == Domain::Ground
}

/// Buffers a shot: the direct hit, plus — for splash weapons — one hit on
/// every other hostile unit inside the radius that the weapon can cover.
/// Victims are chosen against the start-of-tick world like every other
/// decision this phase makes; buildings only ever take the direct hit.
///
/// Splash deliberately skips the owner-sight fire gate the aimed paths
/// enforce: the gate governs *choosing* a victim, and a shell in flight
/// chooses nothing — whatever stands in the blast is hit, seen or not.
/// No information leaks through the hole: the only emitted event names
/// the aimed victim, and retaliation stays gated on the sufferer seeing
/// the shooter, so an unseen bystander takes damage silently and nobody
/// learns anything they could not already see.
/// Launches a real projectile at the victim's fire-time position:
/// unguided from this instant, arriving after a distance-proportional
/// flight, resolving against whatever stands there then. Returns the
/// flight length for the launch event.
fn launch_shell(
    state: &State,
    launches: &mut Vec<crate::state::Shell>,
    attacker: Target,
    attacker_owner: PlayerId,
    from: Vec2Fx,
    aim: Vec2Fx,
    weapon: &WeaponStats,
) -> u64 {
    let dist = from.dist(aim);
    let flight = (dist / crate::stats::SHELL_SPEED)
        .ceil()
        .to_num::<u64>()
        .max(1);
    launches.push(crate::state::Shell {
        shooter: attacker,
        player: attacker_owner,
        launch: from,
        impact: aim,
        arrival: state.tick + flight,
        damage: weapon.damage,
        targets: weapon.targets,
        splash: weapon.splash,
    });
    flight
}

/// Arrived shells join this tick's volley, computed against the same
/// start-of-tick world every buffered shot uses. The direct hit lands
/// on the hostile building whose footprint covers the impact tile
/// (buildings cannot dodge — sieges are preserved); units take splash
/// only, which is the standing splash rule. No fire gate here: the
/// gate cleared at launch, and a shell in flight chooses nothing.
fn land_shells(state: &mut State, hits: &mut Vec<PendingHit>, events: &mut Vec<Event>) {
    let now = state.tick;
    let mut due = Vec::new();
    state.shells.retain(|shell| {
        if shell.arrival <= now {
            due.push(shell.clone());
            false
        } else {
            true
        }
    });
    for shell in due {
        events.push(Event::ShellLanded {
            at: shell.impact,
            splash: shell.splash,
        });
        // The direct hit is distance-zero to a footprint, not tile
        // containment: a shell aimed at a building lands on the
        // footprint's closest EDGE point, whose exact coordinate floors
        // into the neighboring tile — containment alone made sieges
        // deal nothing. First hostile footprint touching the impact
        // (id order) takes the hit.
        let direct = state.buildings.iter().find(|b| {
            b.hp > 0
                && state.hostile(shell.player, b.player)
                && b.closest_point_to(shell.impact).dist_sq(shell.impact)
                    <= chassis::fx::Fx::lit("0.0001")
        });
        if let Some(b) = direct {
            hits.push(PendingHit {
                attacker: shell.shooter,
                victim: Target::Building(b.id),
                damage: shell.damage,
            });
        }
        let Some(radius) = shell.splash else { continue };
        let radius_sq = radius * radius;
        for u in state.units.iter() {
            if u.hp == 0
                || !state.hostile(shell.player, u.player)
                || !shell.targets.covers(u.kind.stats().domain)
                || u.pos.dist_sq(shell.impact) > radius_sq
            {
                continue;
            }
            hits.push(PendingHit {
                attacker: shell.shooter,
                victim: Target::Unit(u.id),
                damage: shell.damage,
            });
        }
    }
}

fn buffer_shot(
    state: &State,
    attacker: Target,
    attacker_owner: PlayerId,
    victim: Target,
    aim: Vec2Fx,
    weapon: &WeaponStats,
    hits: &mut Vec<PendingHit>,
) {
    hits.push(PendingHit {
        attacker,
        victim,
        damage: weapon.damage,
    });
    let Some(radius) = weapon.splash else { return };
    let radius_sq = radius * radius;
    for u in state.units.iter() {
        if u.hp == 0
            || !state.hostile(attacker_owner, u.player)
            || Target::Unit(u.id) == victim
            || !weapon.targets.covers(u.kind.stats().domain)
            || u.pos.dist_sq(aim) > radius_sq
        {
            continue;
        }
        hits.push(PendingHit {
            attacker,
            victim: Target::Unit(u.id),
            damage: weapon.damage,
        });
    }
}

/// Built turrets pick their own fights: nearest enemy unit in range with a
/// clear line (buildings can't chase, so out-of-line targets are simply
/// ignored until they move). Stateless — target choice re-evaluates every
/// shot, in building-id order.
fn turret_fire(
    state: &mut State,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let ids: Vec<crate::ids::BuildingId> = state.buildings.iter().map(|b| b.id).collect();
    for id in ids {
        let Some(b) = state.building(id) else {
            continue;
        };
        let Some(atk) = b.kind.stats().weapons.first() else {
            continue;
        };
        if !b.built || b.hp == 0 {
            continue;
        }
        let (me, center, cooling, kind) = (b.player, b.center(), b.cooldown > 0, b.kind);
        if cooling {
            let b = state.building_mut(id).expect("just seen");
            b.cooldown -= 1;
            if b.cooldown > 0 {
                continue;
            }
            // Reached zero this tick: fire now, like unit cooldowns do.
        }
        let range_sq = atk.range * atk.range;
        let shot_open = |t: TilePos, full: bool| {
            let Some(tile) = state.map.tile(t) else {
                return false;
            };
            if tile.terrain == crate::map::Terrain::Peak {
                return false;
            }
            !full
                || (tile.terrain == crate::map::Terrain::Ground
                    && state.building_at(t).is_none_or(|other| other.id == id))
        };
        // The owner must see the victim's tile — a turret that outranges
        // its own mast fires on a spotter's eyes, never into fog.
        let victim = state
            .units
            .iter()
            .filter(|u| {
                state.hostile(me, u.player) && u.hp > 0 && atk.targets.covers(u.kind.stats().domain)
            })
            .filter(|u| state.can_see(me, u.tile()))
            .map(|u| (center.dist_sq(u.pos), u.id, u.pos, u.kind.stats().domain))
            .filter(|(d, _, _, _)| *d <= range_sq)
            .filter(|(_, _, pos, dom)| {
                let full = traces_terrain(atk, Domain::Ground, *dom);
                !chassis::path::line_blocked(center, *pos, |t| shot_open(t, full))
            })
            .min_by_key(|&(d, uid, _, _)| (d, uid));
        let Some((_, uid, upos, _)) = victim else {
            continue;
        };
        let b = state.building_mut(id).expect("just seen");
        b.cooldown = atk.cooldown_ticks;
        if atk.projectile {
            let flight = launch_shell(state, launches, Target::Building(id), me, center, upos, atk);
            events.push(Event::ShellLaunched {
                player: me,
                from: center,
                to: upos,
                flight,
            });
        } else {
            buffer_shot(
                state,
                Target::Building(id),
                me,
                Target::Unit(uid),
                upos,
                atk,
                hits,
            );
            events.push(Event::TurretFired {
                turret: id,
                kind,
                target: uid,
                turret_pos: center,
                target_pos: upos,
            });
        }
    }
}

/// Stand up an own unfinished site: walk adjacent, then feed it progress.
/// One built tick raises hp along a linear ramp to full at completion
/// (damage taken meanwhile is simply kept — nobody rebuilds for free).
fn build(
    state: &mut State,
    id: UnitId,
    site: crate::ids::BuildingId,
    events: &mut Vec<Event>,
    builds: &mut Vec<PendingHpGain>,
) {
    let me = state.unit(id).expect("caller checked").player;
    // hp > 0 is defense in depth: with buffered damage nothing dies
    // mid-brains anymore, but building on a corpse would resurrect it and
    // swallow the destruction event, so the guard stays.
    let Some(b) = state
        .building(site)
        .filter(|b| b.player == me && !b.built && b.hp > 0)
    else {
        // Finished, cancelled, or destroyed: the job is over either way.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (anchor, kind) = (b.anchor, b.kind);
    let stats = kind.stats();
    let size = stats.size;
    let build_ticks = stats
        .construction
        .expect("sites only exist for buildable kinds")
        .build_ticks;
    let tile = state.unit(id).expect("caller checked").tile();
    if tile_adjacent_to_rect(tile, anchor, size) {
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        let b = state.building_mut(site).expect("just seen");
        let step = (ramp * (b.progress + 1) / build_ticks) - (ramp * b.progress / build_ticks);
        b.progress += 1;
        // Both the hp gain and the completion are buffered and applied
        // after damage — see PendingHpGain. The builder learns the site is
        // done next tick, through the built-site branch above.
        let completes = b.progress >= build_ticks;
        if step > 0 || completes {
            builds.push(PendingHpGain {
                site,
                step,
                max_hp: stats.max_hp,
                completes,
                player: me,
                kind,
            });
        }
        state.unit_mut(id).expect("caller checked").path = None;
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// Weld a damaged own built building: walk adjacent, then feed it hp
/// along the same ramp construction climbs, paying one scrap per
/// [`crate::stats::REPAIR_TICKS_PER_SCRAP`] ticks of torch time. Gains
/// buffer like construction — fire wins ties — and an empty bank stalls
/// the job. Several welders stack, each burning their own scrap ticks.
fn repair(
    state: &mut State,
    id: UnitId,
    building: crate::ids::BuildingId,
    events: &mut Vec<Event>,
    builds: &mut Vec<PendingHpGain>,
) {
    let me = state.unit(id).expect("caller checked").player;
    let Some(b) = state
        .building(building)
        .filter(|b| b.player == me && b.built && b.hp > 0 && b.hp < b.kind.stats().max_hp)
    else {
        // Healed, destroyed, or never a patient: the job is over.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (anchor, kind) = (b.anchor, b.kind);
    let stats = kind.stats();
    let size = stats.size;
    // The welding rate is the construction ramp; the unbuyable Foundry
    // repairs on an authored ramp of its own.
    let ramp_ticks = stats
        .construction
        .map_or(crate::stats::FOUNDRY_REPAIR_TICKS, |c| c.build_ticks);
    let tile = state.unit(id).expect("caller checked").tile();
    if tile_adjacent_to_rect(tile, anchor, size) {
        // The bank is consulted only at billing boundaries: the coin
        // paid at an interval's start has prepaid the whole interval,
        // so the torch burns it to the end before broke can stall it —
        // and a weld shorter than an interval still pays (chip repairs
        // were free when billing landed at the interval's close).
        let p = state.unit(id).expect("caller checked").progress;
        if p.is_multiple_of(crate::stats::REPAIR_TICKS_PER_SCRAP) {
            if state.player(me).scrap == 0 {
                // Broke stalls the torch.
                let unit = state.unit_mut(id).expect("caller checked");
                let (player, pos) = (unit.player, unit.pos);
                unit.clear_program();
                events.push(Event::OrderStalled {
                    unit: id,
                    player,
                    pos,
                    reason: StallReason::InsufficientScrap,
                });
                return;
            }
            state.player_mut(me).scrap -= 1;
        }
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        unit.progress += 1;
        let step = (ramp * (p + 1) / ramp_ticks) - (ramp * p / ramp_ticks);
        if step > 0 {
            builds.push(PendingHpGain {
                site: building,
                step,
                max_hp: stats.max_hp,
                completes: false,
                player: me,
                kind,
            });
        }
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// Firing positions for a chaser around an unstandable victim tile:
/// ring-scanned outward (row-major within a ring — the deterministic
/// snap every goal uses), keeping only tiles the chaser can stand on
/// AND shoot from — a stand-in beyond the weapon's Euclidean reach is
/// no stand-in at all (ring corners sit √2 further out than their
/// Chebyshev radius suggests). Candidates come back in scan order; the
/// caller takes the first it can actually route to. Empty when the
/// victim sits deeper in blocked ground than any weapon reaches.
fn chase_stand_ins(
    state: &State,
    domain: Domain,
    around: TilePos,
    range: chassis::fx::Fx,
) -> Vec<TilePos> {
    /// Furthest ring hunted for standing room — covers the longest
    /// anti-air reach (range 5 lands exactly on ring 5's axis tiles).
    const CHASE_STAND_RADIUS: i32 = 5;
    let aim = around.center();
    let mut out = Vec::new();
    for r in 1..=CHASE_STAND_RADIUS {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let t = around.offset(dx, dy);
                if state.passable_for(domain, t) && t.center().dist_sq(aim) <= range * range {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// The nearest enemy this unit's weapons can cover, in aggro range —
/// units before buildings, ties to the lowest id. `None` for pacifists,
/// empty horizons, and everything outside the weapon masks (a flak
/// crawler never picks a fight with infantry it cannot shoot).
fn acquire_target(state: &State, id: UnitId) -> Option<Target> {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if !stats.can_fight() {
        return None;
    }
    let (pos, me) = (unit.pos, unit.player);
    let aggro_sq = stats.aggro_range * stats.aggro_range;

    let unit_target = state
        .units
        .iter()
        .filter(|u| {
            state.hostile(me, u.player) && u.hp > 0 && stats.can_target(u.kind.stats().domain)
        })
        .map(|u| (pos.dist_sq(u.pos), u.id))
        .filter(|(d, _)| *d <= aggro_sq)
        .min();
    if let Some((_, uid)) = unit_target {
        return Some(Target::Unit(uid));
    }
    if !stats.can_target(Domain::Ground) {
        return None;
    }
    state
        .buildings
        .iter()
        .filter(|b| state.hostile(me, b.player) && b.hp > 0)
        .map(|b| (pos.dist_sq(b.closest_point_to(pos)), b.id))
        .filter(|(d, _)| *d <= aggro_sq)
        .min()
        .map(|(_, bid)| Target::Building(bid))
}

/// Idle combat units pick fights on their own.
fn idle(state: &mut State, id: UnitId) {
    if let Some(target) = acquire_target(state, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target,
            resume: None,
        };
        unit.path = None;
    }
}

/// March toward the goal, but engage anything that shows up on the way;
/// the attack order remembers the goal and hands it back afterwards.
fn attack_move(state: &mut State, id: UnitId, goal: TilePos, events: &mut Vec<Event>) {
    if let Some(target) = acquire_target(state, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target,
            resume: Some(goal),
        };
        unit.path = None;
        return;
    }
    walk(state, id, goal, events);
}

/// Walks toward an exact goal tile; going idle on arrival or when no route
/// exists. A unit close to the goal that bumps into an already-settled
/// arrival also counts as arrived — the whole group parks instead of
/// churning around the click point forever.
fn walk(state: &mut State, id: UnitId, goal: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let tile = unit.tile();
    if tile == goal || touching_settled_arrival(state, id, goal) {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    }
    let unit = state.unit(id).expect("caller checked");
    let has_fresh_path = unit.path.as_ref().is_some_and(|p| p.goal == goal);
    if has_fresh_path {
        return;
    }
    let (tile, kind) = (unit.tile(), unit.kind);
    let path = route_for(state, kind, tile, goal);
    let unit = state.unit_mut(id).expect("caller checked");
    match path {
        Some(waypoints) => {
            unit.path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
        }
        None => {
            let (player, pos) = (unit.player, unit.pos);
            unit.clear_program();
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
                reason: StallReason::NoRoute,
            });
        }
    }
}

/// Whether this near-goal unit is in contact with a settled (idle,
/// pathless) unit that itself sits near the same goal — the arrival wave
/// propagates outward from the first unit to park.
fn touching_settled_arrival(state: &State, id: UnitId, goal: TilePos) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let near_sq = crate::stats::ARRIVAL_NEAR * crate::stats::ARRIVAL_NEAR;
    let goal_center = goal.center();
    if unit.pos.dist_sq(goal_center) > near_sq {
        return false;
    }
    let my_stats = unit.kind.stats();
    let my_radius = my_stats.radius;
    let contact_slack = chassis::fx::Fx::lit("0.05");
    // Contact only means anything between bodies that collide: a flyer
    // hovering over a parked crowd is not "touching" it.
    state.units.iter().any(|other| {
        other.id != id
            && other.hp > 0
            && other.kind.stats().domain == my_stats.domain
            && other.path.is_none()
            && other.order == Order::Idle
            && other.pos.dist_sq(goal_center) <= near_sq
            && unit.pos.dist(other.pos) <= my_radius + other.kind.stats().radius + contact_slack
    })
}

/// The harvest loop: walk to the salvage, extract to capacity, haul to
/// the nearest Foundry, repeat; when the source dies, hop to a neighbor
/// source or go idle. Nodes are worked from an adjacent tile (they block
/// ground); wrecks are worked standing *on* the tile — they are junk on
/// open ground.
fn harvest(state: &mut State, id: UnitId, node: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let Some(hstats) = unit.kind.stats().harvest else {
        // Only harvesters ever get this order; be defensive anyway.
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    };
    let (tile, kind, carrying) = (unit.tile(), unit.kind, unit.carrying);
    let node_scrap = state.map.scrap_at(node);
    let node_wreck = state.map.wreck_at(node);

    if carrying >= hstats.capacity {
        deliver(state, id, node, events);
    } else if node_scrap > 0 {
        if tile_adjacent_to_rect(tile, node, (1, 1)) {
            extract(state, id, node, hstats.ticks_per_scrap, events);
        } else if !approach_rect(state, id, node, (1, 1)) {
            let unit = state.unit_mut(id).expect("caller checked");
            let (player, pos) = (unit.player, unit.pos);
            unit.clear_program();
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
                reason: StallReason::NoRoute,
            });
        }
    } else if node_wreck > 0 {
        if tile == node {
            extract_wreck(state, id, node, hstats.ticks_per_scrap);
        } else {
            let keep = state
                .unit(id)
                .expect("caller checked")
                .path
                .as_ref()
                .is_some_and(|p| p.goal == node);
            if !keep {
                let path = route_for(state, kind, tile, node);
                let unit = state.unit_mut(id).expect("caller checked");
                match path {
                    Some(waypoints) => {
                        unit.path = Some(PathFollow {
                            goal: node,
                            waypoints,
                            next: 0,
                        });
                    }
                    None => {
                        let (player, pos) = (unit.player, unit.pos);
                        unit.clear_program();
                        events.push(Event::OrderStalled {
                            unit: id,
                            player,
                            pos,
                            reason: StallReason::NoRoute,
                        });
                    }
                }
            }
        }
    } else {
        // Dry. Find a replacement source, else wrap up.
        match replacement_node(state, node, tile) {
            Some(next) => {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Harvest { node: next };
                unit.path = None;
                unit.progress = 0;
            }
            None if carrying > 0 => deliver(state, id, node, events),
            None => state.unit_mut(id).expect("caller checked").advance_queue(),
        }
    }
}

/// Stand on the wreck and strip it. Decay can beat the stripper to the
/// last piece — the dry-source branch above handles the morning after.
fn extract_wreck(state: &mut State, id: UnitId, node: TilePos, ticks_per_scrap: u32) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.path = None;
    unit.progress += 1;
    if unit.progress < ticks_per_scrap {
        return;
    }
    unit.progress = 0;
    if state.map.extract_wreck(node).is_some() {
        state.unit_mut(id).expect("caller checked").carrying += 1;
    }
}

/// Stand at the node and chip scrap off it.
fn extract(
    state: &mut State,
    id: UnitId,
    node: TilePos,
    ticks_per_scrap: u32,
    events: &mut Vec<Event>,
) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.path = None;
    unit.progress += 1;
    if unit.progress < ticks_per_scrap {
        return;
    }
    unit.progress = 0;
    unit.carrying += 1;
    if state.map.extract_scrap(node) == Some(0) {
        events.push(Event::NodeDepleted { pos: node });
    }
}

/// Haul the load to the nearest own Foundry; deposit when adjacent. After
/// depositing, resume the node (or go idle if it is gone for good).
fn deliver(state: &mut State, id: UnitId, node: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let (tile, me, carrying) = (unit.tile(), unit.player, unit.carrying);
    let pos = unit.pos;

    // Only a built Foundry takes deliveries — turrets and half-standing
    // sites are not drop-offs, however conveniently they're placed.
    let nearest = state
        .buildings
        .iter()
        .filter(|b| {
            b.player == me && b.hp > 0 && b.built && b.kind == crate::stats::BuildingKind::Foundry
        })
        .map(|b| (pos.dist_sq(b.center()), b.id))
        .min();
    let Some((_, foundry_id)) = nearest else {
        // Homeless: hold the scrap; the harvest is over, but a queued
        // program can still go on.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let foundry = state.building(foundry_id).expect("just found");
    let (anchor, size) = (foundry.anchor, foundry.kind.stats().size);

    if tile_adjacent_to_rect(tile, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.carrying = 0;
        unit.progress = 0;
        unit.path = None;
        // Saturating: a hostile scenario can start a bank near u32::MAX.
        // The event reports what was actually credited, not what was
        // carried — at the ceiling those differ.
        let bank = &mut state.player_mut(me).scrap;
        let credited = bank.saturating_add(carrying) - *bank;
        *bank += credited;
        events.push(Event::ScrapDeposited {
            player: me,
            amount: credited,
        });
        // Nothing left to go back to? Then we're done hauling.
        if state.map.scrap_at(node) == 0
            && state.map.wreck_at(node) == 0
            && replacement_node(state, node, tile).is_none()
        {
            state.unit_mut(id).expect("caller checked").advance_queue();
        }
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// Chase-and-hit. Range is measured to the target's closest point and
/// shots are buffered. A vanished target — or one no carried weapon can
/// cover — hands control back to the remembered attack-move (or idle,
/// where auto-acquire finds the next fight).
fn attack(
    state: &mut State,
    id: UnitId,
    target: Target,
    resume: Option<TilePos>,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if !stats.can_fight() {
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    }
    let (pos, tile, me, kind, cooldowns) = (
        unit.pos,
        unit.tile(),
        unit.player,
        unit.kind,
        unit.cooldowns,
    );

    // An attack-mover pounding a building stays alert: an enemy *unit*
    // wandering into aggro takes priority (deterministic — acquire prefers
    // units), so marching armies fight back instead of tunnel-visioning.
    if resume.is_some()
        && matches!(target, Target::Building(_))
        && let Some(better @ Target::Unit(_)) = acquire_target(state, id)
    {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target: better,
            resume,
        };
        unit.path = None;
        return;
    }

    // Resolve the target's current position; None means it is gone. A
    // target outside every weapon mask ends the engagement the same way —
    // nothing this chassis carries will ever land on it.
    let target_info: Option<(Vec2Fx, TilePos)> = match target {
        Target::Unit(uid) => state
            .unit(uid)
            .filter(|t| t.hp > 0)
            .map(|t| (t.pos, t.tile())),
        Target::Building(bid) => state
            .building(bid)
            .filter(|b| b.hp > 0)
            .map(|b| (b.closest_point_to(pos), b.anchor)),
    };
    let victim_domain = target_domain(state, target);
    let primary = stats
        .weapons
        .iter()
        .position(|w| w.targets.covers(victim_domain));
    let (Some((aim_point, target_tile)), Some(pi)) = (target_info, primary) else {
        let unit = state.unit_mut(id).expect("caller checked");
        match resume {
            Some(goal) => {
                unit.order = Order::AttackMove { goal };
                unit.path = None;
            }
            None => unit.advance_queue(),
        }
        return;
    };
    let weapon = &stats.weapons[pi];

    // In range only counts with a clear line — and with eyes. Terrain
    // cover (rock, non-victim buildings) applies to direct ground-vs-
    // ground fire only; shots to or from the air and indirect shells arc
    // past it — but nothing arcs past a peak. The owner must currently
    // *see* the victim's tile: a gun that outranges its own vision fires
    // on a spotter's sight (scrap piles are low junk — fire passes over
    // them). No shot → keep approaching; the chase path already routes
    // around what's in the way.
    let shot_open = |t: TilePos, full: bool| {
        let Some(tile) = state.map.tile(t) else {
            return false;
        };
        if tile.terrain == crate::map::Terrain::Peak {
            return false;
        }
        if !full {
            return true;
        }
        let building_open = match target {
            Target::Building(bid) => state.building_at(t).is_none_or(|b| b.id == bid),
            _ => state.building_at(t).is_none(),
        };
        tile.terrain == crate::map::Terrain::Ground && building_open
    };
    // Sight of any footprint tile serves for a building (matching attack
    // validation); a unit is seen at its own tile. The line trace runs
    // only once in range — it is not built for cross-map endpoints.
    let seen = match target {
        Target::Unit(_) => state.can_see(me, target_tile),
        Target::Building(bid) => state
            .building(bid)
            .is_some_and(|b| b.tiles().any(|t| state.can_see(me, t))),
    };
    let in_range = pos.dist_sq(aim_point) <= weapon.range * weapon.range;
    let full = traces_terrain(weapon, stats.domain, victim_domain);
    if in_range && seen && !chassis::path::line_blocked(pos, aim_point, |t| shot_open(t, full)) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        if cooldowns[pi] == 0 {
            unit.cooldowns[pi] = weapon.cooldown_ticks;
            if weapon.projectile {
                let flight = launch_shell(
                    state,
                    launches,
                    Target::Unit(id),
                    me,
                    pos,
                    aim_point,
                    weapon,
                );
                events.push(Event::ShellLaunched {
                    player: me,
                    from: pos,
                    to: aim_point,
                    flight,
                });
            } else {
                buffer_shot(state, Target::Unit(id), me, target, aim_point, weapon, hits);
                events.push(Event::AttackHit {
                    attacker: id,
                    attacker_kind: kind,
                    weapon: pi,
                    target,
                    attacker_pos: pos,
                    target_pos: aim_point,
                });
            }
        }
        fire_sidearms(state, id, pi, hits, events);
        return;
    }
    // Opportunist guns don't wait for the march to end.
    fire_sidearms(state, id, pi, hits, events);

    // Out of range (or blind, or blocked): chase.
    let reached: Result<(), StallReason> = match target {
        Target::Unit(_) => {
            // A ground chaser cannot stand where a flyer hovers — over
            // rock, over a roof — so it marches to a tile it CAN stand
            // on and shoot from instead; getting within weapon range is
            // the job, occupying the victim's tile never was. Air
            // chasers (and reachable tiles) keep the direct goal.
            let direct = state.passable_for(stats.domain, target_tile);
            // Repath when the target has drifted a tile from the
            // path's goal — cheap pursuit without per-tick A*. A path
            // already aimed at a firing position for this victim stays
            // fresh while it parks, or a grounded chaser would repath
            // forever.
            let stale = state
                .unit(id)
                .expect("caller checked")
                .path
                .as_ref()
                .is_none_or(|p| {
                    if direct {
                        p.goal != target_tile && p.goal.chebyshev(target_tile) > 1
                    } else {
                        !(state.passable_for(stats.domain, p.goal)
                            && p.goal.center().dist_sq(target_tile.center())
                                <= weapon.range * weapon.range)
                    }
                });
            if stale {
                let routed = if direct {
                    route_for(state, kind, tile, target_tile).map(|w| (target_tile, w))
                } else {
                    // Scan-order candidates, first one that routes wins:
                    // an isolated pocket next to the victim must not
                    // stall a chaser that could fire from the far side.
                    chase_stand_ins(state, stats.domain, target_tile, weapon.range)
                        .into_iter()
                        .find_map(|goal| route_for(state, kind, tile, goal).map(|w| (goal, w)))
                };
                match routed {
                    Some((goal, waypoints)) => {
                        let unit = state.unit_mut(id).expect("caller checked");
                        unit.path = Some(PathFollow {
                            goal,
                            waypoints,
                            next: 0,
                        });
                        Ok(())
                    }
                    None if !direct
                        && chase_stand_ins(state, stats.domain, target_tile, weapon.range)
                            .is_empty() =>
                    {
                        Err(StallReason::NoFiringPosition)
                    }
                    None => Err(StallReason::NoRoute),
                }
            } else {
                Ok(())
            }
        }
        Target::Building(bid) => {
            let b = state.building(bid).expect("resolved above");
            let (anchor, size) = (b.anchor, b.kind.stats().size);
            if approach_rect(state, id, anchor, size) {
                Ok(())
            } else {
                Err(StallReason::NoRoute)
            }
        }
    };
    if let Err(reason) = reached {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason,
        });
    }
}

/// Weapons other than the one engaging the ordered target pick their own
/// fights: the nearest hostile unit each can cover, in range, seen by the
/// owner, and clear — opportunist fire that never steers the chassis.
fn fire_sidearms(
    state: &mut State,
    id: UnitId,
    primary: usize,
    hits: &mut Vec<PendingHit>,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if stats.weapons.len() < 2 {
        return;
    }
    let (pos, me, kind, cooldowns) = (unit.pos, unit.player, unit.kind, unit.cooldowns);
    for (wi, weapon) in stats.weapons.iter().enumerate() {
        if wi == primary || cooldowns[wi] > 0 {
            continue;
        }
        let range_sq = weapon.range * weapon.range;
        let shot_open = |t: TilePos, full: bool| {
            let Some(tile) = state.map.tile(t) else {
                return false;
            };
            if tile.terrain == crate::map::Terrain::Peak {
                return false;
            }
            !full || (tile.terrain == crate::map::Terrain::Ground && state.building_at(t).is_none())
        };
        let victim = state
            .units
            .iter()
            .filter(|u| {
                state.hostile(me, u.player)
                    && u.hp > 0
                    && weapon.targets.covers(u.kind.stats().domain)
            })
            .filter(|u| state.can_see(me, u.tile()))
            .map(|u| (pos.dist_sq(u.pos), u.id, u.pos, u.kind.stats().domain))
            .filter(|(d, _, _, _)| *d <= range_sq)
            .filter(|(_, _, upos, dom)| {
                let full = traces_terrain(weapon, stats.domain, *dom);
                !chassis::path::line_blocked(pos, *upos, |t| shot_open(t, full))
            })
            .min_by_key(|&(d, uid, _, _)| (d, uid));
        let Some((_, uid, upos, _)) = victim else {
            continue;
        };
        state.unit_mut(id).expect("caller checked").cooldowns[wi] = weapon.cooldown_ticks;
        buffer_shot(
            state,
            Target::Unit(id),
            me,
            Target::Unit(uid),
            upos,
            weapon,
            hits,
        );
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: kind,
            weapon: wi,
            target: Target::Unit(uid),
            attacker_pos: pos,
            target_pos: upos,
        });
    }
}

/// Damage answers back: a hit unit that can fight and isn't already
/// fighting turns on its attacker — the counter to weapons that outrange
/// aggro (nothing else ever gets this far: inside aggro, auto-acquire
/// already found the attacker). An attack-mover keeps its destination as
/// the resume point. Brains run in id order, so the first hit of a tick
/// picks the target deterministically.
fn retaliate(state: &mut State, victim: UnitId, attacker: Target) {
    let Some(unit) = state.unit(victim) else {
        return;
    };
    let stats = unit.kind.stats();
    let attacker_domain = target_domain(state, attacker);
    if unit.hp == 0 || !stats.can_target(attacker_domain) {
        return;
    }
    // Answering fire needs eyes: an indirect shell lobbed from beyond
    // every friendly sight line reveals nothing to march after — chasing
    // it would hand out free intel and a suicide route.
    let seen = match attacker {
        Target::Unit(uid) => state
            .unit(uid)
            .is_some_and(|a| state.can_see(unit.player, a.tile())),
        Target::Building(bid) => state
            .building(bid)
            .is_some_and(|b| b.tiles().any(|t| state.can_see(unit.player, t))),
    };
    if !seen {
        return;
    }
    let resume = match unit.order {
        Order::Idle => None,
        Order::AttackMove { goal } => Some(goal),
        // An attack aimed at something that just died in resolution is no
        // engagement — a victim auto-acquired a neighbor this tick, the
        // neighbor fell in the volley, and without this arm the busy-guard
        // would let a surviving out-of-aggro shooter fire unanswered for
        // another full cooldown. Live targets stay protected.
        Order::Attack { target, resume } if !target_standing(state, target) => resume,
        _ => return, // already busy fighting or working
    };
    let unit = state.unit_mut(victim).expect("checked above");
    unit.order = Order::Attack {
        target: attacker,
        resume,
    };
    unit.path = None;
}

/// Whether a target is still on the field with hit points.
fn target_standing(state: &State, target: Target) -> bool {
    match target {
        Target::Unit(u) => state.unit(u).is_some_and(|u| u.hp > 0),
        Target::Building(b) => state.building(b).is_some_and(|b| b.hp > 0),
    }
}

/// Ensures the unit is walking to some passable tile touching the rectangle.
/// Returns false when no ring tile is reachable.
fn approach_rect(state: &mut State, id: UnitId, anchor: TilePos, size: (i32, i32)) -> bool {
    let (tile, kind) = {
        let u = state.unit(id).expect("caller checked");
        (u.tile(), u.kind)
    };
    let keep = state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_some_and(|p| tile_adjacent_to_rect(p.goal, anchor, size));
    if keep {
        return true;
    }
    // Candidate doorsteps, nearest first (stable sort, so ties stay in
    // ring order). The nearest few are then rotated by unit id so a crowd
    // heading for the same rectangle fans out across doorsteps instead of
    // magnetizing onto one tile and jamming — the exact configuration that
    // froze bot economies. Only the near face rotates: a lone unit never
    // detours to the building's far side.
    let domain = kind.stats().domain;
    let mut candidates: Vec<TilePos> = rect_adjacent_tiles(anchor, size)
        .filter(|&t| state.passable_for(domain, t))
        .collect();
    candidates.sort_by_key(|t| t.chebyshev(tile));
    let near = candidates.len().min(4);
    if near > 1 {
        candidates[..near].rotate_left(id.0 as usize % near);
    }
    for goal in candidates {
        if let Some(waypoints) = route_for(state, kind, tile, goal) {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
            return true;
        }
    }
    false
}

/// The nearest tile still holding salvage — node scrap or wreck — within
/// [`RETARGET_RADIUS`] of a dead source, keyed by (distance from the
/// unit, y, x) so the pick is unique.
fn replacement_node(state: &State, around: TilePos, unit_tile: TilePos) -> Option<TilePos> {
    let mut best: Option<(i32, i32, i32)> = None;
    for dy in -RETARGET_RADIUS..=RETARGET_RADIUS {
        for dx in -RETARGET_RADIUS..=RETARGET_RADIUS {
            let t = around.offset(dx, dy);
            if state.map.scrap_at(t) == 0 && state.map.wreck_at(t) == 0 {
                continue;
            }
            let key = (t.manhattan(unit_tile), t.y, t.x);
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        }
    }
    best.map(|(_, y, x)| TilePos::new(x, y))
}
