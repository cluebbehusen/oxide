//! Intent lowering and emergency economy recovery.

use super::armies::{is_artillery, march, vanguard_centroid};
use super::*;

impl Executive {
    /// Reserves a stranded economy's bank and queues its replacement
    /// Harvester as soon as the reserve is whole.
    ///
    /// `None` means ordinary play. `Some([])` means the seat is still
    /// saving (or its Foundry queue is temporarily full); callers must
    /// skip policy spending for this think. A non-empty vector is the
    /// one emergency Train command. Recovery is an executive safety
    /// rule, independent of the policy's ordinary spending channels.
    pub(crate) fn harvester_recovery(
        &self,
        me: PlayerId,
        obs: &Observation,
    ) -> Option<Vec<PlayerCommand>> {
        let has_harvester = obs
            .my_units
            .iter()
            .any(|u| u.kind.stats().harvest.is_some());
        let queued_harvester = obs
            .my_queues
            .iter()
            .flatten()
            .any(|kind| *kind == UnitKind::Harvester);
        let has_foundry = obs
            .my_buildings
            .iter()
            .any(|b| b.kind == BuildingKind::Foundry && b.built);
        if has_harvester || queued_harvester || !has_foundry {
            return None;
        }

        let mut commands = Vec::new();
        if obs.scrap >= UnitKind::Harvester.stats().cost
            && let Some((_, foundry)) = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(qi, b)| {
                    b.kind == BuildingKind::Foundry
                        && b.built
                        && obs.my_queues[*qi].len() < crate::stats::QUEUE_CAP
                })
                .min_by_key(|(_, b)| b.id)
        {
            commands.push(PlayerCommand {
                player: me,
                command: Command::Train {
                    building: foundry.id,
                    kind: UnitKind::Harvester,
                },
            });
        }
        Some(commands)
    }

    /// Applies a think's intents in order and returns the commands they
    /// lower to. Deterministic given `(self, obs, intents)`.
    pub fn apply(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        intents: &[Intent],
    ) -> Vec<PlayerCommand> {
        self.apply_inner(me, obs, intents, &[], false)
    }

    /// Applies intents while keeping an exact operation's members out of
    /// opportunistic drafts and worker selection. Reserved ids remain eligible
    /// for an exact-group intent; they are unavailable only to intents that
    /// choose units implicitly.
    pub fn apply_with_reservations(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        intents: &[Intent],
        reservations: &[UnitId],
    ) -> Vec<PlayerCommand> {
        self.apply_inner(me, obs, intents, reservations, true)
    }

    fn apply_inner(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        intents: &[Intent],
        reservations: &[UnitId],
        defer_unseen_builds: bool,
    ) -> Vec<PlayerCommand> {
        let mut out = Vec::new();
        let centroid_frame = defer_unseen_builds
            .then(|| self.player_frame.as_ref().map(|tactics| tactics.frame))
            .flatten();
        let reserved = canonical_owned_units(me, obs, reservations, &[]);
        // Units spoken for by an earlier intent in this same think — keeps
        // a Scout's unit from being drafted by a FormArmy a line later.
        let mut claimed: Vec<UnitId> = Vec::new();
        for intent in intents {
            match intent {
                Intent::TrainAt { building, kind } => out.push(PlayerCommand {
                    player: me,
                    command: Command::Train {
                        building: *building,
                        kind: *kind,
                    },
                }),
                Intent::StopUnits { units } => {
                    let units = canonical_owned_units(me, obs, units, &[]);
                    if !units.is_empty() {
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Stop { units },
                        });
                    }
                }
                Intent::Build { kind, anchor } => {
                    if let Some(builder) = self.free_harvester(
                        obs,
                        *anchor,
                        Some(kind.base_stats().size),
                        &claimed,
                        &reserved,
                        defer_unseen_builds,
                    ) {
                        claimed.push(builder);
                        let (width, height) = kind.base_stats().size;
                        let defer = defer_unseen_builds
                            && (0..height)
                                .any(|dy| (0..width).any(|dx| !obs.visible(anchor.offset(dx, dy))));
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Build {
                                units: vec![builder],
                                kind: *kind,
                                anchor: *anchor,
                                queue: false,
                                defer,
                            },
                        });
                    }
                }
                Intent::BuildWith {
                    builder,
                    kind,
                    anchor,
                } => {
                    let valid = obs.my_units.iter().any(|unit| {
                        unit.id == *builder
                            && unit.player == me
                            && unit.kind.stats().harvest.is_some()
                            && unit.site.is_none()
                            && unit.founding.is_none()
                    });
                    if !valid || claimed.contains(builder) || reserved.contains(builder) {
                        continue;
                    }
                    claimed.push(*builder);
                    let (width, height) = kind.base_stats().size;
                    let defer = defer_unseen_builds
                        && (0..height)
                            .any(|dy| (0..width).any(|dx| !obs.visible(anchor.offset(dx, dy))));
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Build {
                            units: vec![*builder],
                            kind: *kind,
                            anchor: *anchor,
                            queue: false,
                            defer,
                        },
                    });
                }
                Intent::FormArmy { staging, size } => {
                    // `size` is a target strength, not an increment: an
                    // army already staging here only drafts the shortfall.
                    // Player-facing bodies whose rallies converge within the
                    // executive's arrival radius are one muster, not separate
                    // armies. Keeping them split lets the policy inspect only
                    // one under-strength fragment forever even when their
                    // combined force is ready. The Overseer retains exact-rally
                    // matching as its frozen QA behavior.
                    let existing = if defer_unseen_builds {
                        self.consolidate_staging_armies(obs, *staging)
                    } else {
                        self.armies
                            .iter()
                            .position(|a| a.state == ArmyState::Staging && a.staging == *staging)
                    };
                    let want = existing
                        .map(|i| (*size as usize).saturating_sub(self.armies[i].members.len()))
                        .unwrap_or(*size as usize);
                    let draft = self.draft(
                        obs,
                        *staging,
                        want as u32,
                        &claimed,
                        &reserved,
                        defer_unseen_builds,
                    );
                    if !draft.is_empty() {
                        claimed.extend(draft.iter().copied());
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: draft.clone(),
                                goal: *staging,
                                queue: false,
                            },
                        });
                        if let Some(i) = existing {
                            self.armies[i].members.extend(draft);
                            self.armies[i].members.sort_unstable();
                        } else {
                            let id = ArmyId(self.next_army);
                            self.next_army += 1;
                            self.armies.push(Army {
                                id,
                                members: draft,
                                state: ArmyState::Staging,
                                staging: *staging,
                                target: None,
                                focus: None,
                                progress: None,
                                issued: None,
                                bounces: 0,
                            });
                        }
                    }
                }
                Intent::PushArmy { army, target } => {
                    if let Some(a) = self.armies.iter_mut().find(|a| a.id == *army) {
                        if defer_unseen_builds && a.members.iter().any(|id| reserved.contains(id)) {
                            let available: Vec<_> = a
                                .members
                                .iter()
                                .copied()
                                .filter(|id| !reserved.contains(id))
                                .collect();
                            if !available.is_empty() {
                                a.members = available;
                                a.focus = None;
                                a.progress = None;
                                a.issued = None;
                                a.bounces = 0;
                            }
                        }
                        if !a.members.is_empty()
                            && a.members.iter().all(|id| {
                                !reserved.contains(id) && !claimed.contains(id)
                            })
                            // An artillery-only body is parked at staging by
                            // march() rather than sent forward, so entering
                            // Pushing would start a wedge clock on a march
                            // that never happened and fabricate a seal.
                            && obs
                                .my_units
                                .iter()
                                .any(|u| a.members.contains(&u.id) && !is_artillery(u))
                        {
                            // A re-push at the same target is the same march:
                            // the wedge clock and bounce count carry over, or a
                            // policy re-issuing Push every think could never
                            // accumulate either and a refused march would stall
                            // forever without testifying.
                            // The distance clock is per target; the bounce
                            // count is not. Two refusals in a row are evidence
                            // wherever the second was aimed — an alternating
                            // pair of doorsteps reset the count every think
                            // and a 20-unit army was refused 160 times without
                            // ever testifying.
                            if a.target != Some(*target) {
                                a.progress = None;
                            }
                            a.state = ArmyState::Pushing;
                            a.target = Some(*target);
                            a.issued = Some((
                                obs.tick,
                                vanguard_centroid(&a.members, obs, centroid_frame),
                            ));
                            march(me, obs, a, *target, &mut out);
                        }
                    }
                }
                Intent::MoveUnits { units, goal } => {
                    let units = self.claim_exact_units(me, obs, units, &mut claimed);
                    if !units.is_empty() {
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Move {
                                units,
                                goal: *goal,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::AttackMoveUnits { units, goal } => {
                    let units = self.claim_exact_units(me, obs, units, &mut claimed);
                    if !units.is_empty() {
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units,
                                goal: *goal,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::AttackUnits { units, target } => {
                    let units = self.claim_exact_units(me, obs, units, &mut claimed);
                    if !units.is_empty() {
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Attack {
                                units,
                                target: *target,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::RepairUnits { welders, target } => {
                    let target_is_own_ground = obs.my_units.iter().any(|unit| {
                        unit.id == *target
                            && unit.player == me
                            && unit.hp > 0
                            && unit.kind.stats().domain == crate::stats::Domain::Ground
                    });
                    if !target_is_own_ground {
                        continue;
                    }
                    let eligible: Vec<_> = welders
                        .iter()
                        .copied()
                        .filter(|id| {
                            *id != *target
                                && obs
                                    .my_units
                                    .iter()
                                    .any(|unit| unit.id == *id && unit.kind.stats().welder)
                        })
                        .collect();
                    let welders = self.claim_exact_units(me, obs, &eligible, &mut claimed);
                    if !welders.is_empty() {
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::RepairUnit {
                                units: welders,
                                target: *target,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::AssignHarvest { unit, node } => {
                    // A unit an earlier intent claimed (a chosen builder,
                    // a scout) or one already held in an army/rear line
                    // must not be re-tasked by a chore.
                    if !claimed.contains(unit)
                        && !reserved.contains(unit)
                        && !self.enlisted().any(|id| id == *unit)
                    {
                        claimed.push(*unit);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Harvest {
                                units: vec![*unit],
                                node: *node,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::Scout { unit, to } => {
                    if claimed.contains(unit) || reserved.contains(unit) {
                        continue;
                    }
                    // A dispatched scout leaves its army, mirroring
                    // Load's rider strike: an army-wide command later
                    // this think would otherwise replace the scout walk.
                    for army in &mut self.armies {
                        army.members.retain(|member| member != unit);
                    }
                    self.armies.retain(|army| !army.members.is_empty());
                    claimed.push(*unit);
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Move {
                            units: vec![*unit],
                            goal: *to,
                            queue: false,
                        },
                    });
                }
                Intent::Repair { building } => {
                    let anchor = obs
                        .my_buildings
                        .iter()
                        .find(|b| b.id == *building)
                        .map(|b| b.anchor);
                    if let Some(anchor) = anchor
                        && let Some(welder) = self.free_harvester(
                            obs,
                            anchor,
                            None,
                            &claimed,
                            &reserved,
                            defer_unseen_builds,
                        )
                    {
                        claimed.push(welder);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Repair {
                                units: vec![welder],
                                building: *building,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::Load { transport, riders } => {
                    let riders = if defer_unseen_builds {
                        let transport_is_valid = obs.my_units.iter().any(|unit| {
                            unit.id == *transport
                                && unit.player == me
                                && unit.hp > 0
                                && unit.kind.stats().transport_capacity > 0
                        });
                        if !transport_is_valid || claimed.contains(transport) {
                            continue;
                        }
                        let requested: Vec<_> = riders
                            .iter()
                            .copied()
                            .filter(|rider| {
                                rider != transport
                                    && obs.my_units.iter().any(|unit| {
                                        unit.id == *rider && unit.kind.stats().transport_size > 0
                                    })
                            })
                            .collect();
                        let riders = canonical_owned_units(me, obs, &requested, &claimed);
                        if riders.is_empty() {
                            continue;
                        }
                        let mut members = riders.clone();
                        members.push(*transport);
                        self.claim_exact_units(me, obs, &members, &mut claimed);
                        riders
                    } else {
                        // The frozen Overseer already supplies its exact
                        // distance-ranked rider order. Preserve those command
                        // bytes here; the simulation applies set semantics at
                        // dispatch.
                        riders.clone()
                    };
                    // A boarding rider leaves the world at the sling, and
                    // an army-wide command later this think would replace
                    // its boarding walk. Strike riders from the bodies
                    // and claim them before lowering later intents.
                    if !defer_unseen_builds {
                        for army in &mut self.armies {
                            army.members.retain(|member| !riders.contains(member));
                        }
                        self.armies.retain(|army| !army.members.is_empty());
                        claimed.extend(riders.iter().copied());
                        claimed.push(*transport);
                    }
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Load {
                            units: riders,
                            transport: *transport,
                            queue: false,
                        },
                    });
                }
                Intent::Unload { transport, at } => {
                    if defer_unseen_builds {
                        let transport_is_valid = obs.my_units.iter().any(|unit| {
                            unit.id == *transport
                                && unit.player == me
                                && unit.hp > 0
                                && unit.kind.stats().transport_capacity > 0
                        });
                        if !transport_is_valid
                            || self
                                .claim_exact_units(me, obs, &[*transport], &mut claimed)
                                .is_empty()
                        {
                            continue;
                        }
                    }
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Unload {
                            transport: *transport,
                            at: *at,
                            queue: false,
                        },
                    });
                }
                Intent::Upgrade { building } => {
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::UpgradeBuilding {
                            building: *building,
                        },
                    });
                }
                Intent::Salvage { building } => {
                    let anchor = obs
                        .my_buildings
                        .iter()
                        .find(|b| b.id == *building)
                        .map(|b| b.anchor);
                    if let Some(anchor) = anchor
                        && let Some(stripper) =
                            self.free_harvester(obs, anchor, None, &claimed, &reserved, false)
                    {
                        claimed.push(stripper);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Salvage {
                                units: vec![stripper],
                                building: *building,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::RaidAir { target } => {
                    let enlisted: Vec<UnitId> = self.enlisted().collect();
                    let wings: Vec<UnitId> = obs
                        .my_units
                        .iter()
                        .filter(|u| {
                            let stats = u.kind.stats();
                            stats.domain == crate::stats::Domain::Air
                                && stats.can_target(crate::stats::Domain::Ground)
                                && u.idle
                                && !enlisted.contains(&u.id)
                                && !claimed.contains(&u.id)
                                && !reserved.contains(&u.id)
                        })
                        .map(|u| u.id)
                        .collect();
                    if !wings.is_empty() {
                        claimed.extend(wings.iter().copied());
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: wings,
                                goal: *target,
                                queue: false,
                            },
                        });
                    }
                }
            }
        }
        out
    }
    /// The nearest own harvester to `anchor` that isn't enlisted or already
    /// claimed this think. Player-facing construction also refuses workers
    /// whose fog-honest known component cannot reach the command's snapped
    /// goal; otherwise one trapped worker can make every legal site look
    /// unusable. Working harvesters remain fair game because the economy
    /// re-hires.
    fn free_harvester(
        &self,
        obs: &Observation,
        anchor: TilePos,
        proposed_footprint: Option<(i32, i32)>,
        claimed: &[UnitId],
        reserved: &[UnitId],
        require_route: bool,
    ) -> Option<UnitId> {
        let enlisted: Vec<UnitId> = self.enlisted().collect();
        let candidates: Vec<_> = obs
            .my_units
            .iter()
            .filter(|u| {
                u.kind.stats().harvest.is_some()
                    && u.site.is_none()
                    // A walking founder is as spoken for as a builder
                    // on site: re-tasking it silently drops the
                    // promised claim.
                    && u.founding.is_none()
                    && !enlisted.contains(&u.id)
                    && !claimed.contains(&u.id)
                    && !reserved.contains(&u.id)
            })
            .collect();
        let mut routes =
            crate::bot::routing::RouteProjection::new(obs, crate::stats::Domain::Ground);
        candidates
            .into_iter()
            .filter(|unit| {
                !require_route
                    || proposed_footprint.map_or_else(
                        || routes.group_reaches_command_goal(&[unit.id], anchor),
                        |size| {
                            crate::bot::routing::unit_reaches_build_site(obs, unit, anchor, size)
                        },
                    )
            })
            .map(|u| (u.tile.manhattan(anchor), u.id))
            .min()
            .map(|(_, id)| id)
    }

    /// Drafts up to `size` un-enlisted, unclaimed fighters, nearest to
    /// the staging point first, ties to the lowest id. Player-facing drafts
    /// include only members whose explored ground component can accept the
    /// resulting muster command; the frozen Overseer remains optimistic.
    fn draft(
        &self,
        obs: &Observation,
        staging: TilePos,
        size: u32,
        claimed: &[UnitId],
        reserved: &[UnitId],
        require_known_route: bool,
    ) -> Vec<UnitId> {
        let enlisted: Vec<UnitId> = self.enlisted().collect();
        let mut candidates: Vec<(i32, UnitId)> = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                // Armies are ground bodies: the lifecycle's centroids,
                // standoffs, and focus picks are all ground-shaped, and
                // enlisting wings here would starve the raid channel of
                // the very units it was bought for.
                stats.can_fight()
                    && stats.domain == crate::stats::Domain::Ground
                    && u.idle
                    && !enlisted.contains(&u.id)
                    && !claimed.contains(&u.id)
                    && !reserved.contains(&u.id)
                    && (!require_known_route || self.exhausted_rear.binary_search(&u.id).is_err())
            })
            .map(|u| (u.tile.manhattan(staging), u.id))
            .collect();
        candidates.sort_unstable();
        if !require_known_route {
            return candidates
                .into_iter()
                .take(size as usize)
                .map(|(_, id)| id)
                .collect();
        }

        let mut routes = crate::bot::routing::RouteProjection::known_ground(obs);
        let mut draft = Vec::with_capacity((size as usize).min(candidates.len()));
        for (_, id) in candidates {
            if draft.len() == size as usize {
                break;
            }
            let mut proposed = draft.clone();
            proposed.push(id);
            if routes.group_reaches_command_goal(&proposed, staging) {
                draft.push(id);
            }
        }
        draft.sort_unstable();
        draft
    }

    fn consolidate_staging_armies(&mut self, obs: &Observation, staging: TilePos) -> Option<usize> {
        let mut routes = crate::bot::routing::RouteProjection::known_ground(obs);
        let candidates: Vec<ArmyId> = self
            .armies
            .iter()
            .filter(|army| {
                army.state == ArmyState::Staging
                    // A target-holding body is still prosecuting its previous
                    // attack. It is not the rally that receives the next
                    // generation of fighters.
                    && army.target.is_none()
                    && army.staging.chebyshev(staging) <= 2
                    && routes.group_reaches_command_goal(&army.members, staging)
            })
            .map(|army| army.id)
            .collect();
        let primary = candidates.iter().copied().min()?;
        if candidates.len() > 1 {
            let mut members: Vec<UnitId> = self
                .armies
                .iter()
                .filter(|army| candidates.contains(&army.id))
                .flat_map(|army| army.members.iter().copied())
                .collect();
            members.sort_unstable();
            members.dedup();
            self.armies
                .retain(|army| army.id == primary || !candidates.contains(&army.id));
            let army = self
                .armies
                .iter_mut()
                .find(|army| army.id == primary)
                .expect("the primary staged army survives consolidation");
            army.members = members;
            army.staging = staging;
        }
        self.armies.iter().position(|army| army.id == primary)
    }

    fn claim_exact_units(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        requested: &[UnitId],
        claimed: &mut Vec<UnitId>,
    ) -> Vec<UnitId> {
        let units = canonical_owned_units(me, obs, requested, claimed);
        if units.is_empty() {
            return units;
        }

        for army in &mut self.armies {
            army.members.retain(|member| !units.contains(member));
        }
        self.armies.retain(|army| !army.members.is_empty());
        claimed.extend(units.iter().copied());
        units
    }
}

fn canonical_owned_units(
    me: PlayerId,
    obs: &Observation,
    requested: &[UnitId],
    unavailable: &[UnitId],
) -> Vec<UnitId> {
    let mut units = requested.to_vec();
    units.sort_unstable();
    units.dedup();
    units.retain(|id| {
        !unavailable.contains(id)
            && obs
                .my_units
                .iter()
                .any(|unit| unit.id == *id && unit.player == me && unit.hp > 0)
    });
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::OBSERVATION_VERSION;
    use crate::state::Faction;

    fn fighter(id: u32, tile: TilePos, idle: bool) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Lancer,
            tile,
            hp: UnitKind::Lancer.stats().max_hp,
            idle,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        }
    }

    fn unit(id: u32, player: u8, kind: UnitKind, hp: u32) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(player),
            kind,
            tile: TilePos::new(4 + id as i32, 4),
            hp,
            idle: true,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        }
    }

    fn target_holding_position() -> (Observation, Executive) {
        let target = TilePos::new(19, 9);
        let mut units: Vec<_> = (1..=6).map(|id| fighter(id, target, false)).collect();
        units.extend((100..=104).map(|id| fighter(id, TilePos::new(4, 4), true)));
        let observation = Observation {
            version: OBSERVATION_VERSION,
            tick: 200_000,
            me: PlayerId(0),
            scrap: 0,
            map_width: 40,
            map_height: 24,
            my_units: units,
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![true; 40 * 24],
            explored: vec![true; 40 * 24],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        };
        let executive = Executive {
            armies: vec![Army {
                id: ArmyId(7),
                members: (1..=6).map(UnitId).collect(),
                state: ArmyState::Staging,
                staging: target,
                target: Some(target),
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            }],
            next_army: 8,
            ..Executive::default()
        };
        (observation, executive)
    }

    #[test]
    fn player_facing_muster_does_not_absorb_a_target_holding_staged_army() {
        let (obs, mut executive) = target_holding_position();
        let target = TilePos::new(19, 9);

        let commands = executive.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy {
                staging: target,
                size: 5,
            }],
            &[],
        );

        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0].command,
            Command::AttackMove { units, goal, queue }
                if units == &(100..=104).map(UnitId).collect::<Vec<_>>()
                    && *goal == target
                    && !queue
        ));
        assert_eq!(executive.armies().len(), 2);
        let old = executive
            .armies()
            .iter()
            .find(|army| army.id == ArmyId(7))
            .expect("the target-holding army remains intact");
        assert_eq!(old.members, (1..=6).map(UnitId).collect::<Vec<_>>());
        assert_eq!(old.target, Some(target));
        let muster = executive
            .armies()
            .iter()
            .find(|army| army.id == ArmyId(8))
            .expect("fresh fighters form a distinct muster");
        assert_eq!(muster.members, (100..=104).map(UnitId).collect::<Vec<_>>());
        assert_eq!(muster.target, None);
    }

    #[test]
    fn forward_defense_muster_does_not_recall_a_fighter_into_the_home_army() {
        let home = TilePos::new(5, 5);
        let expansion = TilePos::new(31, 17);
        let mut units: Vec<_> = (1..=5).map(|id| fighter(id, home, false)).collect();
        units.push(fighter(100, expansion, true));
        let obs = Observation {
            version: OBSERVATION_VERSION,
            tick: 20_000,
            me: PlayerId(0),
            scrap: 0,
            map_width: 40,
            map_height: 24,
            my_units: units,
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![true; 40 * 24],
            explored: vec![true; 40 * 24],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        };
        let mut executive = Executive {
            armies: vec![Army {
                id: ArmyId(7),
                members: (1..=5).map(UnitId).collect(),
                state: ArmyState::Staging,
                staging: home,
                target: None,
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            }],
            next_army: 8,
            ..Executive::default()
        };

        let commands = executive.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy {
                staging: expansion,
                size: 6,
            }],
            &[],
        );

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::AttackMove { units, goal, queue: false },
                ..
            }] if units == &[UnitId(100)] && *goal == expansion
        ));
        assert_eq!(executive.armies().len(), 2);
        assert_eq!(
            executive.armies()[0].members,
            (1..=5).map(UnitId).collect::<Vec<_>>()
        );
        assert_eq!(executive.armies()[0].staging, home);
        assert_eq!(executive.armies()[1].members, vec![UnitId(100)]);
        assert_eq!(executive.armies()[1].staging, expansion);
    }

    #[test]
    fn player_facing_muster_leaves_exhausted_rear_units_home_until_repaired() {
        let (mut obs, mut executive) = target_holding_position();
        let staging = TilePos::new(12, 8);
        obs.my_units = vec![
            fighter(1, TilePos::new(4, 4), true),
            fighter(2, TilePos::new(5, 4), true),
        ];
        obs.my_units[0].hp = UnitKind::Sentinel.stats().max_hp / 4;
        executive.armies.clear();
        executive.exhausted_rear = vec![UnitId(1)];

        let commands = executive.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy { staging, size: 2 }],
            &[],
        );

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::AttackMove { units, goal, queue: false },
                ..
            }] if units == &[UnitId(2)] && *goal == staging
        ));
        assert_eq!(executive.armies()[0].members, vec![UnitId(2)]);

        obs.my_units[0].hp = UnitKind::Sentinel.stats().max_hp;
        executive.maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2));
        assert!(executive.exhausted_rear.is_empty());

        let commands = executive.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy { staging, size: 2 }],
            &[],
        );
        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::AttackMove { units, goal, queue: false },
                ..
            }] if units == &[UnitId(1)] && *goal == staging
        ));
        assert_eq!(executive.armies()[0].members, vec![UnitId(1), UnitId(2)]);
    }

    #[test]
    fn overseer_muster_preserves_legacy_access_to_exhausted_rear_units() {
        let (mut obs, mut executive) = target_holding_position();
        let staging = TilePos::new(12, 8);
        obs.my_units = vec![fighter(1, TilePos::new(4, 4), true)];
        obs.my_units[0].hp = UnitKind::Sentinel.stats().max_hp / 4;
        executive.armies.clear();
        executive.exhausted_rear = vec![UnitId(1)];

        let commands = executive.apply(PlayerId(0), &obs, &[Intent::FormArmy { staging, size: 1 }]);

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::AttackMove { units, goal, queue: false },
                ..
            }] if units == &[UnitId(1)] && *goal == staging
        ));
    }

    #[test]
    fn a_partially_reserved_army_defends_without_retasking_operation_members() {
        let (obs, mut executive) = target_holding_position();
        let target = TilePos::new(12, 8);

        let commands = executive.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::PushArmy {
                army: ArmyId(7),
                target,
            }],
            &[UnitId(3)],
        );

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::AttackMove { units, goal, queue: false },
                ..
            }] if units == &[UnitId(1), UnitId(2), UnitId(4), UnitId(5), UnitId(6)]
                && *goal == target
        ));
        let army = &executive.armies()[0];
        assert_eq!(
            army.members,
            [UnitId(1), UnitId(2), UnitId(4), UnitId(5), UnitId(6)]
        );
        assert_eq!(army.state, ArmyState::Pushing);
        assert_eq!(army.target, Some(target));
        assert!(executive.enlisted().all(|unit| unit != UnitId(3)));
    }

    #[test]
    fn a_fully_reserved_army_remains_untouched_until_its_operation_dispatches() {
        let (obs, mut executive) = target_holding_position();
        let original = executive.armies()[0].clone();
        let reserved = (1..=6).map(UnitId).collect::<Vec<_>>();

        let commands = executive.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::PushArmy {
                army: ArmyId(7),
                target: TilePos::new(12, 8),
            }],
            &reserved,
        );

        assert!(commands.is_empty());
        assert_eq!(executive.armies(), &[original]);
    }

    #[test]
    fn overseer_keeps_exact_rally_reinforcement_behavior() {
        let (obs, mut executive) = target_holding_position();
        let target = TilePos::new(19, 9);

        let commands = executive.apply(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy {
                staging: target,
                size: 7,
            }],
        );

        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0].command,
            Command::AttackMove { units, goal, queue }
                if units == &[UnitId(100)] && *goal == target && !queue
        ));
        assert_eq!(executive.armies().len(), 1);
        let army = &executive.armies()[0];
        assert_eq!(army.id, ArmyId(7));
        assert_eq!(
            army.members,
            (1..=6).map(UnitId).chain([UnitId(100)]).collect::<Vec<_>>()
        );
        assert_eq!(army.target, Some(target));
    }

    #[test]
    fn exact_builds_fail_closed_for_stale_claimed_and_reserved_workers() {
        let (mut obs, _) = target_holding_position();
        let build_anchor = TilePos::new(18, 6);
        let mut stale_founder = unit(
            1,
            0,
            UnitKind::Harvester,
            UnitKind::Harvester.stats().max_hp,
        );
        stale_founder.founding = Some((BuildingKind::Turret, build_anchor));
        obs.my_units = vec![
            stale_founder,
            unit(
                2,
                0,
                UnitKind::Harvester,
                UnitKind::Harvester.stats().max_hp,
            ),
            unit(
                3,
                0,
                UnitKind::Harvester,
                UnitKind::Harvester.stats().max_hp,
            ),
        ];
        let stale_goal = TilePos::new(7, 7);
        let claimed_goal = TilePos::new(8, 7);
        let reserved_goal = TilePos::new(9, 7);

        let commands = Executive::new().apply_with_reservations(
            PlayerId(0),
            &obs,
            &[
                Intent::BuildWith {
                    builder: UnitId(1),
                    kind: BuildingKind::Turret,
                    anchor: build_anchor,
                },
                Intent::MoveUnits {
                    units: vec![UnitId(1)],
                    goal: stale_goal,
                },
                Intent::MoveUnits {
                    units: vec![UnitId(2)],
                    goal: claimed_goal,
                },
                Intent::BuildWith {
                    builder: UnitId(2),
                    kind: BuildingKind::Turret,
                    anchor: build_anchor,
                },
                Intent::BuildWith {
                    builder: UnitId(3),
                    kind: BuildingKind::Turret,
                    anchor: build_anchor,
                },
                Intent::MoveUnits {
                    units: vec![UnitId(3)],
                    goal: reserved_goal,
                },
            ],
            &[UnitId(3)],
        );

        assert_eq!(
            commands,
            [
                PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Move {
                        units: vec![UnitId(1)],
                        goal: stale_goal,
                        queue: false,
                    },
                },
                PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Move {
                        units: vec![UnitId(2)],
                        goal: claimed_goal,
                        queue: false,
                    },
                },
                PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Move {
                        units: vec![UnitId(3)],
                        goal: reserved_goal,
                        queue: false,
                    },
                },
            ]
        );
    }

    #[test]
    fn repair_units_rejects_invalid_targets_without_claiming_the_welder() {
        let (mut obs, _) = target_holding_position();
        obs.my_units = vec![
            unit(10, 0, UnitKind::Tender, UnitKind::Tender.stats().max_hp),
            unit(1, 0, UnitKind::Lancer, 0),
            unit(2, 1, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(3, 0, UnitKind::Buzzard, UnitKind::Buzzard.stats().max_hp),
        ];
        let goal = TilePos::new(14, 8);

        let commands = Executive::new().apply_with_reservations(
            PlayerId(0),
            &obs,
            &[
                Intent::RepairUnits {
                    welders: vec![UnitId(10)],
                    target: UnitId(99),
                },
                Intent::RepairUnits {
                    welders: vec![UnitId(10)],
                    target: UnitId(1),
                },
                Intent::RepairUnits {
                    welders: vec![UnitId(10)],
                    target: UnitId(2),
                },
                Intent::RepairUnits {
                    welders: vec![UnitId(10)],
                    target: UnitId(3),
                },
                Intent::MoveUnits {
                    units: vec![UnitId(10)],
                    goal,
                },
            ],
            &[],
        );

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::Move {
                    units,
                    goal: command_goal,
                    queue: false,
                },
                ..
            }] if units == &[UnitId(10)] && *command_goal == goal
        ));
    }

    #[test]
    fn refused_exact_load_does_not_claim_its_transport() {
        let (mut obs, _) = target_holding_position();
        obs.my_units = vec![
            unit(1, 0, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(10, 0, UnitKind::Skyhook, UnitKind::Skyhook.stats().max_hp),
        ];
        let rider_goal = TilePos::new(8, 7);
        let transport_goal = TilePos::new(18, 7);

        let commands = Executive::new().apply_with_reservations(
            PlayerId(0),
            &obs,
            &[
                Intent::MoveUnits {
                    units: vec![UnitId(1)],
                    goal: rider_goal,
                },
                Intent::Load {
                    transport: UnitId(10),
                    riders: vec![UnitId(1)],
                },
                Intent::MoveUnits {
                    units: vec![UnitId(10)],
                    goal: transport_goal,
                },
            ],
            &[UnitId(1), UnitId(10)],
        );

        assert_eq!(commands.len(), 2);
        assert!(matches!(
            &commands[0].command,
            Command::Move { units, goal, queue: false }
                if units == &[UnitId(1)] && *goal == rider_goal
        ));
        assert!(matches!(
            &commands[1].command,
            Command::Move { units, goal, queue: false }
                if units == &[UnitId(10)] && *goal == transport_goal
        ));
    }

    #[test]
    fn exact_load_rejects_invalid_transports_without_claiming_riders() {
        let (mut obs, _) = target_holding_position();
        obs.my_units = vec![
            unit(1, 0, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(2, 0, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(11, 0, UnitKind::Skyhook, 0),
            unit(12, 1, UnitKind::Skyhook, UnitKind::Skyhook.stats().max_hp),
        ];
        let goal = TilePos::new(16, 8);

        let commands = Executive::new().apply_with_reservations(
            PlayerId(0),
            &obs,
            &[
                Intent::Load {
                    transport: UnitId(99),
                    riders: vec![UnitId(1)],
                },
                Intent::Load {
                    transport: UnitId(11),
                    riders: vec![UnitId(1)],
                },
                Intent::Load {
                    transport: UnitId(12),
                    riders: vec![UnitId(1)],
                },
                Intent::Load {
                    transport: UnitId(2),
                    riders: vec![UnitId(1)],
                },
                Intent::MoveUnits {
                    units: vec![UnitId(1), UnitId(2)],
                    goal,
                },
            ],
            &[],
        );

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::Move { units, goal: command_goal, queue: false },
                ..
            }] if units == &[UnitId(1), UnitId(2)] && *command_goal == goal
        ));
    }

    #[test]
    fn exact_load_filters_invalid_riders_before_committing_the_group() {
        let (mut obs, _) = target_holding_position();
        obs.my_units = vec![
            unit(1, 0, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(2, 0, UnitKind::Lancer, 0),
            unit(3, 1, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(4, 0, UnitKind::Buzzard, UnitKind::Buzzard.stats().max_hp),
            unit(10, 0, UnitKind::Skyhook, UnitKind::Skyhook.stats().max_hp),
        ];
        let goal = TilePos::new(17, 8);

        let commands = Executive::new().apply_with_reservations(
            PlayerId(0),
            &obs,
            &[
                Intent::Load {
                    transport: UnitId(10),
                    riders: vec![
                        UnitId(10),
                        UnitId(4),
                        UnitId(3),
                        UnitId(2),
                        UnitId(1),
                        UnitId(99),
                        UnitId(1),
                    ],
                },
                Intent::MoveUnits {
                    units: vec![UnitId(1), UnitId(4), UnitId(10)],
                    goal,
                },
            ],
            &[UnitId(1), UnitId(10)],
        );

        assert_eq!(commands.len(), 2);
        assert!(matches!(
            &commands[0].command,
            Command::Load { units, transport: UnitId(10), queue: false }
                if units == &[UnitId(1)]
        ));
        assert!(matches!(
            &commands[1].command,
            Command::Move { units, goal: command_goal, queue: false }
                if units == &[UnitId(4)] && *command_goal == goal
        ));
    }

    #[test]
    fn exact_unload_rejects_invalid_transports_without_claiming_own_units() {
        let (mut obs, _) = target_holding_position();
        obs.my_units = vec![
            unit(1, 0, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(2, 0, UnitKind::Lancer, UnitKind::Lancer.stats().max_hp),
            unit(11, 0, UnitKind::Skyhook, 0),
            unit(12, 1, UnitKind::Skyhook, UnitKind::Skyhook.stats().max_hp),
        ];
        let at = TilePos::new(15, 8);

        let commands = Executive::new().apply_with_reservations(
            PlayerId(0),
            &obs,
            &[
                Intent::Unload {
                    transport: UnitId(99),
                    at,
                },
                Intent::Unload {
                    transport: UnitId(11),
                    at,
                },
                Intent::Unload {
                    transport: UnitId(12),
                    at,
                },
                Intent::Unload {
                    transport: UnitId(2),
                    at,
                },
                Intent::MoveUnits {
                    units: vec![UnitId(1), UnitId(2)],
                    goal: at,
                },
            ],
            &[],
        );

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::Move { units, goal, queue: false },
                ..
            }] if units == &[UnitId(1), UnitId(2)] && *goal == at
        ));
    }

    #[test]
    fn exact_unload_accepts_a_reserved_transport_and_claims_it_once() {
        let (mut obs, _) = target_holding_position();
        obs.my_units = vec![unit(
            10,
            0,
            UnitKind::Skyhook,
            UnitKind::Skyhook.stats().max_hp,
        )];
        let at = TilePos::new(15, 8);

        let commands = Executive::new().apply_with_reservations(
            PlayerId(0),
            &obs,
            &[
                Intent::Unload {
                    transport: UnitId(10),
                    at,
                },
                Intent::MoveUnits {
                    units: vec![UnitId(10)],
                    goal: TilePos::new(4, 4),
                },
            ],
            &[UnitId(10)],
        );

        assert!(matches!(
            commands.as_slice(),
            [PlayerCommand {
                command: Command::Unload { transport: UnitId(10), at: command_at, queue: false },
                ..
            }] if *command_at == at
        ));
    }
}
