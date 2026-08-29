//! The tick pipeline.
//!
//! Phase order is part of the sim's contract — changing it changes game
//! outcomes (and therefore every regression hash):
//!
//! 1. **Recovery and commands** — capture any newly stranded economy's
//!    finite entitlement from the tick-boundary state, then validate and
//!    apply this tick's [`PlayerCommand`]s.
//! 2. **Production** — Foundries advance queues and spawn finished units
//!    (before brains, so a fresh unit acts on its birth tick).
//! 3. **Brains** — each unit, in id order, turns intent into action:
//!    acquiring targets, pathing, attacking, extracting, depositing. Shots
//!    are *buffered*, not applied — every machine decides against the same
//!    start-of-tick world, so seat order grants no reaction edge and
//!    mutual kills are possible.
//! 4. **Resolution** — buffered damage lands (decision order), then
//!    surviving victims retaliate against their earliest attacker.
//! 5. **Movement** — a pre-pass walks pathless ground bodies off claimed
//!    building footprints (path only — programs survive; brains may null
//!    the path each tick, so the pre-pass re-arms it), then units
//!    advance along their paths.
//! 6. **Collision** — overlapping bodies are pushed apart until they fit;
//!    units are solid to each other but never block tiles.
//! 7. **Cleanup** — entities at 0 hp are removed, with events; every
//!    death deposits wreck salvage on its ground.
//! 8. **Decay** — on its global cadence, every wreck tile loses one
//!    salvage. Cleanup and decay share the tick, so a wreck born on a
//!    cadence tick pays its first salvage immediately.
//! 9. **Vision** — every player's fog-of-war visible set is rebuilt from
//!    their surviving entities (explored only accumulates).
//! 10. **Victory** — a player with no Foundry (or who conceded) is out;
//!     last standing wins.
//!
//! After [`GameResult`] is set the world freezes: ticks still count up (so
//! timelines stay aligned) but nothing moves and commands are ignored.

mod brain;
mod commands;
mod flight;
mod movement;
mod production;
mod spatial;

use crate::command::PlayerCommand;
use crate::event::{Event, TickReport};
use crate::state::{GameResult, State};
use crate::stats::PATH_EXPANSION_CAP;
use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;
use chassis::path::astar;

/// Read-only state after the command phase of a hypothetical tick.
///
/// The view deliberately exposes only prediction-safe queries. It cannot be
/// converted into a [`State`], serialized, hashed, or installed as an
/// authoritative session with a stale tick, vision table, or result.
pub struct CommandPhaseView<'a> {
    state: &'a State,
}

impl CommandPhaseView<'_> {
    /// Projected tick, unchanged because later phases never ran.
    pub fn current_tick(&self) -> crate::Tick {
        self.state.current_tick()
    }

    /// Whether `player` may issue another command after the projected batch.
    pub fn accepts_commands(&self, player: crate::ids::PlayerId) -> bool {
        self.state.result.is_none()
            && self
                .state
                .try_player(player)
                .is_some_and(|seat| !seat.resigned)
            && self.state.buildings.iter().any(|building| {
                building.player == player && building.kind == crate::stats::BuildingKind::Foundry
            })
    }

    /// Projected scrap in `player`'s bank.
    pub fn scrap(&self, player: crate::ids::PlayerId) -> Option<u32> {
        self.state.try_player(player).map(|seat| seat.scrap)
    }

    /// Projected live units, in canonical id order.
    pub fn units(&self) -> &[crate::state::Unit] {
        self.state.units()
    }

    /// One projected live unit.
    pub fn unit(&self, id: crate::ids::UnitId) -> Option<&crate::state::Unit> {
        self.state.unit(id)
    }

    /// Whether projected commands already paid for a matching ordinary site.
    ///
    /// A deferred founder joins that unfinished site for free when it
    /// arrives, so callers must not reserve the construction price again.
    pub fn has_own_unfinished_site(
        &self,
        player: crate::ids::PlayerId,
        kind: crate::stats::BuildingKind,
        anchor: TilePos,
    ) -> bool {
        self.state.buildings.iter().any(|building| {
            building.player == player
                && building.kind == kind
                && building.anchor == anchor
                && !building.built
                && building.tier == 0
        })
    }

    /// Fog-safe projected placement verdict, excluding claims carried by
    /// workers whose programs the candidate command will replace.
    pub fn place_intent_refusal_replacing(
        &self,
        player: crate::ids::PlayerId,
        kind: crate::stats::BuildingKind,
        anchor: TilePos,
        units: &[crate::ids::UnitId],
    ) -> Option<crate::state::PlaceRefusal> {
        self.state
            .place_intent_refusal_replacing(player, kind, anchor, units)
    }
}

/// Whether this seat is stranded and eligible for Foundry recovery income.
///
/// Keep every automatic consumer of the recovery reserve on this one
/// predicate: a living, completed Foundry can rebuild an economy only when
/// its owner has neither a Harvester in the world nor one prepaid in a live
/// production queue.
fn harvester_recovery_needed(state: &State, player: crate::ids::PlayerId) -> bool {
    state.harvester_recovery_needed(player)
}

impl State {
    /// Inspects a private clone after applying only phase-one commands.
    ///
    /// This is the non-authoritative prediction seam for shells and tools:
    /// command validation, ordering, charges, sites, and unit programs match
    /// [`State::tick`], but production, brains, movement, cleanup, vision,
    /// victory, and the tick counter do not advance. The receiver is never
    /// mutated; only [`State::tick`] advances an authoritative session.
    pub fn inspect_command_phase<R>(
        &self,
        commands: &[PlayerCommand],
        inspect: impl FnOnce(CommandPhaseView<'_>) -> R,
    ) -> R {
        let mut projected = self.clone();
        if projected.result.is_none() {
            let mut events = Vec::new();
            commands::apply(&mut projected, commands, &mut events);
        }
        inspect(CommandPhaseView { state: &projected })
    }

    /// Advances the world by one fixed timestep, applying `commands` (all
    /// stamped for this tick). The returned report is presentation data —
    /// dropping it never affects the sim.
    pub fn tick(&mut self, commands: &[PlayerCommand]) -> TickReport {
        let tick = self.tick;
        let mut events = Vec::new();
        if self.result.is_none() {
            // One spatial index serves the tick's unit-neighborhood
            // queries (acquisition windows, collision pairs). A scratch
            // local on purpose: the pipeline rebuilds it at each use
            // point, and it must never ride on `State` (see `spatial`).
            let mut index = spatial::UnitIndex::new();
            production::capture_recovery_entitlements(self);
            commands::apply(self, commands, &mut events);
            production::run(self, &mut events);
            production::decay_abandoned_sites(self);
            let boardings = brain::run(self, &mut index, &mut events);
            // Embarkations and landings mutate the unit list, which must
            // hold still under the brains; they land here, between the
            // last decision and the first movement.
            brain::logistics::resolve(self, boardings, &mut events);
            movement::evict_claimed_ground(self);
            let travel = movement::run(self);
            movement::resolve_collisions(self, &travel, &mut index);
            detonate_charges(self, &mut events);
            cleanup(self, &mut events);
            if self.tick.is_multiple_of(crate::stats::WRECK_DECAY_TICKS) {
                self.map.decay_wrecks();
            }
            self.refresh_vision();
            victory(self, &mut events);
        }
        self.tick += 1;
        TickReport { tick, events }
    }
}

/// Buried charges under hostile treads go off — after movement, so the
/// step onto the trigger and the blast share a tick. Charges detonate
/// in id order against post-movement positions; a charge zeroed by
/// combat (or by an earlier blast — mines never sympathetically
/// detonate, they are simply destroyed) no longer fires. The blast
/// hits every hostile ground machine in the ring and every hostile
/// buried charge (the splash-vulnerability rule), and cleanup sweeps
/// the casualties in the same tick.
fn detonate_charges(state: &mut State, events: &mut Vec<Event>) {
    use crate::stats::{BuildingKind, CHARGE_BLAST_RADIUS, CHARGE_DAMAGE, CHARGE_TRIGGER_RADIUS};
    let trigger_sq = CHARGE_TRIGGER_RADIUS * CHARGE_TRIGGER_RADIUS;
    let blast_sq = CHARGE_BLAST_RADIUS * CHARGE_BLAST_RADIUS;
    for slot in 0..state.buildings.len() {
        let b = &state.buildings[slot];
        if b.kind != BuildingKind::ScuttleCharge || !b.built || b.hp == 0 {
            continue;
        }
        let (id, owner, center) = (b.id, b.player, b.center());
        let tripped = state.units.iter().any(|u| {
            u.hp > 0
                && state.hostile(owner, u.player)
                && u.kind.stats().domain == crate::stats::Domain::Ground
                && u.pos.dist_sq(center) <= trigger_sq
        });
        if !tripped {
            continue;
        }
        state.buildings[slot].hp = 0;
        events.push(Event::ChargeDetonated {
            building: id,
            player: owner,
            at: center,
        });
        for u in state.units.iter_mut() {
            if u.hp > 0
                && state.players[owner.0 as usize].team != state.players[u.player.0 as usize].team
                && u.kind.stats().domain == crate::stats::Domain::Ground
                && u.pos.dist_sq(center) <= blast_sq
            {
                u.hp = u.hp.saturating_sub(CHARGE_DAMAGE);
            }
        }
        for other in 0..state.buildings.len() {
            if other == slot {
                continue;
            }
            let ob = &state.buildings[other];
            if ob.hp > 0
                && ob.kind.is_stealthy()
                && state.players[owner.0 as usize].team != state.players[ob.player.0 as usize].team
                && ob.center().dist_sq(center) <= blast_sq
            {
                state.buildings[other].hp = ob.hp.saturating_sub(CHARGE_DAMAGE);
            }
        }
    }
}

/// Removes entities that hit 0 hp this tick, reporting each — and leaves
/// their price on the ground: a fraction of every destroyed machine's
/// cost lands as wreck salvage (buildings split theirs across the
/// footprint). Battles literally feed the salvagers.
fn cleanup(state: &mut State, events: &mut Vec<Event>) {
    let mut deposits: Vec<(TilePos, u32)> = Vec::new();
    for unit in state.units.iter().filter(|u| u.hp == 0) {
        events.push(Event::UnitDied {
            unit: unit.id,
            kind: unit.kind,
            player: unit.player,
            pos: unit.pos,
        });
        let value =
            unit.kind.stats().cost * crate::stats::WRECK_VALUE_NUM / crate::stats::WRECK_VALUE_DEN;
        deposits.push((unit.tile(), value));
        // Cargo dies with the airframe, and its price falls at the
        // crash tile with everything else (a crash over the Pit is
        // swallowed by the standing wreck rule).
        for rider in &unit.cargo {
            events.push(Event::UnitDied {
                unit: rider.id,
                kind: rider.kind,
                player: rider.player,
                pos: unit.pos,
            });
            let value = rider.kind.stats().cost * crate::stats::WRECK_VALUE_NUM
                / crate::stats::WRECK_VALUE_DEN;
            deposits.push((unit.tile(), value));
        }
    }
    state.units.retain(|u| u.hp > 0);

    let mut queue_refunds: Vec<(crate::ids::PlayerId, u32)> = Vec::new();
    for building in state.buildings.iter().filter(|b| b.hp == 0) {
        // A salvaged building came apart on purpose: no wreck, no
        // destruction event, and its prepaid production queue refunds
        // in full (training spends only time — the CancelTrain rule,
        // applied to the whole line at once).
        if building.salvaged {
            events.push(Event::BuildingSalvaged {
                building: building.id,
                player: building.player,
                pos: building.center(),
                refund: building.salvage_credited,
            });
            let prepaid: u32 = building.queue.iter().map(|k| k.stats().cost).sum();
            if prepaid > 0 {
                queue_refunds.push((building.player, prepaid));
            }
            continue;
        }
        events.push(Event::BuildingDestroyed {
            building: building.id,
            player: building.player,
            pos: building.center(),
        });
        let stats = building.stats();
        let price = stats
            .construction
            .map_or(crate::stats::FOUNDRY_WRECK_VALUE, |c| c.cost);
        let value = price * crate::stats::WRECK_VALUE_NUM / crate::stats::WRECK_VALUE_DEN;
        let tiles = (stats.size.0 * stats.size.1) as u32;
        for tile in building.tiles() {
            deposits.push((tile, value / tiles));
        }
    }
    for index in 0..state.buildings.len() {
        if state.buildings[index].hp == 0 {
            state.stamp_building_occupancy(index, false);
        }
    }
    state.buildings.retain(|b| b.hp > 0);
    for (player, prepaid) in queue_refunds {
        let bank = &mut state.player_mut(player).scrap;
        *bank = bank.saturating_add(prepaid);
    }

    for (tile, value) in deposits {
        // A tile under a surviving building swallows its deposit — a
        // flyer downed over a roof leaves nothing strippable, and wreck
        // must never coexist with a standing footprint (harvesters
        // cannot reach it, and the building's own eventual wreck would
        // double-stack). Buildings that died this tick are already gone
        // from the vec, so their footprints take deposits normally.
        if state.buildings.iter().any(|b| b.contains(tile)) {
            continue;
        }
        // Rock and peaks never open up, so salvage there is bait no
        // harvester can ever strip — a downed flyer's value is simply
        // lost. Scrap node tiles keep their deposits: they become
        // standable the moment the node exhausts.
        if state
            .map
            .tile(tile)
            .is_none_or(|t| t.terrain != crate::map::Terrain::Ground)
        {
            continue;
        }
        state.map.add_wreck(tile, value);
    }
}

/// Declares the result once at least one team has been eliminated.
///
/// Elimination is Foundry-based: a team lives while *any* of its seats
/// holds a Foundry — no Foundry anywhere, no comeback; turrets and
/// factories left standing do not keep a team in the game.
/// A resigned seat's Foundries stop counting the tick it concedes, so
/// a fully-resigned team is eliminated on the spot. The per-seat
/// command gate in `commands::apply` deliberately stays player-scoped:
/// a foundry-less or resigned seat on a living team spectates while
/// its team plays on.
fn victory(state: &mut State, events: &mut Vec<Event>) {
    if state.result.is_some() {
        return;
    }
    // Stamp each seat's first tick out of the match — resigned, or
    // holding no Foundry at all (sites count). Recorded once, never
    // cleared: the FFA scoreboard's placement key.
    for index in 0..state.players.len() {
        if state.players[index].eliminated_at.is_some() {
            continue;
        }
        let seat = crate::ids::PlayerId(index as u8);
        let out = state.players[index].resigned
            || !state
                .buildings
                .iter()
                .any(|b| b.player == seat && b.kind == crate::stats::BuildingKind::Foundry);
        if out {
            state.players[index].eliminated_at = Some(state.tick);
        }
    }
    let mut teams: Vec<u8> = state.players.iter().map(|p| p.team).collect();
    teams.sort_unstable();
    teams.dedup();
    let alive = |team: u8| {
        state.buildings.iter().any(|b| {
            let owner = &state.players[b.player.0 as usize];
            b.kind == crate::stats::BuildingKind::Foundry && owner.team == team && !owner.resigned
        })
    };
    let survivors: Vec<u8> = teams.iter().copied().filter(|&t| alive(t)).collect();
    if survivors.len() == teams.len() {
        return;
    }
    let result = match survivors.as_slice() {
        [] => GameResult::Draw,
        [team] => GameResult::Victory { team: *team },
        _ => return, // multiple teams standing — play on
    };
    state.result = Some(result);
    events.push(Event::GameOver { result });
}

/// A* against the current world (terrain + buildings).
pub(crate) fn astar_for(state: &State, from: TilePos, to: TilePos) -> Option<Vec<TilePos>> {
    astar(
        state.map.width(),
        state.map.height(),
        from,
        to,
        |p| state.passable(p),
        PATH_EXPANSION_CAP,
    )
}

/// A route for a unit of the given kind: ground units A* around the
/// world; air units fly the straight line — one waypoint, landed exactly —
/// unless a peak stands in it, in which case they A* over air passability
/// (peaks are the only thing the sky routes around).
pub(crate) fn route_for(
    state: &State,
    kind: crate::stats::UnitKind,
    from: TilePos,
    to: TilePos,
) -> Option<Vec<TilePos>> {
    route_for_position(state, kind, from.center(), to)
}

/// A route traced from a unit's exact position. Most ground routing starts
/// at a tile center, but a wide-turn aircraft can meet a peak near one edge
/// of its current tile even when the center-to-center segment looks clear.
pub(crate) fn route_for_position(
    state: &State,
    kind: crate::stats::UnitKind,
    from: Vec2Fx,
    to: TilePos,
) -> Option<Vec<TilePos>> {
    let from_tile = TilePos::containing(from);
    match kind.stats().domain {
        crate::stats::Domain::Ground => astar_for(state, from_tile, to),
        crate::stats::Domain::Air => {
            // Goals ring-snap off peaks here, at the one funnel every
            // air route passes: group orders pre-snap via spread_goals,
            // but patrol waypoints and rally tiles arrive raw — and
            // line_blocked ignores endpoints by design, so an unsnapped
            // peak goal would hand the flyer the mountain itself.
            let to = if state.passable_for(crate::stats::Domain::Air, to) {
                to
            } else {
                snap_air_goal(state, to)?
            };
            let sky_open = |t: TilePos| {
                state
                    .map
                    .tile(t)
                    .is_none_or(|tile| !tile.terrain.blocks_air())
            };
            if !chassis::path::line_blocked(from, to.center(), sky_open) {
                return Some(vec![to]);
            }
            astar(
                state.map.width(),
                state.map.height(),
                from_tile,
                to,
                |p| state.passable_for(crate::stats::Domain::Air, p),
                PATH_EXPANSION_CAP,
            )
        }
    }
}

/// The nearest air-passable tile to `goal`, ring-scanned outward in the
/// same deterministic order group goals use. `None` when nothing within
/// reach is open sky (a map that is all mountain has bigger problems).
fn snap_air_goal(state: &State, goal: TilePos) -> Option<TilePos> {
    for r in 0..=crate::stats::GOAL_SNAP_RADIUS + 3 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let t = goal.offset(dx, dy);
                if state.passable_for(crate::stats::Domain::Air, t) {
                    return Some(t);
                }
            }
        }
    }
    None
}

/// The ring of tiles surrounding a rectangle, row-major (deterministic).
pub(crate) fn rect_adjacent_tiles(
    anchor: TilePos,
    size: (i32, i32),
) -> impl Iterator<Item = TilePos> {
    let (w, h) = size;
    (-1..=h).flat_map(move |dy| {
        (-1..=w)
            .map(move |dx| anchor.offset(dx, dy))
            .filter(move |t| {
                let inside = t.x > anchor.x - 1
                    && t.x < anchor.x + w
                    && t.y > anchor.y - 1
                    && t.y < anchor.y + h;
                !inside
            })
    })
}

/// A doorstep's stable key in the approaching body's local frame.
///
/// The dot and cross products are unchanged by a 180-degree rotation of both
/// the footprint and body. This makes a mirrored approach choose a mirrored
/// candidate instead of inheriting the absolute row-major scan direction.
/// Coordinates are doubled so even-sized footprint centers stay exact.
pub(crate) fn rect_approach_key(
    from: TilePos,
    anchor: TilePos,
    size: (i32, i32),
    candidate: TilePos,
) -> (i32, std::cmp::Reverse<i64>, i64) {
    rect_approach_key_from(from, from, anchor, size, candidate)
}

pub(crate) fn rect_approach_key_from(
    from: TilePos,
    approach_from: TilePos,
    anchor: TilePos,
    size: (i32, i32),
    candidate: TilePos,
) -> (i32, std::cmp::Reverse<i64>, i64) {
    let center_x = i64::from(anchor.x) * 2 + i64::from(size.0);
    let center_y = i64::from(anchor.y) * 2 + i64::from(size.1);
    let approach_x = i64::from(approach_from.x) * 2 + 1 - center_x;
    let approach_y = i64::from(approach_from.y) * 2 + 1 - center_y;
    let candidate_x = i64::from(candidate.x) * 2 + 1 - center_x;
    let candidate_y = i64::from(candidate.y) * 2 + 1 - center_y;
    let dot = approach_x * candidate_x + approach_y * candidate_y;
    let cross = approach_x * candidate_y - approach_y * candidate_x;
    (candidate.chebyshev(from), std::cmp::Reverse(dot), cross)
}

/// A nonzero local frame for doorstep ties. Most bodies supply their own
/// approach ray. A body exactly at the center of an odd footprint has no ray,
/// so use the home-side corner of its earliest Foundry instead; mirrored
/// owners then leave a newly claimed footprint through mirrored doorsteps.
pub(crate) fn rect_approach_origin(
    state: &State,
    player: crate::ids::PlayerId,
    from: TilePos,
    anchor: TilePos,
    size: (i32, i32),
) -> TilePos {
    let center_x = i64::from(anchor.x) * 2 + i64::from(size.0);
    let center_y = i64::from(anchor.y) * 2 + i64::from(size.1);
    let from_x = i64::from(from.x) * 2 + 1;
    let from_y = i64::from(from.y) * 2 + 1;
    if from_x != center_x || from_y != center_y {
        return from;
    }

    if let Some(foundry) = state
        .buildings
        .iter()
        .filter(|building| {
            building.player == player && building.kind == crate::stats::BuildingKind::Foundry
        })
        .min_by_key(|building| building.id)
    {
        let foundry_size = foundry.kind.base_stats().size;
        let flip_x = i64::from(foundry.anchor.x) * 2 + i64::from(foundry_size.0)
            >= i64::from(state.map.width());
        let flip_y = i64::from(foundry.anchor.y) * 2 + i64::from(foundry_size.1)
            >= i64::from(state.map.height());
        return foundry.anchor.offset(
            if flip_x { foundry_size.0 - 1 } else { 0 },
            if flip_y { foundry_size.1 - 1 } else { 0 },
        );
    }

    let flip_x = if center_x == i64::from(state.map.width()) {
        player.0 % 2 == 1
    } else {
        center_x > i64::from(state.map.width())
    };
    let flip_y = if center_y == i64::from(state.map.height()) {
        player.0 % 2 == 1
    } else {
        center_y > i64::from(state.map.height())
    };
    TilePos::new(
        if flip_x { state.map.width() - 1 } else { 0 },
        if flip_y { state.map.height() - 1 } else { 0 },
    )
}

/// The footprint tile nearest an impact, with ties resolved in the incoming
/// attack's local frame.
///
/// Even-sized footprints have no single center tile. Flooring their geometric
/// center therefore chooses an absolute southeast tile and breaks half-turn
/// symmetry. Distance keeps the warning on the part of the destroyed
/// footprint nearest the impact; cross and dot products choose one of the
/// equally near tiles without importing row-major world direction. Callers
/// must supply a nonzero approach vector that rotates with the attack.
pub(crate) fn footprint_incident_tile(
    anchor: TilePos,
    size: (i32, i32),
    impact: chassis::fx::Vec2Fx,
    approach: chassis::fx::Vec2Fx,
) -> TilePos {
    use std::cmp::Reverse;

    assert!(size.0 > 0 && size.1 > 0, "footprints have positive size");
    assert_ne!(
        approach,
        chassis::fx::Vec2Fx::ZERO,
        "an incident tie needs an attack-relative direction"
    );

    let center_x = i64::from(anchor.x) * 2 + i64::from(size.0);
    let center_y = i64::from(anchor.y) * 2 + i64::from(size.1);
    let approach_x = i128::from(approach.x.to_bits());
    let approach_y = i128::from(approach.y.to_bits());

    (0..size.1)
        .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
        .min_by_key(|tile| {
            let candidate_x = i128::from(i64::from(tile.x) * 2 + 1 - center_x);
            let candidate_y = i128::from(i64::from(tile.y) * 2 + 1 - center_y);
            let cross = approach_x * candidate_y - approach_y * candidate_x;
            let dot = approach_x * candidate_x + approach_y * candidate_y;
            (tile.center().dist_sq(impact), cross, Reverse(dot))
        })
        .expect("positive footprint contains a tile")
}

/// Whether `tile` touches (including diagonally) but does not overlap the
/// rectangle at `anchor`.
pub(crate) fn tile_adjacent_to_rect(tile: TilePos, anchor: TilePos, size: (i32, i32)) -> bool {
    let (w, h) = size;
    let inside =
        tile.x >= anchor.x && tile.y >= anchor.y && tile.x < anchor.x + w && tile.y < anchor.y + h;
    if inside {
        return false;
    }
    tile.x >= anchor.x - 1
        && tile.y >= anchor.y - 1
        && tile.x <= anchor.x + w
        && tile.y <= anchor.y + h
}

/// The tile a commanded goal actually means to one movement domain:
/// ground snaps to the nearest walkable tile, air clamps onto the map —
/// any tile flies, rock included.
pub(crate) fn domain_goal(
    state: &State,
    goal: TilePos,
    domain: crate::stats::Domain,
) -> Option<TilePos> {
    match domain {
        crate::stats::Domain::Ground => {
            find_nearby_passable(state, goal, crate::stats::GOAL_SNAP_RADIUS)
        }
        crate::stats::Domain::Air => {
            // Clamp to the map, then off any peak: this is the funnel
            // patrol waypoints and rally orders lower through, and a
            // stored peak goal deadlocks the flyer — it reaches the
            // route's snapped endpoint, compares against the original
            // order goal, and repaths to the same tile forever.
            let clamped = TilePos::new(
                goal.x.clamp(0, state.map.width() - 1),
                goal.y.clamp(0, state.map.height() - 1),
            );
            if state.passable_for(crate::stats::Domain::Air, clamped) {
                Some(clamped)
            } else {
                snap_air_goal(state, clamped)
            }
        }
    }
}

/// The nearest passable tile to `goal` within `radius`, scanning rings
/// outward, row-major within a ring — a deterministic "snap to walkable".
pub(crate) fn find_nearby_passable(state: &State, goal: TilePos, radius: i32) -> Option<TilePos> {
    for r in 0..=radius {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let t = goal.offset(dx, dy);
                if state.passable(t) {
                    return Some(t);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_phase_inspection_is_pure_and_stops_before_the_tick() {
        let state = crate::Scenario::skirmish()
            .build()
            .expect("embedded skirmish builds");
        let before = state.clone();
        let worker = state
            .units()
            .iter()
            .find(|unit| {
                unit.player == crate::PlayerId(0) && unit.kind == crate::UnitKind::Harvester
            })
            .expect("skirmish authors a worker")
            .id;
        let kind = crate::BuildingKind::Turret;
        let anchor = TilePos::new(10, 4);
        let cost = kind
            .base_stats()
            .construction
            .expect("turret is constructible")
            .cost;
        let command = PlayerCommand {
            player: crate::PlayerId(0),
            command: crate::Command::Build {
                units: vec![worker],
                kind,
                anchor,
                queue: false,
                defer: false,
            },
        };

        state.inspect_command_phase(&[command], |projected| {
            assert_eq!(projected.current_tick(), state.current_tick());
            assert_eq!(
                projected.scrap(crate::PlayerId(0)),
                Some(state.player(crate::PlayerId(0)).scrap - cost)
            );
            assert_eq!(
                projected.place_intent_refusal_replacing(crate::PlayerId(0), kind, anchor, &[]),
                Some(crate::PlaceRefusal::Building),
                "the projected site owns its footprint"
            );
            assert!(matches!(
                projected.unit(worker).expect("worker remains").order,
                crate::Order::Build { .. }
            ));
        });
        assert_eq!(state, before, "inspection never mutates its source");
    }

    #[test]
    fn command_phase_inspection_honors_a_frozen_result() {
        let mut state = crate::Scenario::skirmish()
            .build()
            .expect("embedded skirmish builds");
        state.result = Some(GameResult::Draw);
        let before = state.clone();

        state.inspect_command_phase(
            &[PlayerCommand {
                player: crate::PlayerId(0),
                command: crate::Command::Surrender,
            }],
            |projected| {
                assert!(!projected.accepts_commands(crate::PlayerId(0)));
                assert_eq!(
                    projected.scrap(crate::PlayerId(0)),
                    Some(state.player(crate::PlayerId(0)).scrap)
                );
            },
        );
        assert_eq!(state, before);
    }

    #[test]
    fn rect_ring_has_expected_size_and_order() {
        // 2x2 rect → 12-tile ring.
        let ring: Vec<TilePos> = rect_adjacent_tiles(TilePos::new(5, 5), (2, 2)).collect();
        assert_eq!(ring.len(), 12);
        assert_eq!(
            ring[0],
            TilePos::new(4, 4),
            "row-major: top-left corner first"
        );
        assert!(
            ring.iter()
                .all(|t| tile_adjacent_to_rect(*t, TilePos::new(5, 5), (2, 2)))
        );
    }

    #[test]
    fn footprint_incident_tiles_are_half_turn_equivariant() {
        use chassis::fx::{Fx, Vec2Fx};

        let (map_width, map_height) = (48, 30);
        let rotate_tile =
            |tile: TilePos| TilePos::new(map_width - 1 - tile.x, map_height - 1 - tile.y);
        let rotate_point = |point: Vec2Fx| {
            Vec2Fx::new(
                Fx::from_num(map_width) - point.x,
                Fx::from_num(map_height) - point.y,
            )
        };
        let cases = [
            (
                "northeast impact",
                TilePos::new(8, 5),
                (2, 2),
                Vec2Fx::new(Fx::from_num(10), Fx::from_num(5)),
                Vec2Fx::new(Fx::ONE, -Fx::ONE),
            ),
            (
                "southwest impact",
                TilePos::new(31, 20),
                (2, 2),
                Vec2Fx::new(Fx::from_num(31), Fx::from_num(22)),
                Vec2Fx::new(-Fx::ONE, Fx::ONE),
            ),
            (
                "wide even footprint",
                TilePos::new(12, 11),
                (4, 2),
                Vec2Fx::new(Fx::from_num(14), Fx::from_num(11)),
                Vec2Fx::new(Fx::ZERO, Fx::ONE),
            ),
        ];

        for (name, anchor, size, impact, approach) in cases {
            let mirrored_anchor = TilePos::new(
                map_width - size.0 - anchor.x,
                map_height - size.1 - anchor.y,
            );
            let tile = footprint_incident_tile(anchor, size, impact, approach);
            let mirrored =
                footprint_incident_tile(mirrored_anchor, size, rotate_point(impact), -approach);
            let inside = |tile: TilePos, anchor: TilePos| {
                tile.x >= anchor.x
                    && tile.x < anchor.x + size.0
                    && tile.y >= anchor.y
                    && tile.y < anchor.y + size.1
            };
            assert!(inside(tile, anchor), "{name}: warning left its footprint");
            assert!(
                inside(mirrored, mirrored_anchor),
                "{name}: mirrored warning left its footprint"
            );
            assert_eq!(rotate_tile(tile), mirrored, "{name}");
        }

        let axis_anchor = TilePos::new(23, 5);
        let axis_impact = Vec2Fx::new(Fx::from_num(24), Fx::from_num(5));
        let axis_tile = footprint_incident_tile(
            axis_anchor,
            (2, 2),
            axis_impact,
            Vec2Fx::new(Fx::ZERO, Fx::ONE),
        );
        assert_eq!(
            axis_tile,
            TilePos::new(24, 5),
            "cross-product tie selects the same attack-local side on the map axis"
        );
        let mirrored_axis_anchor = TilePos::new(23, 23);
        let mirrored_axis_tile = footprint_incident_tile(
            mirrored_axis_anchor,
            (2, 2),
            rotate_point(axis_impact),
            Vec2Fx::new(Fx::ZERO, -Fx::ONE),
        );
        assert_eq!(rotate_tile(axis_tile), mirrored_axis_tile);
    }

    #[test]
    fn mirrored_lethal_hits_record_mirrored_footprint_incidents() {
        use crate::scenario::{BuildingSpec, UnitSpec};
        use crate::{BuildingKind, Order, PlayerId, Target, UnitKind};

        let mut scenario = calibration_open_cupric();
        scenario.units = vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 18,
                y: 16,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 29,
                y: 13,
            },
        ];
        let left_anchor = TilePos::new(25, 13);
        let right_anchor = TilePos::new(21, 15);
        scenario.buildings = vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: left_anchor.x,
                y: left_anchor.y,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Fabricator,
                x: right_anchor.x,
                y: right_anchor.y,
            },
        ];
        let mut state = scenario.build().expect("the mirrored volley builds");
        let victim = |state: &State, player, anchor| {
            state
                .buildings
                .iter()
                .find(|building| building.player == player && building.anchor == anchor)
                .expect("the victim exists")
                .id
        };
        let left_victim = victim(&state, PlayerId(0), left_anchor);
        let right_victim = victim(&state, PlayerId(1), right_anchor);
        let damage = UnitKind::Sentinel.stats().weapons[0].damage;
        state.building_mut(left_victim).expect("left victim").hp = damage;
        state.building_mut(right_victim).expect("right victim").hp = damage;
        for (player, target) in [
            (PlayerId(0), Target::Building(right_victim)),
            (PlayerId(1), Target::Building(left_victim)),
        ] {
            state
                .units
                .iter_mut()
                .find(|unit| unit.player == player)
                .expect("the mirrored shooter exists")
                .order = Order::Attack {
                target,
                resume: None,
            };
        }

        let report = state.tick(&[]);
        assert!(
            report.events.iter().any(|event| matches!(
                event,
                crate::Event::BuildingDestroyed { building, .. } if *building == left_victim
            )),
            "the west victim dies in the mirrored volley"
        );
        assert!(
            report.events.iter().any(|event| matches!(
                event,
                crate::Event::BuildingDestroyed { building, .. } if *building == right_victim
            )),
            "the east victim dies in the mirrored volley"
        );
        let left = state.vision(PlayerId(0)).salvage_incidents();
        let right = state.vision(PlayerId(1)).salvage_incidents();
        assert_eq!(left.len(), 1);
        assert_eq!(right.len(), 1);
        let inside = |tile: TilePos, anchor: TilePos| {
            tile.x >= anchor.x
                && tile.x < anchor.x + 2
                && tile.y >= anchor.y
                && tile.y < anchor.y + 2
        };
        assert!(inside(left[0].tile, left_anchor));
        assert!(inside(right[0].tile, right_anchor));
        assert_eq!(mirror_tile(&state, left[0].tile), right[0].tile);
        assert_eq!(left[0].expires_at, right[0].expires_at);
    }

    #[test]
    fn adjacency_excludes_inside_and_far() {
        let anchor = TilePos::new(3, 3);
        assert!(!tile_adjacent_to_rect(TilePos::new(3, 3), anchor, (2, 2)));
        assert!(!tile_adjacent_to_rect(TilePos::new(4, 4), anchor, (2, 2)));
        assert!(tile_adjacent_to_rect(TilePos::new(2, 2), anchor, (2, 2)));
        assert!(tile_adjacent_to_rect(TilePos::new(5, 4), anchor, (2, 2)));
        assert!(!tile_adjacent_to_rect(TilePos::new(6, 4), anchor, (2, 2)));
    }

    fn calibration_open_cupric() -> crate::Scenario {
        use crate::scenario::{PlayerSpec, UnitSpec};
        use crate::{Faction, UnitKind};

        crate::Scenario {
            name: "Calibration Open - Cupric".into(),
            seed: 1_616_101,
            map: [
                "################################################",
                "#..............................................#",
                "#..............................................#",
                "#.......ss.....................................#",
                "#..............................................#",
                "#....1....E....##..............................#",
                "#..............................................#",
                "#..............................................#",
                "#...................#..........................#",
                "#...........s.......#..........................#",
                "#.............s................................#",
                "#................E.............................#",
                "#..............................................#",
                "#..................S...........................#",
                "#..............................................#",
                "#..............................................#",
                "#...........................S..................#",
                "#............................E.................#",
                "#..............................................#",
                "#................................s.............#",
                "#..........................#.......s...........#",
                "#..........................#...................#",
                "#..............................................#",
                "#...................................E....2.....#",
                "#..............................##..............#",
                "#..............................................#",
                "#.....................................ss.......#",
                "#..............................................#",
                "#..............................................#",
                "################################################",
            ]
            .map(str::to_owned)
            .into(),
            players: ["West Cupric", "East Cupric"]
                .map(|name| PlayerSpec {
                    name: name.into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 150,
                    bot: false,
                    bot_config: None,
                })
                .into(),
            units: [
                (0, UnitKind::Harvester, 6, 8),
                (0, UnitKind::Harvester, 7, 8),
                (0, UnitKind::Harvester, 8, 7),
                (0, UnitKind::Sentinel, 10, 8),
                (1, UnitKind::Harvester, 41, 21),
                (1, UnitKind::Harvester, 40, 21),
                (1, UnitKind::Harvester, 39, 22),
                (1, UnitKind::Sentinel, 37, 21),
            ]
            .map(|(player, kind, x, y)| UnitSpec { player, kind, x, y })
            .into(),
            buildings: Vec::new(),
            meta: None,
        }
    }

    fn calibration_open_tick_zero_commands() -> Vec<PlayerCommand> {
        use crate::{BuildingId, Command, PlayerId, UnitId, UnitKind};

        let mut commands = Vec::new();
        for (player, units, node) in [
            (0, [0, 1, 2], TilePos::new(8, 3)),
            (1, [4, 5, 6], TilePos::new(39, 26)),
        ] {
            commands.extend(units.map(|unit| PlayerCommand {
                player: PlayerId(player),
                command: Command::Harvest {
                    units: vec![UnitId(unit)],
                    node,
                    queue: false,
                },
            }));
            commands.push(PlayerCommand {
                player: PlayerId(player),
                command: Command::Train {
                    building: BuildingId(player as u32),
                    kind: UnitKind::Harvester,
                },
            });
            commands.push(PlayerCommand {
                player: PlayerId(player),
                command: Command::AttackMove {
                    units: vec![UnitId(if player == 0 { 3 } else { 7 })],
                    goal: if player == 0 {
                        TilePos::new(8, 8)
                    } else {
                        TilePos::new(39, 21)
                    },
                    queue: false,
                },
            });
        }
        commands
    }

    fn mirror_tile(state: &State, tile: TilePos) -> TilePos {
        TilePos::new(
            state.map.width() - 1 - tile.x,
            state.map.height() - 1 - tile.y,
        )
    }

    fn assert_calibration_open_symmetry(
        stage: &str,
        state: &State,
        unit_pairs: &[(crate::UnitId, crate::UnitId)],
    ) {
        use crate::Order;
        use chassis::fx::{Fx, Vec2Fx};

        for y in 0..state.map.height() {
            for x in 0..state.map.width() {
                let tile = TilePos::new(x, y);
                assert_eq!(
                    state.map.tile(tile),
                    state.map.tile(mirror_tile(state, tile)),
                    "{stage}: map tile {tile:?}"
                );
            }
        }
        assert_eq!(
            state.players[0].scrap, state.players[1].scrap,
            "{stage}: scrap"
        );
        for &(left_id, right_id) in unit_pairs {
            let left = state.unit(left_id).expect("left unit exists");
            let right = state.unit(right_id).expect("right unit exists");
            assert_eq!(left.player, crate::PlayerId(0), "{stage}: left owner");
            assert_eq!(right.player, crate::PlayerId(1), "{stage}: right owner");
            assert_eq!(left.kind, right.kind, "{stage}: unit kind {left_id}");
            assert_eq!(left.hp, right.hp, "{stage}: unit hp {left_id}");
            assert_eq!(left.carrying, right.carrying, "{stage}: cargo {left_id}");
            assert_eq!(left.progress, right.progress, "{stage}: progress {left_id}");
            let mirrored_pos = Vec2Fx::new(
                Fx::from_num(state.map.width()) - left.pos.x,
                Fx::from_num(state.map.height()) - left.pos.y,
            );
            assert_eq!(mirrored_pos, right.pos, "{stage}: unit position {left_id}");

            match (left.order, right.order) {
                (
                    Order::Harvest {
                        node: left_node,
                        anchor: left_anchor,
                        retiring: left_retiring,
                    },
                    Order::Harvest {
                        node: right_node,
                        anchor: right_anchor,
                        retiring: right_retiring,
                    },
                ) => {
                    assert_eq!(mirror_tile(state, left_node), right_node, "{stage}: node");
                    assert_eq!(
                        left_anchor.map(|tile| mirror_tile(state, tile)),
                        right_anchor,
                        "{stage}: anchor"
                    );
                    assert_eq!(left_retiring, right_retiring, "{stage}: retirement");
                }
                (Order::AttackMove { goal: left_goal }, Order::AttackMove { goal: right_goal }) => {
                    assert_eq!(
                        mirror_tile(state, left_goal),
                        right_goal,
                        "{stage}: attack-move goal"
                    )
                }
                (Order::Idle, Order::Idle) => {}
                orders => panic!("{stage}: orders are not paired: {orders:?}"),
            }

            match (&left.path, &right.path) {
                (None, None) => {}
                (Some(left_path), Some(right_path)) => {
                    assert_eq!(
                        mirror_tile(state, left_path.goal),
                        right_path.goal,
                        "{stage}: path goal {left_id}"
                    );
                    let mirrored: Vec<_> = left_path
                        .waypoints
                        .iter()
                        .map(|tile| mirror_tile(state, *tile))
                        .collect();
                    assert_eq!(mirrored, right_path.waypoints, "{stage}: path {left_id}");
                    assert_eq!(
                        left_path.next, right_path.next,
                        "{stage}: path cursor {left_id}"
                    );
                }
                paths => panic!("{stage}: only one paired unit has a path: {paths:?}"),
            }
        }
        {
            let (left, right) = (0, 1);
            let left = &state.buildings[left];
            let right = &state.buildings[right];
            let (width, height) = left.kind.base_stats().size;
            assert_eq!(left.kind, right.kind, "{stage}: building kind");
            assert_eq!(left.hp, right.hp, "{stage}: building hp");
            assert_eq!(left.queue, right.queue, "{stage}: production queue");
            assert_eq!(
                left.progress, right.progress,
                "{stage}: production progress"
            );
            assert_eq!(
                TilePos::new(
                    state.map.width() - width - left.anchor.x,
                    state.map.height() - height - left.anchor.y,
                ),
                right.anchor,
                "{stage}: building anchor"
            );
        }
    }

    fn pair_new_calibration_units(
        state: &State,
        unit_pairs: &mut Vec<(crate::UnitId, crate::UnitId)>,
    ) {
        let already_paired = |id| {
            unit_pairs
                .iter()
                .any(|&(left, right)| left == id || right == id)
        };
        let unmatched = |player| {
            state
                .units
                .iter()
                .filter(|unit| unit.player == player && !already_paired(unit.id))
                .map(|unit| unit.id)
                .collect::<Vec<_>>()
        };
        let left = unmatched(crate::PlayerId(0));
        let right = unmatched(crate::PlayerId(1));
        assert_eq!(
            left.len(),
            right.len(),
            "a production phase spawned for only one mirrored seat: {left:?} vs {right:?}"
        );
        for pair in left.into_iter().zip(right) {
            unit_pairs.push(pair);
        }
    }

    fn run_calibration_open_tick(
        state: &mut State,
        commands_for_tick: &[PlayerCommand],
        unit_pairs: &mut Vec<(crate::UnitId, crate::UnitId)>,
    ) {
        let tick = state.tick;
        let mut events = Vec::new();
        let mut index = spatial::UnitIndex::new();
        let stage = |phase| format!("tick {tick} {phase}");

        production::capture_recovery_entitlements(state);
        commands::apply(state, commands_for_tick, &mut events);
        assert_calibration_open_symmetry(&stage("commands"), state, unit_pairs);
        production::run(state, &mut events);
        pair_new_calibration_units(state, unit_pairs);
        assert_calibration_open_symmetry(&stage("production"), state, unit_pairs);
        production::decay_abandoned_sites(state);
        let pending = brain::run(state, &mut index, &mut events);
        assert_calibration_open_symmetry(&stage("brains"), state, unit_pairs);
        brain::logistics::resolve(state, pending, &mut events);
        assert_calibration_open_symmetry(&stage("logistics"), state, unit_pairs);
        movement::evict_claimed_ground(state);
        let travel = movement::run(state);
        assert_calibration_open_symmetry(&stage("movement"), state, unit_pairs);
        movement::resolve_collisions(state, &travel, &mut index);
        assert_calibration_open_symmetry(&stage("collisions"), state, unit_pairs);
        detonate_charges(state, &mut events);
        cleanup(state, &mut events);
        if state.tick.is_multiple_of(crate::stats::WRECK_DECAY_TICKS) {
            state.map.decay_wrecks();
        }
        state.refresh_vision();
        victory(state, &mut events);
        assert_calibration_open_symmetry(&stage("cleanup"), state, unit_pairs);
        state.tick += 1;
    }

    #[test]
    fn calibration_open_mirrored_opening_stays_symmetric_through_harvest_cycles() {
        use crate::{Command, PlayerId, UnitId};

        let mut state = calibration_open_cupric()
            .build()
            .expect("the calibration scenario builds");
        let commands = calibration_open_tick_zero_commands();
        let mut unit_pairs = Vec::from(
            [(0, 4), (1, 5), (2, 6), (3, 7)]
                .map(|(left, right)| (crate::UnitId(left), crate::UnitId(right))),
        );

        assert_calibration_open_symmetry("initial", &state, &unit_pairs);
        run_calibration_open_tick(&mut state, &commands, &mut unit_pairs);
        for _ in 1..=600 {
            let commands = if state.tick == 102 {
                vec![
                    PlayerCommand {
                        player: PlayerId(0),
                        command: Command::Harvest {
                            units: vec![UnitId(8)],
                            node: TilePos::new(8, 3),
                            queue: false,
                        },
                    },
                    PlayerCommand {
                        player: PlayerId(1),
                        command: Command::Harvest {
                            units: vec![UnitId(9)],
                            node: TilePos::new(39, 26),
                            queue: false,
                        },
                    },
                ]
            } else {
                Vec::new()
            };
            run_calibration_open_tick(&mut state, &commands, &mut unit_pairs);
        }
    }

    #[test]
    fn mirrored_haulers_replan_together_when_construction_closes_their_routes() {
        use crate::scenario::UnitSpec;
        use crate::state::PathFollow;
        use crate::{BuildingKind, Command, Order, PlayerId, UnitId, UnitKind};
        use chassis::fx::{Fx, Vec2Fx};

        let mut scenario = calibration_open_cupric();
        scenario.units = vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 14,
                y: 7,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 33,
                y: 22,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 8,
                y: 8,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 39,
                y: 21,
            },
        ];
        let mut state = scenario.build().expect("the mirrored scenario builds");
        let left_waypoints = [(13, 7), (12, 7), (11, 7), (10, 7), (9, 7), (8, 6), (7, 6)]
            .map(|(x, y)| TilePos::new(x, y));
        let right_waypoints = left_waypoints.map(|tile| mirror_tile(&state, tile));
        let left_goal = *left_waypoints.last().expect("the route has a goal");
        let right_goal = *right_waypoints.last().expect("the route has a goal");
        for (player, goal) in [(PlayerId(0), left_goal), (PlayerId(1), right_goal)] {
            let foundry = state
                .buildings
                .iter()
                .find(|building| {
                    building.player == player && building.kind == BuildingKind::Foundry
                })
                .expect("each side has a foundry");
            assert!(
                tile_adjacent_to_rect(goal, foundry.anchor, foundry.stats().size),
                "{goal:?} must be a doorstep around {:?}",
                foundry.anchor
            );
        }
        for (id, node, anchor, goal, waypoints) in [
            (
                UnitId(0),
                TilePos::new(12, 9),
                TilePos::new(12, 9),
                left_goal,
                left_waypoints.to_vec(),
            ),
            (
                UnitId(1),
                TilePos::new(35, 20),
                TilePos::new(35, 20),
                right_goal,
                right_waypoints.to_vec(),
            ),
        ] {
            let unit = state.unit_mut(id).expect("the hauler exists");
            unit.carrying = 10;
            unit.order = Order::Harvest {
                node,
                anchor: Some(anchor),
                retiring: false,
            };
            unit.path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
        }

        let report = state.tick(&[
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Build {
                    units: vec![UnitId(2)],
                    kind: BuildingKind::Turret,
                    anchor: TilePos::new(9, 7),
                    queue: false,
                    defer: false,
                },
            },
            PlayerCommand {
                player: PlayerId(1),
                command: Command::Build {
                    units: vec![UnitId(3)],
                    kind: BuildingKind::Turret,
                    anchor: TilePos::new(38, 22),
                    queue: false,
                    defer: false,
                },
            },
        ]);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, crate::Event::CommandRejected { .. })),
            "the mirrored build commands must both land: {:?}",
            report.events
        );

        let left = state.unit(UnitId(0)).expect("the left hauler remains");
        let right = state.unit(UnitId(1)).expect("the right hauler remains");
        assert_eq!(
            Vec2Fx::new(
                Fx::from_num(state.map.width()) - left.pos.x,
                Fx::from_num(state.map.height()) - left.pos.y,
            ),
            right.pos,
            "equivalent route closures must not stagger by global unit id"
        );
        let left_path = left.path.as_ref().expect("the left hauler replans");
        let right_path = right.path.as_ref().expect("the right hauler replans");
        assert!(
            !left_path.waypoints.contains(&TilePos::new(9, 7)),
            "the left route must clear the new footprint: {left_path:?}"
        );
        assert!(
            !right_path.waypoints.contains(&TilePos::new(38, 22)),
            "the right route must clear the new footprint: {right_path:?}"
        );
        assert_eq!(left_path.next, right_path.next);
        assert_eq!(mirror_tile(&state, left_path.goal), right_path.goal);
        assert_eq!(
            left_path
                .waypoints
                .iter()
                .map(|tile| mirror_tile(&state, *tile))
                .collect::<Vec<_>>(),
            right_path.waypoints
        );
    }

    #[test]
    fn centered_mirrored_builders_leave_new_footprints_through_mirrored_doorsteps() {
        use crate::scenario::{BuildingSpec, UnitSpec};
        use crate::{BuildingKind, Command, PlayerId, UnitId, UnitKind};
        use chassis::fx::{Fx, Vec2Fx};

        let left_anchor = TilePos::new(8, 7);
        let right_anchor = TilePos::new(39, 22);
        let mut scenario = calibration_open_cupric();
        scenario.units = vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: left_anchor.x,
                y: left_anchor.y,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: right_anchor.x,
                y: right_anchor.y,
            },
        ];
        scenario.buildings = vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 10,
                y: 12,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Fabricator,
                x: 36,
                y: 16,
            },
        ];
        let mut state = scenario
            .build()
            .expect("the centered mirrored builder scenario builds");

        let report = state.tick(&[
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Build {
                    units: vec![UnitId(0)],
                    kind: BuildingKind::ScuttleCharge,
                    anchor: left_anchor,
                    queue: false,
                    defer: false,
                },
            },
            PlayerCommand {
                player: PlayerId(1),
                command: Command::Build {
                    units: vec![UnitId(1)],
                    kind: BuildingKind::ScuttleCharge,
                    anchor: right_anchor,
                    queue: false,
                    defer: false,
                },
            },
        ]);

        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, crate::Event::CommandRejected { .. })),
            "the mirrored build commands must both land: {:?}",
            report.events
        );
        let left = state.unit(UnitId(0)).expect("the west builder remains");
        let right = state.unit(UnitId(1)).expect("the east builder remains");
        assert_eq!(
            right.pos,
            Vec2Fx::new(
                Fx::from_num(state.map.width()) - left.pos.x,
                Fx::from_num(state.map.height()) - left.pos.y,
            ),
            "builders centered on new sites must take exact half-turn steps"
        );
        let left_path = left.path.as_ref().expect("the west builder routes out");
        let right_path = right.path.as_ref().expect("the east builder routes out");
        assert!(tile_adjacent_to_rect(
            left_path.goal,
            left_anchor,
            BuildingKind::ScuttleCharge.base_stats().size,
        ));
        assert!(tile_adjacent_to_rect(
            right_path.goal,
            right_anchor,
            BuildingKind::ScuttleCharge.base_stats().size,
        ));
        assert_eq!(mirror_tile(&state, left_path.goal), right_path.goal);
        assert_eq!(left_path.next, right_path.next);
        assert_eq!(
            left_path
                .waypoints
                .iter()
                .map(|tile| mirror_tile(&state, *tile))
                .collect::<Vec<_>>(),
            right_path.waypoints
        );
    }

    #[test]
    fn mirrored_six_unit_attack_move_spreads_in_each_armys_local_frame() {
        use crate::scenario::{BuildingSpec, UnitSpec};
        use crate::{BuildingKind, Command, PlayerId, UnitId, UnitKind};

        let mut scenario = calibration_open_cupric();
        scenario.units.extend(
            [
                (0, 9, 9),
                (1, 38, 20),
                (0, 8, 9),
                (1, 39, 20),
                (0, 9, 7),
                (1, 38, 22),
                (0, 7, 9),
                (1, 40, 20),
                (0, 7, 7),
                (1, 40, 22),
            ]
            .map(|(player, x, y)| UnitSpec {
                player,
                kind: UnitKind::Sentinel,
                x,
                y,
            }),
        );
        scenario.buildings.extend([
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Barricade,
                x: 21,
                y: 15,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Barricade,
                x: 26,
                y: 14,
            },
        ]);
        let mut state = scenario.build().expect("the mirrored scenario builds");
        let mut unit_pairs = Vec::from(
            [
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
                (8, 9),
                (10, 11),
                (12, 13),
                (14, 15),
                (16, 17),
            ]
            .map(|(left, right)| (UnitId(left), UnitId(right))),
        );
        let commands = vec![
            PlayerCommand {
                player: PlayerId(0),
                command: Command::AttackMove {
                    units: [3, 8, 10, 12, 14, 16].map(UnitId).into(),
                    goal: TilePos::new(21, 15),
                    queue: false,
                },
            },
            PlayerCommand {
                player: PlayerId(1),
                command: Command::AttackMove {
                    units: [7, 9, 11, 13, 15, 17].map(UnitId).into(),
                    goal: TilePos::new(26, 14),
                    queue: false,
                },
            },
        ];

        assert_calibration_open_symmetry("before group order", &state, &unit_pairs);
        run_calibration_open_tick(&mut state, &commands, &mut unit_pairs);
    }
}
