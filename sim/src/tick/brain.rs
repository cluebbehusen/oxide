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

use crate::event::Event;
use crate::ids::{Target, UnitId};
use crate::state::{Order, State};

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
    completes: bool,
    player: crate::ids::PlayerId,
    kind: crate::stats::BuildingKind,
    /// Scrap this welder prepaid for this tick's step (zero for
    /// construction, which pays at placement). Stacked welders bill
    /// their own meters against the same start-of-tick reading, so a
    /// welder whose whole step lands past the hp ceiling gets exactly
    /// this coin back at resolution.
    paid: u32,
}

/// A buffered hp drain — salvage work — resolved with the gains as one
/// signed per-building delta after damage. A building fire zeroed this
/// tick forfeits every drain (fire wins; forfeited hp refunds
/// nothing), and refund crediting reads the hp the drain *actually*
/// removed, never the step it asked for.
struct PendingHpDrain {
    building: crate::ids::BuildingId,
    step: u32,
}

/// A deferred founder that arrived this tick. Buffered like damage and
/// resolved in unit-id order after the volley: brains iterate reversed
/// on odd ticks, and an inline claim would let tick parity decide which
/// of two arriving crewmates founds — the buffer keeps the choice a
/// pure function of ids.
struct PendingFounding {
    unit: UnitId,
    player: crate::ids::PlayerId,
    kind: crate::stats::BuildingKind,
    anchor: chassis::grid::TilePos,
}

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    let mut hits: Vec<PendingHit> = Vec::new();
    let mut builds: Vec<PendingHpGain> = Vec::new();
    let mut drains: Vec<PendingHpDrain> = Vec::new();
    let mut founds: Vec<PendingFounding> = Vec::new();
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
            Order::Salvage { building } => salvage(state, id, building, events, &mut drains),
            Order::Found { kind, anchor } => found(state, id, kind, anchor, events, &mut founds),
        }
    }
    turret_fire(state, events, &mut hits, &mut launches);
    // Arrivals join this tick's volley; launches land on later ticks
    // (flight is at least one tick), so ordering here cannot matter.
    land_shells(state, &mut hits, events);
    state.shells.extend(launches);
    resolve_hits(state, hits, builds, drains, events);
    resolve_founds(state, founds, events);
}

mod combat;
mod economy;
mod locomotion;

use combat::attack;
use combat::{land_shells, retaliate, target_standing, turret_fire};
use economy::{build, found, harvest, repair, salvage};
use locomotion::{attack_move, idle, walk};

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
    drains: Vec<PendingHpDrain>,
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
    // Stacked welders each prepaid their own meter against the same
    // start-of-tick hp reading, but the ceiling accepts hp in decision
    // order — a welder whose WHOLE step lands past it gets this tick's
    // coin back (the marginal welder's partially-accepted step keeps
    // its ceil-billed fraction: within the billing doc's one-scrap
    // tolerance). A building fire zeroed this tick refunds nothing —
    // fire wins and the crew's coin forfeits with its work.
    {
        let mut rooms: Vec<(crate::ids::BuildingId, i64)> = Vec::new();
        for gain in &builds {
            let Some(b) = state.building(gain.site).filter(|b| b.hp > 0) else {
                continue;
            };
            let i = match rooms.iter().position(|(id, _)| *id == gain.site) {
                Some(i) => i,
                None => {
                    let room = i64::from(b.kind.stats().max_hp) - i64::from(b.hp);
                    rooms.push((gain.site, room));
                    rooms.len() - 1
                }
            };
            if rooms[i].1 <= 0 {
                // Only the refund cares what was paid; EVERY gain
                // consumes room. A mid-meter welder frequently steps a
                // free hp (its coins land every few hp), and skipping
                // that gain here once let it eat the last room while a
                // prepaid neighbor took the clamp uncompensated.
                if gain.paid > 0 {
                    let bank = &mut state.player_mut(gain.player).scrap;
                    *bank = bank.saturating_add(gain.paid);
                }
            } else {
                rooms[i].1 -= i64::from(gain.step);
            }
        }
    }
    // Buffered work lands only on buildings that survived the volley
    // (hp > 0 after hits — fire wins, and a dead site forfeits gains
    // and drains alike). Per building, gains and drains net into ONE
    // signed delta clamped once to [0, max_hp]; the hp that delta
    // actually removed is what salvage crediting counts. In practice
    // gains and drains never meet on one building (construction wants
    // !built, salvage wants built, repair and salvage evict each
    // other), but the resolution is stated so the day they do has one
    // answer.
    struct Work {
        building: crate::ids::BuildingId,
        gain: i64,
        drain: i64,
        completes: Option<(crate::ids::PlayerId, crate::stats::BuildingKind)>,
    }
    let mut work: Vec<Work> = Vec::new();
    let slot = |v: &mut Vec<Work>, building| match v.iter_mut().position(|w| w.building == building)
    {
        Some(i) => i,
        None => {
            v.push(Work {
                building,
                gain: 0,
                drain: 0,
                completes: None,
            });
            v.len() - 1
        }
    };
    for gain in &builds {
        let i = slot(&mut work, gain.site);
        work[i].gain += i64::from(gain.step);
        if gain.completes && work[i].completes.is_none() {
            // First completion wins: two builders can both cross the
            // finish line in one tick, and the site comes online once.
            work[i].completes = Some((gain.player, gain.kind));
        }
    }
    for drain in &drains {
        let i = slot(&mut work, drain.building);
        work[i].drain += i64::from(drain.step);
    }
    for w in &work {
        let Some(b) = state.building_mut(w.building) else {
            continue;
        };
        if b.hp == 0 {
            continue; // fire won this tick; every gain and drain forfeits
        }
        let stats = b.kind.stats();
        let before = b.hp;
        let after = (i64::from(before) + w.gain - w.drain).clamp(0, i64::from(stats.max_hp)) as u32;
        b.hp = after;
        if let Some((player, kind)) = w.completes {
            b.built = true;
            b.progress = 0;
            events.push(Event::BuildingCompleted {
                building: w.building,
                player,
                kind,
            });
        }
        if w.drain > 0 && after < before {
            b.salvage_drained += before - after;
            // The cumulative ledger: credit whole scrap as the running
            // target passes it, so truncation never drifts and a
            // full-health salvage totals exactly cost * permille / 1000.
            let basis = stats.construction.map_or(0, |c| c.cost);
            let target = u64::from(b.salvage_drained)
                * u64::from(basis)
                * crate::stats::SALVAGE_REFUND_PERMILLE
                / (1000 * u64::from(stats.max_hp));
            let due = u32::try_from(target).unwrap_or(u32::MAX) - b.salvage_credited;
            if after == 0 {
                b.salvaged = true;
            }
            if due > 0 {
                b.salvage_credited += due;
                let player = b.player;
                let bank = &mut state.player_mut(player).scrap;
                *bank = bank.saturating_add(due);
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

/// Arrived deferred founders claim their ground, strictly in unit-id
/// order. The claim re-proves [`crate::State::place_refusal`] on ground
/// the founder now stands beside — adjacency puts the whole footprint
/// inside a harvester's sight, so every fact the verdict reads is one
/// the founder's own eyes deliver. Ground honestly taken (a building
/// raised, a hostile machine parked) drops the program with a fog-safe
/// stall; a crewmate whose lower-id partner founded first simply joins
/// the fresh site. Nothing was charged before this moment, so a failed
/// claim has nothing to refund.
fn resolve_founds(state: &mut State, mut founds: Vec<PendingFounding>, events: &mut Vec<Event>) {
    use crate::command::RejectReason;
    use crate::event::StallReason;
    founds.sort_unstable_by_key(|f| f.unit);
    for f in founds {
        let Some(unit) = state.unit(f.unit) else {
            continue;
        };
        if unit.hp == 0 {
            continue; // the volley won; a corpse claims nothing
        }
        // Retaliation (just resolved) can hand a hit machine a new
        // order; a founder no longer on the errand claims nothing.
        if unit.order
            != (Order::Found {
                kind: f.kind,
                anchor: f.anchor,
            })
        {
            continue;
        }
        // A lower-id crewmate founded this tick (or the site already
        // stood): join it instead of stalling on "taken" ground that
        // is the crew's own.
        let ours = state
            .buildings
            .iter()
            .find(|b| b.anchor == f.anchor && b.kind == f.kind && b.player == f.player && !b.built)
            .map(|b| b.id);
        if let Some(site) = ours {
            let unit = state.unit_mut(f.unit).expect("checked above");
            unit.order = Order::Build { site };
            unit.path = None;
            unit.progress = 0;
            continue;
        }
        let stall = |state: &mut State, reason: StallReason, events: &mut Vec<Event>| {
            let unit = state.unit_mut(f.unit).expect("checked above");
            let (player, pos) = (unit.player, unit.pos);
            unit.clear_program();
            events.push(Event::OrderStalled {
                unit: f.unit,
                player,
                pos,
                reason,
            });
        };
        if state.place_refusal(f.player, f.kind, f.anchor).is_some() {
            stall(state, StallReason::GroundTaken, events);
            continue;
        }
        let claimed = super::commands::found_site(
            state,
            f.player,
            f.unit,
            f.kind,
            f.anchor,
            |state, site| {
                // The founder's own active order becomes the build; its
                // queued program survives untouched — deferral changes
                // when the claim lands, never what comes after.
                let unit = state.unit_mut(f.unit).expect("checked above");
                unit.order = Order::Build { site };
                unit.path = None;
                unit.progress = 0;
                true
            },
        );
        match claimed {
            Ok(_) => {}
            Err(RejectReason::NotEnoughScrap) => {
                stall(state, StallReason::InsufficientScrap, events);
            }
            Err(_) => stall(state, StallReason::NoRoute, events),
        }
    }
}
