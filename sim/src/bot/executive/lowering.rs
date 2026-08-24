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
        let mut out = Vec::new();
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
                Intent::Build { kind, anchor } => {
                    if let Some(builder) = self.free_harvester(obs, *anchor, &claimed) {
                        claimed.push(builder);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Build {
                                units: vec![builder],
                                kind: *kind,
                                anchor: *anchor,
                                queue: false,
                                defer: false,
                            },
                        });
                    }
                }
                Intent::FormArmy { staging, size } => {
                    // `size` is a target strength, not an increment: an
                    // army already staging here only drafts the shortfall.
                    let existing = self
                        .armies
                        .iter()
                        .position(|a| a.state == ArmyState::Staging && a.staging == *staging);
                    let want = existing
                        .map(|i| (*size as usize).saturating_sub(self.armies[i].members.len()))
                        .unwrap_or(*size as usize);
                    let draft = self.draft(obs, *staging, want as u32, &claimed);
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
                    if let Some(a) = self.armies.iter_mut().find(|a| a.id == *army)
                        && !a.members.is_empty()
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
                        a.issued = Some((obs.tick, vanguard_centroid(&a.members, obs)));
                        march(me, obs, a, *target, &mut out);
                    }
                }
                Intent::AssignHarvest { unit, node } => {
                    // A unit an earlier intent claimed (a chosen builder,
                    // a scout) or one already held in an army/rear line
                    // must not be re-tasked by a chore.
                    if !claimed.contains(unit) && !self.enlisted().any(|id| id == *unit) {
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
                        && let Some(welder) = self.free_harvester(obs, anchor, &claimed)
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
                    // A boarding rider leaves the world at the sling, and
                    // an army-wide command later this think would replace
                    // its boarding walk. Strike riders from the bodies
                    // and claim them before lowering later intents.
                    for army in &mut self.armies {
                        army.members.retain(|member| !riders.contains(member));
                    }
                    self.armies.retain(|army| !army.members.is_empty());
                    claimed.extend(riders.iter().copied());
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Load {
                            units: riders.clone(),
                            transport: *transport,
                            queue: false,
                        },
                    });
                }
                Intent::Unload { transport, at } => {
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
                        && let Some(stripper) = self.free_harvester(obs, anchor, &claimed)
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
    /// The nearest own harvester to `anchor` that isn't enlisted or
    /// already claimed this think, for construction. Working ones are
    /// fair game (the economy re-hires).
    fn free_harvester(
        &self,
        obs: &Observation,
        anchor: TilePos,
        claimed: &[UnitId],
    ) -> Option<UnitId> {
        let enlisted: Vec<UnitId> = self.enlisted().collect();
        obs.my_units
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
            })
            .map(|u| (u.tile.manhattan(anchor), u.id))
            .min()
            .map(|(_, id)| id)
    }

    /// Drafts up to `size` un-enlisted, unclaimed fighters, nearest to
    /// the staging point first, ties to the lowest id.
    fn draft(
        &self,
        obs: &Observation,
        staging: TilePos,
        size: u32,
        claimed: &[UnitId],
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
            })
            .map(|u| (u.tile.manhattan(staging), u.id))
            .collect();
        candidates.sort_unstable();
        candidates
            .into_iter()
            .take(size as usize)
            .map(|(_, id)| id)
            .collect()
    }
}
