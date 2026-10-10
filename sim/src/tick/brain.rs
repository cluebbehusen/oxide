//! Unit brains: intent becomes action.
//!
//! Units decide in id order on even ticks and reverse id order on odd ticks.
//! Damage is buffered: every shot this tick is recorded and applied only after
//! all brains and turrets have acted, so everyone decides against the same
//! start-of-tick hp and two machines can kill each other in the same tick.
//! Inline damage would give the earlier decider a same-tick reaction edge.
//! Every selection a brain makes (targets, doorstep tiles, replacement nodes)
//! is ordered by an explicit key ending in an id or a position, so exactly one
//! choice is possible.

use crate::event::Event;
use crate::ids::{BuildingId, Target, UnitId};
use crate::state::{Building, Order, State};

/// A shot decided this tick, applied after every brain has acted.
struct PendingHit {
    attacker: Target,
    victim: Target,
    damage: u32,
    /// The exact point damage was aimed at or landed. Transient geometry only:
    /// it never enters authoritative serialized state.
    impact: chassis::fx::Vec2Fx,
    /// Incoming direction for resolving a tied warning tile on an even-sized
    /// footprint. A same-center unit attack falls back to chassis facing.
    approach: chassis::fx::Vec2Fx,
    /// Where the shot was fired from: the shooter's position, or a shell's
    /// launch point.
    origin: chassis::fx::Vec2Fx,
}

impl PendingHit {
    fn along(
        state: &State,
        attacker: Target,
        victim: Target,
        damage: u32,
        from: chassis::fx::Vec2Fx,
        impact: chassis::fx::Vec2Fx,
    ) -> Self {
        let mut approach = impact - from;
        if approach == chassis::fx::Vec2Fx::ZERO
            && let Target::Unit(id) = attacker
            && let Some(unit) = state.unit(id)
        {
            approach = chassis::compass::dir(unit.heading);
        }
        Self {
            attacker,
            victim,
            damage,
            impact,
            approach,
            origin: from,
        }
    }
}

/// A buffered building hp gain (construction, upgrade, or repair), applied
/// after damage: a building zeroed by fire is dead even if its crew acted the
/// same tick. Completion buffers too, so a site whose final tick coincides
/// with a lethal volley never comes online.
struct PendingHpGain {
    /// The first crew work, even when rounding gives it no hp gain.
    starts: bool,
    site: crate::ids::BuildingId,
    step: u32,
    completes: bool,
    player: crate::ids::PlayerId,
    kind: crate::stats::BuildingKind,
    /// Scrap this welder prepaid for this tick's step (zero for
    /// construction, which pays at placement). Stacked welders bill against
    /// the same start-of-tick hp, so a welder whose whole step lands past the
    /// hp ceiling gets this back at resolution.
    paid: u32,
    /// The Repair Bay that offered this gain, when it came from an aura.
    /// Construction, upgrades, and crew repairs leave this empty.
    repair_bay: Option<crate::ids::BuildingId>,
}

/// A buffered unit hp gain from a field weld or a Repair Bay. Every unit heal
/// must push into this list and resolve through [`resolve_unit_heals`]; two
/// independent unit-heal paths would be a determinism trap. Same contract as
/// [`PendingHpGain`]: applied after damage, so a machine the volley zeroed
/// forfeits every heal.
struct PendingUnitHeal {
    /// The patient.
    unit: UnitId,
    /// Hp this welder's tick offers.
    step: u32,
    /// Whose bank prepaid, for the ceiling refund.
    player: crate::ids::PlayerId,
    /// Scrap prepaid for this step, refunded when the whole step lands past
    /// the hp ceiling, as for a building weld.
    paid: u32,
    /// The exact producer, carried through resolution for output-only
    /// accepted-hp telemetry.
    source: crate::event::UnitRepairSource,
}

/// A field weld whose patient stood in reach when the welder's brain ran.
///
/// The patient's own brain may run later in the parity-alternating phase
/// and create a departure path. Commit these only after every brain has
/// resolved its intent so a weld never rides that same-tick departure.
struct PendingFieldWeld {
    /// The unit holding the torch.
    welder: UnitId,
    /// The wounded own machine offered the weld.
    patient: UnitId,
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

/// Runs every machine's decision and resolves the tick's buffered work.
/// Returns the pending boardings and unloads, and the buildings salvage
/// stripped to nothing, which cleanup removes without wreck.
pub(super) fn run(
    state: &mut State,
    index: &mut super::spatial::UnitIndex,
    events: &mut Vec<Event>,
) -> (logistics::Pending, Vec<crate::ids::BuildingId>) {
    // Positions and unit slots hold still until resolution, so acquisition
    // and arrival queries share this index. Orders, speed and landed state
    // can change during the loop and must still be read from the live unit.
    index.rebuild(&state.units);
    index.survey(state);
    let motion = MotionSnapshot::capture(state);
    let mut hits: Vec<PendingHit> = Vec::new();
    let mut builds: Vec<PendingHpGain> = Vec::new();
    let mut heals: Vec<PendingUnitHeal> = Vec::new();
    let mut field_welds: Vec<PendingFieldWeld> = Vec::new();
    let mut drains: Vec<PendingHpDrain> = Vec::new();
    let mut launches: Vec<crate::state::Shell> = Vec::new();
    let mut logistics_pending = logistics::Pending::default();
    let mut harvest_danger_by_team: Vec<Option<crate::vision::GroundSalvageDanger>> =
        (0..state.players.len()).map(|_| None).collect();
    let mut reach = super::reach::Reach::new(state);
    // Alternate direction by tick parity so no seat holds a standing
    // first-mover edge. With damage buffered, the remaining coupling (shared
    // scrap, own-side order state) is small, but in a mirror match any fixed
    // order decides.
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
        let automatic = matches!(
            state.unit(id).expect("live unit").order,
            Order::Idle | Order::Hunt { .. }
        );
        let mut acquired = None;
        if automatic
            && combat::automatic_radar(
                state,
                index,
                id,
                false,
                events,
                &mut hits,
                &mut launches,
                &mut acquired,
            )
        {
            continue;
        }
        if let Some(unit) = state.unit_mut(id)
            && !matches!(unit.order, Order::Attack { .. })
        {
            unit.retract_braces();
        }
        let order = state.unit(id).expect("just seen").order;
        {
            // Takeoff: any program that is not "rest here" lifts a landed
            // airframe off before it plans, so every order writer works
            // unchanged and the first airborne tick steers from the parked
            // heading at the ordinary turn rate.
            let unit = state.unit_mut(id).expect("just seen");
            if unit.landed && !unit.stays_parked() {
                unit.landed = false;
            }
        }
        let reported = events.len();
        match order {
            // An idle airframe stays parked and brace retraction moves
            // nothing, so the radar check's acquisition still holds here.
            Order::Idle => idle(state, index, id, acquired),
            Order::Run { .. } => {
                if !land_at_destination(state, index, &mut reach, id, events) {
                    walk(state, index, &mut reach, id, events);
                }
            }
            Order::ReturnCargo { foundry, repair } => {
                let player = state.unit(id).expect("caller checked").player;
                let team = state.player(player).team as usize;
                let danger = harvest_danger_by_team[team].get_or_insert_with(|| {
                    crate::vision::GroundSalvageDanger::capture(state, player)
                });
                economy::return_cargo(state, danger, id, foundry, repair, events);
            }
            Order::Harvest {
                node,
                anchor,
                retiring,
            } => {
                let player = state.unit(id).expect("just seen").player;
                let team = state.player(player).team as usize;
                let danger = harvest_danger_by_team[team].get_or_insert_with(|| {
                    crate::vision::GroundSalvageDanger::capture(state, player)
                });
                harvest(state, danger, id, node, anchor, retiring, events);
            }
            Order::Attack {
                target,
                resume,
                pursue,
            } => combat::attack_known(
                state,
                index,
                &motion,
                id,
                (target, resume, pursue),
                combat::ShotBuffers {
                    events,
                    hits: &mut hits,
                    launches: &mut launches,
                },
            ),
            Order::Hunt { goal } => hunt(state, index, &mut reach, id, goal, events),
            Order::Advance { goal } => advance(
                state,
                index,
                &mut reach,
                &motion,
                id,
                goal,
                events,
                &mut hits,
                &mut launches,
            ),
            Order::Build { site } => build(state, id, site, events, &mut builds),
            Order::Repair { building } => repair(state, id, building, events, &mut builds),
            Order::Salvage { building } => salvage(state, id, building, events, &mut drains),
            Order::Found { kind, anchor } => found(state, id, kind, anchor, events, &mut builds),
            Order::RepairUnit { unit } => repair_unit(state, id, unit, events, &mut field_welds),
            Order::Board { transport } => {
                logistics::board(state, id, transport, &mut logistics_pending, events);
            }
            Order::Unload { .. } => {
                logistics::unload(state, index, &mut reach, id, &mut logistics_pending, events);
            }
            Order::Land { goal, from } => land(state, index, id, goal, from, events),
        }
        if events[reported..]
            .iter()
            .any(|event| matches!(event, Event::NodeDepleted { .. }))
        {
            reach.forget_ground();
        }
        combat::normalize_order(state, id);
    }
    advance_upgrades(state, &mut builds);
    commit_unit_welds(state, field_welds, events, &mut heals);
    turret_fire(state, index, &motion, events, &mut hits, &mut launches);
    repair_bay_aura(state, &mut heals, &mut builds);
    crucible_smelter(state);
    // Arrivals join this tick's volley; launches land on later ticks
    // (flight is at least one tick), so ordering here cannot matter.
    land_shells(state, &mut hits, events);
    state.shells.extend(launches);
    let salvaged = resolve_hits(state, &hits, &builds, &heals, &drains, events);
    (logistics_pending, salvaged)
}

mod combat;
pub(in crate::tick) mod contact;
mod economy;
pub(super) use economy::return_cargo_destination;
mod locomotion;
pub(super) mod logistics;

use combat::{MotionSnapshot, advance, land_shells, retaliate, target_standing, turret_fire};
use economy::{
    advance_upgrades, build, commit_unit_welds, found, harvest, repair, repair_unit, salvage,
};
use locomotion::{hunt, idle, land, land_at_destination, walk};

/// Applies the tick's buffered work. Shots land in decision order: the tick's
/// unit order, then turrets in building-id order, then arriving shells. All
/// damage lands before retaliation, so a machine that died this tick answers
/// nothing and a survivor answers its earliest attacker that survived
/// resolution.
fn resolve_hits(
    state: &mut State,
    hits: &[PendingHit],
    builds: &[PendingHpGain],
    heals: &[PendingUnitHeal],
    drains: &[PendingHpDrain],
    events: &mut Vec<Event>,
) -> Vec<crate::ids::BuildingId> {
    struct Work {
        building: crate::ids::BuildingId,
        gain: i64,
        drain: i64,
        completes: Option<(crate::ids::PlayerId, crate::stats::BuildingKind)>,
    }
    let mut salvaged = Vec::new();
    let mut incidents = Vec::new();
    for hit in hits {
        match hit.victim {
            Target::Unit(uid) => {
                if let Some(v) = state.unit_mut(uid) {
                    let relevant_hit = v.kind.stats().harvest.is_some();
                    let relevant_loss =
                        hit.damage >= v.hp && v.domain() == crate::stats::Domain::Ground;
                    if v.hp > 0 && hit.damage > 0 && (relevant_hit || relevant_loss) {
                        incidents.push((v.player, v.tile(), hit));
                    }
                    super::damage::unit(v, hit.damage, events);
                }
            }
            Target::Building(bid) => {
                if state.building(bid).is_some_and(Building::provisional) {
                    continue;
                }
                let incident_tile = state.building(bid).and_then(|b| {
                    (hit.approach != chassis::fx::Vec2Fx::ZERO).then(|| {
                        super::footprint_incident_tile(
                            b.anchor,
                            b.kind.size(),
                            hit.impact,
                            hit.approach,
                        )
                    })
                });
                if let Some(b) = state.building_mut(bid) {
                    let relevant_hit = b.kind == crate::stats::BuildingKind::Reclaimer;
                    let relevant_loss = hit.damage >= b.hp;
                    if b.hp > 0
                        && hit.damage > 0
                        && (relevant_hit || relevant_loss)
                        && let Some(tile) = incident_tile
                    {
                        incidents.push((b.player, tile, hit));
                    }
                    super::damage::building(b, hit.damage, events);
                }
            }
        }
    }
    for (victim, tile, hit) in incidents {
        // A shooter the victim's team can see is already live danger, which
        // ends when it dies or leaves sight. Incident memory stands in only
        // for fire from out of sight. A shell's shooter may move in flight,
        // so only a dead one is judged by where it fired from.
        let shooter = match hit.attacker {
            Target::Unit(id) => state.unit(id).map(|unit| unit.pos),
            Target::Building(id) => state
                .building(id)
                .map(super::super::state::Building::center),
        }
        .unwrap_or(hit.origin);
        if !state.can_see(victim, chassis::grid::TilePos::containing(shooter)) {
            state.record_salvage_incident(victim, tile);
        }
    }
    let starts: Vec<_> = builds
        .iter()
        .filter(|gain| gain.starts)
        .map(|gain| gain.site)
        .collect();
    super::charges::detonate_under_construction(state, &starts, events);
    // Stacked welders each prepaid against the same start-of-tick hp, but
    // the ceiling accepts hp in decision order: a welder whose whole step
    // lands past it gets this tick's payment back. The marginal welder's
    // partially accepted step keeps its ceil-billed fraction, within the
    // one-scrap billing tolerance. A building fire zeroed this tick refunds
    // nothing.
    {
        let mut rooms: Vec<(crate::ids::BuildingId, i64)> = Vec::new();
        for gain in builds {
            let Some(b) = state.building(gain.site).filter(|b| b.hp > 0) else {
                continue;
            };
            let i = if let Some(i) = rooms.iter().position(|(id, _)| *id == gain.site) {
                i
            } else {
                let room = i64::from(b.stats().max_hp) - i64::from(b.hp);
                rooms.push((gain.site, room));
                rooms.len() - 1
            };
            let accepted = u32::try_from(rooms[i].1.clamp(0, i64::from(gain.step)))
                .expect("clamped to a u32 step");
            if rooms[i].1 <= 0 {
                // Every gain consumes room; only the refund cares what was
                // paid. A mid-meter welder often steps a free hp, and
                // skipping it here would let it eat the last room while a
                // prepaid neighbor took the clamp uncompensated.
                if gain.paid > 0 {
                    let bank = &mut state.player_mut(gain.player).scrap;
                    *bank = bank.saturating_add(gain.paid);
                }
            } else {
                rooms[i].1 -= i64::from(gain.step);
            }
            if let Some(repair_bay) = gain.repair_bay
                && accepted > 0
            {
                events.push(Event::BuildingRepaired {
                    building: gain.site,
                    player: gain.player,
                    repair_bay,
                    amount: accepted,
                });
            }
        }
    }
    // Buffered work lands only on buildings that survived the volley; a dead
    // site forfeits gains and drains alike. Per building, gains and drains
    // net into one signed delta clamped once to [0, max_hp], and salvage
    // credits the hp that delta actually removed. Gains and drains do not
    // currently meet on one building, but the netting gives that case one
    // answer.
    let mut work: Vec<Work> = Vec::new();
    let slot = |v: &mut Vec<Work>, building| {
        if let Some(i) = v.iter_mut().position(|w| w.building == building) {
            i
        } else {
            v.push(Work {
                building,
                gain: 0,
                drain: 0,
                completes: None,
            });
            v.len() - 1
        }
    };
    for gain in builds {
        let i = slot(&mut work, gain.site);
        work[i].gain += i64::from(gain.step);
        if gain.completes && work[i].completes.is_none() {
            // Two builders can both finish in one tick; the site comes
            // online once.
            work[i].completes = Some((gain.player, gain.kind));
        }
    }
    for drain in drains {
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
        let stats = b.stats();
        let before = b.hp;
        let after =
            u32::try_from((i64::from(before) + w.gain - w.drain).clamp(0, i64::from(stats.max_hp)))
                .expect("clamped to a u32 max_hp");
        b.hp = after;
        if let Some((player, kind)) = w.completes {
            b.complete();
            events.push(Event::BuildingCompleted {
                building: w.building,
                player,
                kind,
            });
        }
        if w.drain > 0 && after < before {
            b.salvage_drained += before - after;
            // Credit whole scrap as the cumulative target passes it, so
            // truncation never drifts and a full-health salvage totals
            // cost * permille / 1000.
            let basis = stats.construction.map_or(0, |c| c.cost);
            let target = u64::from(b.salvage_drained)
                * u64::from(basis)
                * crate::stats::SALVAGE_REFUND_PERMILLE
                / (1000 * u64::from(stats.max_hp));
            let due = u32::try_from(target).unwrap_or(u32::MAX) - b.salvage_credited;
            if after == 0 {
                salvaged.push(b.id);
            }
            if due > 0 {
                b.salvage_credited += due;
                let player = b.player;
                let bank = &mut state.player_mut(player).scrap;
                *bank = bank.saturating_add(due);
            }
        }
    }
    resolve_unit_heals(state, heals, events);
    for hit in hits {
        if let Target::Unit(uid) = hit.victim
            && target_standing(state, hit.attacker)
        {
            retaliate(state, uid, hit.attacker);
        }
    }
    salvaged
}

/// Resolves every buffered unit heal, with the building resolver's rules: a
/// unit at 0 hp after the volley forfeits every heal and refunds nothing; a
/// heal whose whole step lands past the hp ceiling refunds its prepayment;
/// and per unit the gains net into one delta clamped once to `max_hp`.
fn resolve_unit_heals(state: &mut State, heals: &[PendingUnitHeal], events: &mut Vec<Event>) {
    let mut rooms: Vec<(UnitId, i64)> = Vec::new();
    for heal in heals {
        let Some(u) = state.unit(heal.unit).filter(|u| u.hp > 0) else {
            continue;
        };
        let i = if let Some(i) = rooms.iter().position(|(id, _)| *id == heal.unit) {
            i
        } else {
            let room = i64::from(u.kind.stats().max_hp) - i64::from(u.hp);
            rooms.push((heal.unit, room));
            rooms.len() - 1
        };
        let accepted = u32::try_from(rooms[i].1.clamp(0, i64::from(heal.step)))
            .expect("clamped to a u32 step");
        if rooms[i].1 <= 0 {
            // Every gain consumes room; only the refund cares what was
            // paid, as in the building ledger.
            if heal.paid > 0 {
                let bank = &mut state.player_mut(heal.player).scrap;
                *bank = bank.saturating_add(heal.paid);
            }
        } else {
            rooms[i].1 -= i64::from(heal.step);
        }
        if accepted > 0 {
            events.push(Event::UnitRepaired {
                unit: heal.unit,
                player: heal.player,
                source: heal.source,
                amount: accepted,
            });
        }
    }
    let mut sums: Vec<(UnitId, i64)> = Vec::new();
    for heal in heals {
        match sums.iter_mut().find(|(id, _)| *id == heal.unit) {
            Some((_, gain)) => *gain += i64::from(heal.step),
            None => sums.push((heal.unit, i64::from(heal.step))),
        }
    }
    for (unit, gain) in sums {
        let Some(u) = state.unit_mut(unit) else {
            continue;
        };
        if u.hp == 0 {
            continue; // fire won this tick; the heal and its payment forfeit
        }
        let max = i64::from(u.kind.stats().max_hp);
        u.hp =
            u32::try_from((i64::from(u.hp) + gain).clamp(0, max)).expect("clamped to a u32 max_hp");
    }
}

/// Repair Bay healing. On its pulse cadence every built bay, in building-id
/// order, offers each own wounded unit and completed structure inside
/// [`crate::stats::REPAIR_BAY_RADIUS`] of its footprint one
/// [`crate::stats::REPAIR_BAY_STEP`] of hp, billed from the owner's bank at
/// [`crate::stats::REPAIR_COST_PERMILLE`] of the patient's active tier cost.
/// Reach measures from body to footprint for units and between the nearest
/// footprint edges for structures. A Bay cannot heal itself;
/// another Bay may heal it. Units have first claim on limited scrap, then
/// structures in building-id order. Everything buffers into the shared hp
/// resolvers, so lethal fire wins the tick and overlapping sources settle
/// against the same ceiling.
fn repair_bay_aura(
    state: &mut State,
    heals: &mut Vec<PendingUnitHeal>,
    builds: &mut Vec<PendingHpGain>,
) {
    use crate::stats::BuildingKind;
    if !state
        .current_tick()
        .is_multiple_of(crate::stats::REPAIR_BAY_PERIOD)
    {
        return;
    }
    let bays: Vec<crate::ids::BuildingId> = state
        .buildings
        .iter()
        .filter(|b| b.built() && b.hp > 0 && b.kind == BuildingKind::RepairBay)
        .map(|b| b.id)
        .collect();
    let radius = crate::stats::REPAIR_BAY_RADIUS;
    // Unit steps already queued per patient this pulse. Overlapping auras
    // stack in resolution, so a later bay prices from the hp earlier bays
    // queued; pricing from start-of-tick hp would bill every bay for the
    // same first interval.
    let mut in_flight: std::collections::BTreeMap<UnitId, u32> = std::collections::BTreeMap::new();
    for bay in bays.iter().copied() {
        let Some(b) = state.building(bay) else {
            continue;
        };
        let owner = b.player;
        let recovery_reserve = state.recovery_reserve(owner);
        let patients: Vec<UnitId> = state
            .units
            .iter()
            .filter(|u| {
                u.player == owner
                    && u.hp > 0
                    && u.hp < u.kind.stats().max_hp
                    && b.closest_point_to(u.pos).dist_sq(u.pos) <= radius * radius
            })
            .map(|u| u.id)
            .collect();
        for id in patients {
            let u = state.unit(id).expect("just filtered");
            let stats = u.kind.stats();
            let queued = in_flight.get(&id).copied().unwrap_or(0);
            let healed = (u.hp + queued).min(stats.max_hp);
            let step = crate::stats::REPAIR_BAY_STEP.min(stats.max_hp - healed);
            if step == 0 {
                continue; // earlier bays already carry this one to whole
            }
            let millis = |hp: u32| -> u64 {
                u64::from(hp) * u64::from(stats.cost) * crate::stats::REPAIR_COST_PERMILLE
                    / u64::from(stats.max_hp)
            };
            let due = millis(healed + step).div_ceil(1000) - millis(healed).div_ceil(1000);
            let bank = state.player(owner).scrap;
            let Some(due) = u32::try_from(due).ok().filter(|&due| due <= bank) else {
                continue; // broke for this patient; cheaper coins may still land
            };
            if due > 0 && recovery_reserve > 0 && bank - due < recovery_reserve {
                continue;
            }
            state.player_mut(owner).scrap = bank - due;
            in_flight.insert(id, queued + step);
            heals.push(PendingUnitHeal {
                unit: id,
                step,
                player: owner,
                paid: due,
                source: crate::event::UnitRepairSource::RepairBay { building: bay },
            });
        }
    }

    // Structures compete for the remaining bank only after every Bay has
    // offered its unit pulses. Building gains share PendingHpGain with crew
    // repair and upgrades, so damage, clamping, and refunds keep one
    // resolver. Automatic repair must not fight an explicit teardown: the
    // aura has no order for a salvage command to purge, so skip every target
    // of an active or queued salvage job.
    let salvage_targets: std::collections::BTreeSet<BuildingId> = state
        .units
        .iter()
        .filter(|unit| unit.hp > 0)
        .flat_map(|unit| std::iter::once(&unit.order).chain(unit.queue.iter()))
        .filter_map(|order| match order {
            Order::Salvage { building } => Some(*building),
            _ => None,
        })
        .collect();
    let mut in_flight: std::collections::BTreeMap<BuildingId, u32> =
        std::collections::BTreeMap::new();
    for bay in bays {
        let Some(source) = state.building(bay) else {
            continue;
        };
        let owner = source.player;
        let recovery_reserve = state.recovery_reserve(owner);
        let patients: Vec<crate::ids::BuildingId> = state
            .buildings
            .iter()
            .filter(|target| {
                target.id != bay
                    && !salvage_targets.contains(&target.id)
                    && target.player == owner
                    && target.built()
                    && target.hp > 0
                    && target.hp < target.stats().max_hp
                    && building_distance_sq(source, target) <= radius * radius
            })
            .map(|target| target.id)
            .collect();
        for id in patients {
            let target = state.building(id).expect("just filtered");
            let stats = target.stats();
            let target_kind = target.kind;
            let queued = in_flight.get(&id).copied().unwrap_or(0);
            let healed = (target.hp + queued).min(stats.max_hp);
            let step = crate::stats::REPAIR_BAY_STEP.min(stats.max_hp - healed);
            if step == 0 {
                continue;
            }
            let basis = if target.kind == BuildingKind::Foundry {
                crate::stats::FOUNDRY_REPAIR_PRICE
            } else {
                stats
                    .construction
                    .map_or(crate::stats::FOUNDRY_REPAIR_PRICE, |construction| {
                        construction.cost
                    })
            };
            let millis = |hp: u32| -> u64 {
                u64::from(hp) * u64::from(basis) * crate::stats::REPAIR_COST_PERMILLE
                    / u64::from(stats.max_hp)
            };
            let due = millis(healed + step).div_ceil(1000) - millis(healed).div_ceil(1000);
            let bank = state.player(owner).scrap;
            let Some(due) = u32::try_from(due).ok().filter(|&due| due <= bank) else {
                continue;
            };
            if due > 0 && recovery_reserve > 0 && bank - due < recovery_reserve {
                continue;
            }
            state.player_mut(owner).scrap = bank - due;
            in_flight.insert(id, queued + step);
            builds.push(PendingHpGain {
                starts: false,
                site: id,
                step,
                completes: false,
                player: owner,
                kind: target_kind,
                paid: due,
                repair_bay: Some(bay),
            });
        }
    }
}

fn building_distance_sq(a: &crate::state::Building, b: &crate::state::Building) -> chassis::fx::Fx {
    let on_a = a.closest_point_to(b.center());
    let on_b = b.closest_point_to(on_a);
    on_a.dist_sq(on_b)
}

/// The Crucible's smelter: each pulse, every built Crucible, in id order,
/// melts one wreck unit within its radius into one scrap for its owner. Wreck
/// is unowned, so no fog or ownership check applies.
fn crucible_smelter(state: &mut State) {
    use crate::stats::BuildingKind;
    use chassis::grid::TilePos;
    if !state
        .current_tick()
        .is_multiple_of(crate::stats::CRUCIBLE_SMELT_PERIOD)
    {
        return;
    }
    let crucibles: Vec<crate::ids::BuildingId> = state
        .buildings
        .iter()
        .filter(|b| b.built() && b.hp > 0 && b.kind == BuildingKind::Crucible)
        .map(|b| b.id)
        .collect();
    let radius = crate::stats::CRUCIBLE_SMELT_RADIUS;
    for id in crucibles {
        type FuelKey = (chassis::fx::Fx, std::cmp::Reverse<u32>, (i32, i32));
        let Some(b) = state.building(id) else {
            continue;
        };
        let owner = b.player;
        let anchor = b.anchor;
        let (w, h) = b.kind.size();
        let reach = radius.to_num::<i32>() + 1;
        // The nearest wreck feeds first, then the richer one; exact ties fall
        // to tile order in the crucible's half-turn frame, so mirrored
        // crucibles eat mirrored tiles.
        let footprint_center = |anchor: TilePos, (w, h): (i32, i32)| {
            chassis::fx::Vec2Fx::new(
                chassis::fx::Fx::from_num(anchor.x * 2 + w) / 2,
                chassis::fx::Fx::from_num(anchor.y * 2 + h) / 2,
            )
        };
        let hearth = footprint_center(anchor, (w, h));
        // A crucible sitting exactly on the map center is its own mirror
        // image, so its frame comes from the owner's earliest Foundry, the
        // one thing a half-turn does exchange.
        let map_center = chassis::fx::Vec2Fx::new(
            chassis::fx::Fx::from_num(state.map.width()) / 2,
            chassis::fx::Fx::from_num(state.map.height()) / 2,
        );
        let frame = if hearth == map_center {
            state
                .buildings
                .iter()
                .filter(|f| f.player == owner && f.kind == BuildingKind::Foundry)
                .min_by_key(|f| f.id)
                .map_or(hearth, |f| footprint_center(f.anchor, f.kind.size()))
        } else {
            hearth
        };
        let rotated = super::movement::uses_rotated_map_frame(state, frame);
        let mut fuel: Option<(FuelKey, TilePos)> = None;
        for y in (anchor.y - reach)..(anchor.y + h + reach) {
            for x in (anchor.x - reach)..(anchor.x + w + reach) {
                let tile = TilePos::new(x, y);
                let amount = state.map.wreck_at(tile);
                if amount == 0 {
                    continue;
                }
                let center = tile.center();
                let distance = b.closest_point_to(center).dist_sq(center);
                if distance > radius * radius {
                    continue;
                }
                let order = if rotated {
                    (-tile.y, -tile.x)
                } else {
                    (tile.y, tile.x)
                };
                let key = (distance, std::cmp::Reverse(amount), order);
                if fuel.as_ref().is_none_or(|(best, _)| key < *best) {
                    fuel = Some((key, tile));
                }
            }
        }
        if let Some((_, tile)) = fuel
            && state.map.extract_wreck(tile).is_some()
        {
            let bank = &mut state.player_mut(owner).scrap;
            *bank = bank.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod damage_tests;
