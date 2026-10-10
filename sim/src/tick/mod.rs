//! The tick pipeline.
//!
//! `State::tick` fixes the phase order, which is part of the simulation
//! contract: reordering phases changes outcomes and state hashes. The
//! phase list lives in `docs/simulation-architecture.md`.
//!
//! After [`GameResult`] is set the world freezes: ticks still count up so
//! timelines stay aligned, but nothing moves and commands are ignored.

pub(crate) use crate::geometry::{
    group_spread_scan_reversed, rect_adjacent_tiles, rect_approach_key_from,
    rect_approach_origin_for_map, spawn_doorstep_key,
};
mod aircraft_crashes;
mod brain;
mod charges;
mod commands;
pub(crate) mod construction;
pub(crate) mod crowding;
mod damage;
pub(crate) mod flight;
mod goals;
pub(crate) mod landing;
mod movement;
mod production;
mod reach;
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
        self.state.accepts_commands(player)
    }

    /// Projected scrap in `player`'s bank.
    pub fn scrap(&self, player: crate::ids::PlayerId) -> Option<u32> {
        self.state.try_player(player).map(|seat| seat.scrap)
    }

    /// Full refunds released by replacing the selected workers' unstarted sites.
    pub fn construction_refund(
        &self,
        player: crate::ids::PlayerId,
        units: &[crate::ids::UnitId],
    ) -> u32 {
        self.state.construction_refund(player, units)
    }

    /// Projected live units, in canonical id order.
    pub fn units(&self) -> &[crate::state::Unit] {
        self.state.units()
    }

    /// Projected buildings and their paid production queues, in canonical id order.
    pub fn buildings(&self) -> &[crate::state::Building] {
        self.state.buildings()
    }

    /// One projected live unit.
    pub fn unit(&self, id: crate::ids::UnitId) -> Option<&crate::state::Unit> {
        self.state.unit(id)
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
/// its owner has no machine that can harvest, whether in the world, aboard a
/// transport, or prepaid in a live production queue.
fn harvester_recovery_needed(state: &State, player: crate::ids::PlayerId) -> bool {
    state.harvester_recovery_needed(player)
}

impl State {
    /// Inspects a private clone after applying only this tick's commands.
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
        let mut motion = Vec::new();
        if self.result.is_none() {
            // One scratch spatial index serves the tick's neighborhood
            // queries. It is rebuilt at each use point and must never be
            // stored on `State` (see `spatial`).
            let mut index = spatial::UnitIndex::new();
            production::capture_recovery_entitlements(self);
            construction::reveal(self, &mut events);
            commands::apply(self, commands, &mut events);
            production::run(self, &mut events);
            charges::cancel_discovered(self, &mut events);
            production::decay_abandoned_sites(self);
            let (boardings, salvaged) = brain::run(self, &mut index, &mut events);
            // Boarding and unloading mutate the unit list, which must hold
            // still under the brains, so they resolve between the last
            // decision and the first movement.
            brain::logistics::resolve(self, boardings, &mut events);
            movement::evict_claimed_ground(self);
            let air_positions = aircraft_crashes::capture_positions(self);
            let (travel, refused) = movement::run(self);
            let driven: Vec<_> = self.units.iter().map(|unit| unit.pos).collect();
            movement::resolve_collisions(self, &travel, &mut index);
            movement::note_stalls(self, &travel, &refused, &driven);
            motion.extend(self.units.iter().zip(&travel).zip(driven).filter_map(
                |((unit, &propulsion), driven)| {
                    let correction = unit.pos - driven;
                    (unit.domain() == crate::stats::Domain::Ground
                        && (propulsion != chassis::fx::Vec2Fx::ZERO
                            || correction != chassis::fx::Vec2Fx::ZERO))
                        .then_some(crate::GroundMotion {
                            unit: unit.id,
                            propulsion,
                            correction,
                        })
                },
            ));
            aircraft_crashes::remember_motion(self, &air_positions);
            aircraft_crashes::land(self, &mut events);
            charges::detonate_under_units(self, &mut events);
            cleanup(self, &salvaged, &mut events);
            construction::cancel_abandoned(self, &mut events);
            if self.tick.is_multiple_of(crate::stats::WRECK_DECAY_TICKS) {
                self.map.decay_wrecks();
            }
            self.refresh_vision();
            construction::reveal(self, &mut events);
            if charges::cancel_discovered(self, &mut events) {
                self.reconcile_attack_knowledge();
            }
            goals::expose(self);
            movement::forget_stalls_without_routes(self);
            victory(self, &mut events);
        }
        self.tick += 1;
        TickReport {
            tick,
            events,
            movement: motion,
        }
    }
}

/// Removes entities that hit 0 hp this tick, reporting each, and leaves a
/// fraction of every destroyed entity's cost on the ground as wreck salvage
/// (buildings split theirs across the footprint). `salvaged` names the
/// buildings salvage, not fire, took apart.
fn cleanup(state: &mut State, salvaged: &[crate::ids::BuildingId], events: &mut Vec<Event>) {
    let dead_charges: Vec<_> = state
        .buildings
        .iter()
        .filter(|b| b.hp == 0 && b.kind.is_stealthy())
        .map(|b| b.id)
        .collect();
    for id in dead_charges {
        crate::vision::forget_observed_building(state, id, false);
    }
    aircraft_crashes::schedule(state);
    let mut deposits: Vec<(TilePos, u32)> = Vec::new();
    for unit in state.units.iter().filter(|u| u.hp == 0) {
        events.push(Event::UnitDied {
            unit: unit.id,
            kind: unit.kind,
            player: unit.player,
            pos: unit.pos,
            grounded: unit.domain() == crate::stats::Domain::Ground,
        });
        let value =
            unit.kind.stats().cost * crate::stats::WRECK_VALUE_NUM / crate::stats::WRECK_VALUE_DEN;
        deposits.push((unit.tile(), value));
        // Cargo dies with the airframe and deposits at its tile.
        for rider in &unit.cargo {
            events.push(Event::UnitDied {
                unit: rider.id,
                kind: rider.kind,
                player: rider.player,
                pos: unit.pos,
                grounded: unit.domain() == crate::stats::Domain::Ground,
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
        if salvaged.contains(&building.id) {
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
            tier: building.tier,
            pos: building.center(),
        });
        let stats = building.stats();
        let price = stats
            .construction
            .map_or(crate::stats::FOUNDRY_WRECK_VALUE, |c| c.cost);
        let value = price * crate::stats::WRECK_VALUE_NUM / crate::stats::WRECK_VALUE_DEN;
        let (width, height) = building.kind.size();
        let tiles = u32::try_from(width * height).expect("building footprints have positive area");
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
        // Wreck must never coexist with a standing footprint: harvesters
        // cannot reach it, and the building's own eventual wreck would
        // double-stack. Buildings that died this tick are already removed,
        // so their footprints take deposits normally.
        if state
            .buildings
            .iter()
            .any(|b| !b.provisional() && b.contains(tile))
        {
            continue;
        }
        // Salvage on rock, peaks, or pits could never be harvested, so it is
        // dropped. Scrap node tiles keep their deposits: they become standable
        // when the node exhausts.
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
/// Elimination is Foundry-based: a team lives while any of its seats holds a
/// Foundry; other standing buildings do not keep it in the game. A resigned
/// seat's Foundries stop counting the tick it concedes, so a fully resigned
/// team is eliminated immediately. The command gate in `commands::apply` stays
/// player-scoped: a Foundry-less or resigned seat on a living team spectates
/// while its team plays on.
fn victory(state: &mut State, events: &mut Vec<Event>) {
    if state.mode == crate::scenario::ScenarioMode::Sandbox || state.result.is_some() {
        return;
    }
    // Stamp each seat's first tick out of the match: resigned, or holding no
    // Foundry (sites count). Recorded once and never cleared; it is the FFA
    // scoreboard's placement key.
    for index in 0..state.players.len() {
        if state.players[index].eliminated_at.is_some() {
            continue;
        }
        let seat = crate::ids::PlayerId::from_index(index);
        let out = state.players[index].resigned
            || !state.buildings.iter().any(|b| {
                b.player == seat
                    && !b.provisional()
                    && b.kind == crate::stats::BuildingKind::Foundry
            });
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
            !b.provisional()
                && b.kind == crate::stats::BuildingKind::Foundry
                && owner.team == team
                && !owner.resigned
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
    state.aircraft_crashes.clear();
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
            // Every air route passes here, so peak goals snap here.
            // `line_blocked` ignores endpoints, so an unsnapped peak goal
            // would route the flyer onto the peak.
            let to = if state.passable_for(crate::stats::Domain::Air, to) {
                to
            } else {
                // The flyer's own approach sets the scan frame, so mirrored
                // flights snap to mirrored sky. A flyer already over the goal
                // tile falls back to the map-center frame.
                let reverse = group_spread_scan_reversed(
                    to,
                    [from_tile],
                    None,
                    (state.map.width(), state.map.height()),
                    crate::ids::PlayerId(0),
                );
                goals::group_domain_goal(state, to, crate::stats::Domain::Air, reverse)?
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
    let first_foundry = state
        .buildings
        .iter()
        .filter(|building| {
            building.player == player
                && !building.provisional()
                && building.kind == crate::stats::BuildingKind::Foundry
        })
        .min_by_key(|building| building.id)
        .map(|foundry| (foundry.anchor, foundry.kind.size()));
    rect_approach_origin_for_map(
        (state.map.width(), state.map.height()),
        player,
        from,
        anchor,
        size,
        first_foundry,
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

#[cfg(test)]
mod tests;
