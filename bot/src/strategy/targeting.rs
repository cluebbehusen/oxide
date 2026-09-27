//! Air operation target selection, island admission, and objective
//! tracking.

use super::*;

pub(super) const ISLAND_OPERATION_EARLIEST_TICK: Tick = 3_600;

/// Once a paid operation owns units and factory capital, every difficulty gets
/// the same bounded opportunity to reacquire its objective. Longer tactical
/// memory remains useful when selecting an uncommitted target, but must not
/// make a higher rung hoard committed assets longer after sight is lost.
pub(super) const ACTIVE_OPERATION_TARGET_MEMORY: Tick = 540;

impl ConnectedPlan {
    /// Moves a focus that has left current sight to the best member still in
    /// it. With no member in sight, the previous focus is kept.
    pub(super) fn refocus(&mut self, intel: &StrategicIntelligence) {
        if let Some(best) = best_current_member(&self.commitment, self.focus, intel) {
            self.focus = best.anchor;
        }
    }
}

impl ConnectedCommitment {
    /// Live members whose evidence can size a revision: current sight, or a
    /// remembered contact that still has positive confidence.
    pub(super) fn sized_members<'a>(
        &self,
        intel: &'a StrategicIntelligence,
        now: Tick,
    ) -> Vec<&'a BuildingContact> {
        sized_target_contacts_at_anchors(intel, self.player, &self.anchors, now)
    }
}

impl IslandPlan {
    pub(super) fn new(
        profile: &ResolvedProfile,
        obs: &Observation,
        lanes: ProducerLanes<'_>,
    ) -> Self {
        let airworks = completed(obs, BuildingKind::Airworks);
        let renewable = completed(obs, BuildingKind::Extractor)
            .saturating_add(completed(obs, BuildingKind::Reclaimer));
        let fighters = combat_roster(obs);
        let stance_scale = match profile.stance {
            BotStance::Turtle => 0,
            BotStance::Balanced => 1,
            BotStance::Aggressive => 2,
        };
        let desired_strike_aircraft = 4usize
            .saturating_add(renewable / 2)
            .saturating_add(fighters / 20)
            .saturating_add(usize::from(profile.traits.air >= 60))
            .saturating_add(stance_scale);
        let desired_screen = 2usize
            .saturating_add(renewable / 3)
            .saturating_add(fighters / 40)
            .saturating_add(usize::from(profile.traits.air >= 50))
            .saturating_add(usize::from(profile.traits.guile >= 65));
        let airworks_u64 = u64::try_from(airworks).expect("the map fits in addressable memory");
        let queued_delay = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, building)| building.built && building.kind == BuildingKind::Airworks)
            .map(|(index, building)| {
                let observed_work = obs
                    .my_queues
                    .get(index)
                    .into_iter()
                    .flatten()
                    .map(|kind| u64::from(kind.stats().train_ticks))
                    .sum::<Tick>();
                let same_think_work = lanes
                    .prior_intents
                    .iter()
                    .filter_map(|intent| match intent {
                        Intent::TrainAt {
                            building: producer,
                            kind,
                        } if *producer == building.id => Some(u64::from(kind.stats().train_ticks)),
                        _ => None,
                    })
                    .sum::<Tick>();
                let immediate_delay = observed_work.saturating_add(same_think_work);
                let reserved_delay = lanes
                    .reservations
                    .latest_ready_at(building.id)
                    .map_or(0, |ready_at| ready_at.saturating_sub(obs.tick));
                immediate_delay.max(reserved_delay)
            })
            .max()
            .unwrap_or(0);
        let requested_training = u64::try_from(desired_strike_aircraft)
            .expect("the roster fits in addressable memory")
            .saturating_mul(u64::from(
                Role::Bomber.unit_for(obs.faction).stats().train_ticks,
            ))
            .saturating_add(
                u64::try_from(desired_screen)
                    .expect("the roster fits in addressable memory")
                    .saturating_mul(u64::from(
                        Role::AirGround.unit_for(obs.faction).stats().train_ticks,
                    )),
            )
            .saturating_add(u64::from(
                Role::Scout.unit_for(obs.faction).stats().train_ticks,
            ));
        let assembly_timeout = 900u64
            .saturating_add(queued_delay)
            .saturating_add(requested_training.div_ceil(airworks_u64));
        Self {
            admitted_at: obs.tick,
            desired_strike_aircraft,
            desired_screen,
            screen: Vec::new(),
            assembly_timeout,
            dispatch: AirDispatch::default(),
        }
    }
}

pub(super) fn select_target_candidates(
    intel: &StrategicIntelligence,
    now: Tick,
    tactical_memory: Tick,
) -> Vec<&BuildingContact> {
    let mut candidates: Vec<_> = intel
        .buildings()
        .iter()
        .filter(|b| {
            b.built
                && building_value(b.kind) > 0
                && b.confidence_at(now) > 0
                && (b.evidence == ContactEvidence::Current
                    || b.last_seen
                        .is_some_and(|seen| elapsed(seen, now) <= tactical_memory))
        })
        .collect();
    candidates.sort_unstable_by_key(|b| {
        (
            b.evidence != ContactEvidence::Current,
            Reverse(building_value(b.kind)),
            Reverse(b.confidence_at(now)),
            b.anchor.y,
            b.anchor.x,
            b.player,
            b.kind,
        )
    });
    candidates
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FreshAirTarget<'a> {
    Island(&'a BuildingContact),
    Remembered(&'a BuildingContact),
}

pub(super) fn fresh_air_operation(
    profile: &ResolvedProfile,
    obs: &Observation,
    lanes: ProducerLanes<'_>,
    selected: FreshAirTarget<'_>,
    standby: AirStandby,
) -> ActiveAirOperation {
    let (target, plan) = match selected {
        FreshAirTarget::Island(target) => {
            let island = IslandPlan::new(profile, obs, lanes);
            let plan = if target.evidence == ContactEvidence::Current {
                AirPlan::Island(island)
            } else {
                AirPlan::Reacquire(ReacquirePlan::severed(&island))
            };
            (target, plan)
        }
        FreshAirTarget::Remembered(target) => (target, AirPlan::remembered_connected(obs)),
    };
    let assault_admitted = !matches!(plan, AirPlan::Reacquire(_));
    let op = AirOperation {
        target_player: target.player,
        target_kind: target.kind,
        target: target.anchor,
        target_id: target.id,
        stage: if assault_admitted {
            AirStage::Recon
        } else {
            AirStage::Watching
        },
        started_at: obs.tick,
        phase_started_at: obs.tick,
        scout: standby.scout,
        scout_dispatch: None,
        strike_hold: None,
        artillery_staging: None,
        artillery: if assault_admitted {
            standby.artillery
        } else {
            Vec::new()
        },
        strike_aircraft: if assault_admitted {
            standby.strike_aircraft
        } else {
            Vec::new()
        },
        strike_issued_at: None,
        membership_frozen_at: None,
    };
    ActiveAirOperation { op, plan }
}

pub(super) fn select_fresh_air_target<'a>(
    profile: &ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &Observation,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    lift_support: Option<&LiftSupportRequest>,
    public_map: Option<&PublicMapBriefing>,
) -> Option<FreshAirTarget<'a>> {
    let island_target = if let Some(request) = lift_support {
        exact_wealthy_island_target(profile, obs, home, intel, request, public_map)
    } else {
        select_wealthy_island_target(profile, obs, home, intel, public_map)
    };
    if let Some(target) = island_target {
        return Some(FreshAirTarget::Island(target));
    }
    if lift_support.is_some() {
        return None;
    }
    let candidates = select_target_candidates(intel, obs.tick, tuning.tactical_memory);
    if candidates
        .iter()
        .any(|target| target.evidence == ContactEvidence::Current)
        || combat_roster(obs) < CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER
        || !ready_to_reconnoiter(obs)
    {
        return None;
    }
    candidates.first().copied().map(FreshAirTarget::Remembered)
}

pub(super) fn select_wealthy_island_target<'a>(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    intel: &'a StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> Option<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|target| {
            target.built
                && building_value(target.kind) > 0
                && wealthy_island_target(profile, obs, home, target, public_map)
        })
        .min_by_key(|target| {
            (
                target.evidence != ContactEvidence::Current,
                Reverse(building_value(target.kind)),
                target.anchor.y,
                target.anchor.x,
                target.player,
                target.kind,
            )
        })
}

fn exact_wealthy_island_target<'a>(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    intel: &'a StrategicIntelligence,
    request: &LiftSupportRequest,
    public_map: Option<&PublicMapBriefing>,
) -> Option<&'a BuildingContact> {
    intel.buildings().iter().find(|target| {
        target.player == request.player
            && target.anchor == request.target
            && target.built
            && building_value(target.kind) > 0
            && wealthy_island_target(profile, obs, home, target, public_map)
    })
}

pub(super) fn live_strike_target<'a>(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    let preferred = preferred_anchor(op, plan);
    operation_target_cluster(op, plan, intel)
        .into_iter()
        .filter(|building| building.evidence == ContactEvidence::Current && building.id.is_some())
        .min_by_key(|building| operation_target_key(preferred, building))
}

/// The member an operation stages, scouts, and strikes first: a connected
/// operation's focus, otherwise its objective anchor.
pub(super) fn preferred_anchor(op: &AirOperation, plan: &AirPlan) -> TilePos {
    plan.connected()
        .map_or(op.target, |connected| connected.focus)
}

/// Best currently observed live member, preferring `focus`.
pub(super) fn best_current_member<'a>(
    commitment: &ConnectedCommitment,
    focus: TilePos,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    commitment
        .live_members(intel)
        .into_iter()
        .filter(|building| building.evidence == ContactEvidence::Current && building.id.is_some())
        .min_by_key(|building| operation_target_key(focus, building))
}

fn operation_target_key(
    preferred: TilePos,
    building: &BuildingContact,
) -> (bool, Reverse<u32>, i32, i32, Option<BuildingId>) {
    (
        building.anchor != preferred,
        Reverse(u32::from(building_value(building.kind))),
        building.anchor.y,
        building.anchor.x,
        building.id,
    )
}

/// Connected operations keep the representative identity they were admitted
/// under; only island and remembered objectives follow current sight.
pub(super) fn refresh_target(op: &mut AirOperation, plan: &AirPlan, intel: &StrategicIntelligence) {
    if matches!(plan, AirPlan::Connected(_)) {
        return;
    }
    if let Some(target) = intel.buildings().iter().find(|b| {
        b.player == op.target_player
            && b.anchor == op.target
            && b.evidence == ContactEvidence::Current
    }) {
        op.target_kind = target.kind;
        op.target_id = target.id;
    }
}

pub(super) fn operation_target_cluster<'a>(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Vec<&'a BuildingContact> {
    match plan.connected() {
        Some(connected) => connected.commitment.live_members(intel),
        None => current_target_cluster(intel, op.target_player, op.target),
    }
}

pub(super) fn operation_objective_anchor(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &StrategicIntelligence,
) -> TilePos {
    live_strike_target(op, plan, intel)
        .or_else(|| best_remembered_member(plan, intel))
        .map_or_else(
            || last_strike_anchor(plan).unwrap_or_else(|| preferred_anchor(op, plan)),
            |contact| contact.anchor,
        )
}

/// Best live connected member known only from memory, preferring the focus.
/// Callers use it only when no member is in current sight.
pub(super) fn best_remembered_member<'a>(
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    let connected = plan.connected()?;
    connected
        .commitment
        .live_members(intel)
        .into_iter()
        .filter(|contact| contact.evidence == ContactEvidence::Remembered)
        .min_by_key(|contact| operation_target_key(connected.focus, contact))
}

pub(super) fn last_strike_anchor(plan: &AirPlan) -> Option<TilePos> {
    match plan.strike_dispatch() {
        Some(AirStrikeDispatch::Attack { anchor, .. })
        | Some(AirStrikeDispatch::AttackMove(anchor)) => Some(anchor),
        None => None,
    }
}

pub(super) fn operation_objective_is_stale(
    op: &AirOperation,
    plan: &AirPlan,
    now: Tick,
    intel: &StrategicIntelligence,
) -> bool {
    let Some(connected) = plan.connected() else {
        return intel.buildings().iter().any(|building| {
            building.player == op.target_player
                && building.anchor == op.target
                && building.evidence == ContactEvidence::Remembered
                && building
                    .last_seen
                    .is_none_or(|seen| elapsed(seen, now) > ACTIVE_OPERATION_TARGET_MEMORY)
        });
    };
    let contacts = connected.commitment.live_members(intel);
    !contacts.is_empty()
        && contacts
            .iter()
            .all(|building| building.evidence == ContactEvidence::Remembered)
        && contacts.iter().all(|building| {
            building
                .last_seen
                .is_none_or(|seen| elapsed(seen, now) > ACTIVE_OPERATION_TARGET_MEMORY)
        })
}

/// A connected operation is cleared once current sight has found every
/// admitted member gone, whether or not those anchors remain in sight.
pub(super) fn operation_objective_cleared(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> bool {
    let Some(connected) = plan.connected() else {
        let target_is_current = intel.buildings().iter().any(|building| {
            building.player == op.target_player
                && building.anchor == op.target
                && building.evidence == ContactEvidence::Current
        });
        return target_visible(op, obs) && !target_is_current;
    };
    connected.commitment.live_members(intel).is_empty()
}

/// Live contacts at `anchors` whose evidence can size a package: current sight,
/// or a remembered contact with positive confidence. Anchors selected from a
/// fresh current cluster hold only current contacts.
pub(super) fn sized_target_contacts_at_anchors<'a>(
    intel: &'a StrategicIntelligence,
    player: PlayerId,
    anchors: &[TilePos],
    now: Tick,
) -> Vec<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|contact| {
            contact.player == player
                && anchors.contains(&contact.anchor)
                && (contact.evidence == ContactEvidence::Current || contact.confidence_at(now) > 0)
                && contact.built
                && contact.hp > 0
                && building_value(contact.kind) > 0
        })
        .collect()
}

pub(super) fn target_seen(op: &AirOperation, plan: &AirPlan, obs: &Observation) -> bool {
    obs.enemy_buildings.iter().any(|building| {
        building.seen
            && building.player == op.target_player
            && plan
                .connected()
                .map_or(building.anchor == op.target, |connected| {
                    connected.commitment.contains(building.anchor)
                })
    })
}

pub(super) fn unadmitted_recon(op: &AirOperation) -> bool {
    op.phase() == AirOperationPhase::Recon && !op.assault_admitted()
}

/// The objective's contact while current sight has yet to reacquire it.
pub(super) fn remembered_objective<'a>(
    op: &AirOperation,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    intel.buildings().iter().find(|building| {
        building.player == op.target_player
            && building.anchor == op.target
            && building.evidence == ContactEvidence::Remembered
    })
}

pub(super) fn current_target_contact<'a>(
    op: &AirOperation,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    intel.buildings().iter().find(|building| {
        building.player == op.target_player
            && building.kind == op.target_kind
            && building.anchor == op.target
            && building.evidence == ContactEvidence::Current
    })
}

fn target_visible(op: &AirOperation, obs: &Observation) -> bool {
    let (width, height) = op.target_kind.base_stats().size;
    (0..height).any(|dy| (0..width).any(|dx| obs.visible(op.target.offset(dx, dy))))
}

pub(super) fn wealthy_island_target(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    target: &BuildingContact,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    if completed(obs, BuildingKind::Airworks) == 0
        || completed(obs, BuildingKind::Crucible) == 0
        || !ready_for_airborne_strike(obs)
    {
        return false;
    }
    let stance_delay: Tick = match profile.stance {
        BotStance::Turtle => 500,
        BotStance::Balanced => 250,
        BotStance::Aggressive => 0,
    };
    let personality_delay = u64::from(100u8.saturating_sub(profile.traits.air)) * 8;
    if obs.tick
        < ISLAND_OPERATION_EARLIEST_TICK
            .saturating_add(stance_delay)
            .saturating_add(personality_delay)
    {
        return false;
    }
    let renewable = completed(obs, BuildingKind::Extractor)
        .saturating_add(completed(obs, BuildingKind::Reclaimer));
    let developed_economy = renewable >= 2
        || completed(obs, BuildingKind::Foundry) >= 2
        || obs.scrap
            >= Role::Bomber
                .unit_for(obs.faction)
                .stats()
                .cost
                .saturating_mul(4);
    developed_economy
        && combat_roster(obs) >= 12
        && known_ground_connection(
            obs,
            home,
            target.anchor,
            target.kind.base_stats().size,
            public_map,
        ) == Some(false)
}

fn ready_for_airborne_strike(obs: &Observation) -> bool {
    [
        Role::Scout.unit_for(obs.faction),
        Role::AirGround.unit_for(obs.faction),
        Role::Bomber.unit_for(obs.faction),
    ]
    .into_iter()
    .all(|kind| requirements_met(obs, kind) && has_producer(obs, kind))
}
