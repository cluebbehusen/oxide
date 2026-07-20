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

use super::{astar_for, rect_adjacent_tiles, tile_adjacent_to_rect};
use crate::event::Event;
use crate::ids::{Target, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::RETARGET_RADIUS;
use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;

/// A shot decided this tick, applied after every brain has acted.
struct PendingHit {
    attacker: Target,
    victim: Target,
    damage: u32,
}

/// A construction hp-gain decided this tick. Buffered like damage, and
/// applied *after* it: the documented rule is that a site zeroed by fire
/// is dead even if its builder acted the same tick — the shooter aimed at
/// the start-of-tick world, where the hit was lethal. Completion buffers
/// too: a site whose final tick coincides with a lethal volley must never
/// come online — no free turret shot, no "online" fanfare before death.
struct PendingBuild {
    site: crate::ids::BuildingId,
    step: u32,
    max_hp: u32,
    completes: bool,
    player: crate::ids::PlayerId,
    kind: crate::stats::BuildingKind,
}

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    let mut hits: Vec<PendingHit> = Vec::new();
    let mut builds: Vec<PendingBuild> = Vec::new();
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
            unit.cooldown = unit.cooldown.saturating_sub(1);
        }
        let order = state.unit(id).expect("just seen").order;
        match order {
            Order::Idle => idle(state, id),
            Order::Move { goal } => walk(state, id, goal, events),
            Order::Harvest { node } => harvest(state, id, node, events),
            Order::Attack { target, resume } => {
                attack(state, id, target, resume, events, &mut hits)
            }
            Order::AttackMove { goal } => attack_move(state, id, goal, events),
            Order::Build { site } => build(state, id, site, events, &mut builds),
        }
    }
    turret_fire(state, events, &mut hits);
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
    builds: Vec<PendingBuild>,
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

/// Built turrets pick their own fights: nearest enemy unit in range with a
/// clear line (buildings can't chase, so out-of-line targets are simply
/// ignored until they move). Stateless — target choice re-evaluates every
/// shot, in building-id order.
fn turret_fire(state: &mut State, events: &mut Vec<Event>, hits: &mut Vec<PendingHit>) {
    let ids: Vec<crate::ids::BuildingId> = state.buildings.iter().map(|b| b.id).collect();
    for id in ids {
        let Some(b) = state.building(id) else {
            continue;
        };
        let Some(atk) = b.kind.stats().attack else {
            continue;
        };
        if !b.built || b.hp == 0 {
            continue;
        }
        let (me, center, cooling) = (b.player, b.center(), b.cooldown > 0);
        if cooling {
            let b = state.building_mut(id).expect("just seen");
            b.cooldown -= 1;
            if b.cooldown > 0 {
                continue;
            }
            // Reached zero this tick: fire now, like unit cooldowns do.
        }
        let range_sq = atk.range * atk.range;
        let clear_shot = |t: TilePos| {
            let terrain_open = state
                .map
                .tile(t)
                .is_some_and(|tile| tile.terrain != crate::map::Terrain::Rock);
            let building_open = state.building_at(t).is_none_or(|other| other.id == id);
            terrain_open && building_open
        };
        let victim = state
            .units
            .iter()
            .filter(|u| u.player != me && u.hp > 0)
            .map(|u| (center.dist_sq(u.pos), u.id, u.pos))
            .filter(|(d, _, _)| *d <= range_sq)
            .filter(|(_, _, pos)| !chassis::path::line_blocked(center, *pos, clear_shot))
            .min_by_key(|(d, uid, _)| (*d, *uid));
        let Some((_, uid, upos)) = victim else {
            continue;
        };
        let b = state.building_mut(id).expect("just seen");
        b.cooldown = atk.cooldown_ticks;
        hits.push(PendingHit {
            attacker: Target::Building(id),
            victim: Target::Unit(uid),
            damage: atk.damage,
        });
        events.push(Event::TurretFired {
            turret: id,
            target: uid,
            turret_pos: center,
            target_pos: upos,
        });
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
    builds: &mut Vec<PendingBuild>,
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
        // after damage — see PendingBuild. The builder learns the site is
        // done next tick, through the built-site branch above.
        let completes = b.progress >= build_ticks;
        if step > 0 || completes {
            builds.push(PendingBuild {
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
        });
    }
}

/// The nearest enemy in this unit's aggro range — units before buildings,
/// ties to the lowest id. `None` for pacifists and empty horizons.
fn acquire_target(state: &State, id: UnitId) -> Option<Target> {
    let unit = state.unit(id).expect("caller checked");
    let atk = unit.kind.stats().attack?;
    let (pos, me) = (unit.pos, unit.player);
    let aggro_sq = atk.aggro_range * atk.aggro_range;

    let unit_target = state
        .units
        .iter()
        .filter(|u| u.player != me && u.hp > 0)
        .map(|u| (pos.dist_sq(u.pos), u.id))
        .filter(|(d, _)| *d <= aggro_sq)
        .min();
    if let Some((_, uid)) = unit_target {
        return Some(Target::Unit(uid));
    }
    state
        .buildings
        .iter()
        .filter(|b| b.player != me && b.hp > 0)
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
    let tile = unit.tile();
    let path = astar_for(state, tile, goal);
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
    let my_radius = unit.kind.stats().radius;
    let contact_slack = chassis::fx::Fx::lit("0.05");
    state.units.iter().any(|other| {
        other.id != id
            && other.hp > 0
            && other.path.is_none()
            && other.order == Order::Idle
            && other.pos.dist_sq(goal_center) <= near_sq
            && unit.pos.dist(other.pos) <= my_radius + other.kind.stats().radius + contact_slack
    })
}

/// The harvest loop: walk to node, extract to capacity, haul to the nearest
/// Foundry, repeat; when the node dies, hop to a neighbor node or go idle.
fn harvest(state: &mut State, id: UnitId, node: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let Some(hstats) = unit.kind.stats().harvest else {
        // Only harvesters ever get this order; be defensive anyway.
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    };
    let (tile, carrying) = (unit.tile(), unit.carrying);
    let node_scrap = state.map.scrap_at(node);

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
            });
        }
    } else {
        // Node is dry. Find a replacement, else wrap up.
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
        if state.map.scrap_at(node) == 0 && replacement_node(state, node, tile).is_none() {
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
        });
    }
}

/// Chase-and-hit. Range is measured to the target's closest point and damage
/// is immediate. A vanished target hands control back to the remembered
/// attack-move (or idle, where auto-acquire finds the next fight).
fn attack(
    state: &mut State,
    id: UnitId,
    target: Target,
    resume: Option<TilePos>,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
) {
    let unit = state.unit(id).expect("caller checked");
    let Some(atk) = unit.kind.stats().attack else {
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    };
    let (pos, tile, cooldown) = (unit.pos, unit.tile(), unit.cooldown);

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

    // Resolve the target's current position; None means it is gone.
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
    let Some((aim_point, target_tile)) = target_info else {
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

    // In range only counts with a clear line: rock is cover, and buildings
    // (other than the victim itself) block shots. Scrap piles are low junk
    // — fire passes over them. No LOS → keep approaching; the chase path
    // already routes around whatever is in the way.
    let clear_shot = |t: TilePos| {
        let terrain_open = state
            .map
            .tile(t)
            .is_some_and(|tile| tile.terrain != crate::map::Terrain::Rock);
        let building_open = match target {
            Target::Building(bid) => state.building_at(t).is_none_or(|b| b.id == bid),
            _ => state.building_at(t).is_none(),
        };
        terrain_open && building_open
    };
    let in_range = pos.dist_sq(aim_point) <= atk.range * atk.range;
    if in_range && !chassis::path::line_blocked(pos, aim_point, clear_shot) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        if cooldown > 0 {
            return;
        }
        unit.cooldown = atk.cooldown_ticks;
        hits.push(PendingHit {
            attacker: Target::Unit(id),
            victim: target,
            damage: atk.damage,
        });
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: state.unit(id).expect("caller checked").kind,
            target,
            attacker_pos: pos,
            target_pos: aim_point,
        });
        return;
    }

    // Out of range: chase.
    let reached = match target {
        Target::Unit(_) => {
            // Repath only when the target has drifted a tile away from the
            // path's goal — cheap pursuit without per-tick A*.
            let stale = state
                .unit(id)
                .expect("caller checked")
                .path
                .as_ref()
                .is_none_or(|p| p.goal.chebyshev(target_tile) > 1);
            if stale {
                let path = astar_for(state, tile, target_tile);
                let unit = state.unit_mut(id).expect("caller checked");
                match path {
                    Some(waypoints) => {
                        unit.path = Some(PathFollow {
                            goal: target_tile,
                            waypoints,
                            next: 0,
                        });
                        true
                    }
                    None => false,
                }
            } else {
                true
            }
        }
        Target::Building(bid) => {
            let b = state.building(bid).expect("resolved above");
            let (anchor, size) = (b.anchor, b.kind.stats().size);
            approach_rect(state, id, anchor, size)
        }
    };
    if !reached {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
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
    if unit.hp == 0 || unit.kind.stats().attack.is_none() {
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
    let tile = state.unit(id).expect("caller checked").tile();
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
    let mut candidates: Vec<TilePos> = rect_adjacent_tiles(anchor, size)
        .filter(|&t| state.passable(t))
        .collect();
    candidates.sort_by_key(|t| t.chebyshev(tile));
    let near = candidates.len().min(4);
    if near > 1 {
        candidates[..near].rotate_left(id.0 as usize % near);
    }
    for goal in candidates {
        if let Some(waypoints) = astar_for(state, tile, goal) {
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

/// The nearest tile still holding scrap within [`RETARGET_RADIUS`] of a dead
/// node — keyed by (distance from the unit, y, x) so the pick is unique.
fn replacement_node(state: &State, around: TilePos, unit_tile: TilePos) -> Option<TilePos> {
    let mut best: Option<(i32, i32, i32)> = None;
    for dy in -RETARGET_RADIUS..=RETARGET_RADIUS {
        for dx in -RETARGET_RADIUS..=RETARGET_RADIUS {
            let t = around.offset(dx, dy);
            if state.map.scrap_at(t) == 0 {
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
