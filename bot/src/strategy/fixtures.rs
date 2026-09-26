//! Test-only constructors, accessors, and derivation wrappers for air planning.

use super::*;

pub(crate) struct CommittedClusterFixture {
    pub(crate) faction: oxide_sim::state::Faction,
    pub(crate) primary: (BuildingId, BuildingKind, TilePos),
    pub(crate) members: Vec<TilePos>,
    pub(crate) phase: AirOperationPhase,
    pub(crate) tick: Tick,
    pub(crate) scout: UnitId,
    pub(crate) artillery: Vec<UnitId>,
    pub(crate) strike_aircraft: Vec<UnitId>,
}

pub(crate) struct FreshConnectedProposalFixture {
    pub(crate) objective: BuildingId,
    pub(crate) anchor: TilePos,
    pub(crate) deadline: Tick,
    pub(crate) case: ConnectedOpportunityCase,
    pub(crate) minimum_claims: ConnectedOffenseClaims,
    pub(crate) marginal_additions: Vec<ConnectedOffenseClaims>,
}

impl ConnectedProductionResources {
    pub(super) fn from_observation(
        obs: &Observation,
        target: &BuildingContact,
        unavailable: &[UnitId],
        route: ConnectedRouteContext<'_>,
    ) -> Self {
        Self::from_snapshot_after_current_reserve(
            obs,
            target,
            unavailable,
            route,
            &ResourceSnapshot::from_observation(obs),
            0,
        )
    }
}

impl AirPlan {
    pub(super) fn island(profile: &ResolvedProfile, obs: &Observation) -> Self {
        Self::Island(IslandPlan::new(
            profile,
            obs,
            StrategicProductionContext::empty(),
        ))
    }

    pub(super) fn island_mut(&mut self) -> &mut IslandPlan {
        match self {
            Self::Island(plan) => plan,
            Self::Reacquire(_) | Self::Connected(_) => panic!("expected an island plan"),
        }
    }

    pub(super) fn connected_mut(&mut self) -> &mut ConnectedPlan {
        match self {
            Self::Connected(plan) => plan,
            Self::Reacquire(_) | Self::Island(_) => panic!("expected a connected plan"),
        }
    }

    pub(super) fn package_mut(&mut self) -> Option<&mut ConnectedForcePackage> {
        match self {
            Self::Connected(plan) => Some(&mut plan.package),
            Self::Reacquire(_) | Self::Island(_) => None,
        }
    }
}

impl ConnectedPlan {
    /// A single-target plan against the default test objective.
    pub(super) fn for_test(
        package: ConnectedForcePackage,
        admitted_at: Tick,
        scope: TilePos,
    ) -> Self {
        let mut anchors = package.target_anchors.clone();
        anchors.push(scope);
        anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
        anchors.dedup();
        Self::new(
            ConnectedCommitment {
                player: PlayerId(1),
                primary: BuildingId(80),
                primary_kind: BuildingKind::Crucible,
                scope,
                anchors,
                minimum_capability: package.minimum_capability,
                admitted_at,
                deadline: package.preparation_deadline,
            },
            package,
        )
    }
}

pub(super) fn connected_plan(
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    target: &BuildingContact,
    unavailable: &[UnitId],
    context: ConnectedPlanningContext<'_>,
) -> Result<AirPlan, ConnectedPlanRejection> {
    derive_connected_package(profile, obs, intel, home, target, unavailable, context).map(
        |package| {
            AirPlan::Connected(Box::new(ConnectedPlan::new(
                ConnectedCommitment::admit(target, &package, obs.tick),
                package,
            )))
        },
    )
}

pub(super) fn derive_connected_package(
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    target: &BuildingContact,
    unavailable: &[UnitId],
    context: ConnectedPlanningContext<'_>,
) -> Result<ConnectedForcePackage, ConnectedPlanRejection> {
    derive_connected_package_options(profile, obs, intel, home, target, unavailable, context)
        .map(ConnectedForcePackageOptions::into_largest)
}

impl ConnectedProviderJob {
    pub(crate) fn fixture(
        kind: UnitKind,
        enqueue_not_before: Tick,
        ready_before: Tick,
        eligible_producers: Vec<BuildingId>,
    ) -> Self {
        Self {
            kind,
            enqueue_not_before,
            ready_before,
            eligible_producers,
        }
    }
}

impl ConnectedOffenseClaims {
    pub(crate) fn fixture(
        mut units: Vec<UnitId>,
        provider_jobs: Vec<ConnectedProviderJob>,
    ) -> Self {
        units.sort_unstable();
        units.dedup();
        Self {
            units,
            paid_providers: Vec::new(),
            provider_jobs,
        }
    }
}

impl ConnectedOpportunityCase {
    pub(crate) const fn fixture(
        urgency: ConnectedUrgency,
        confidence: ConnectedConfidence,
        value: ConnectedStrategicValue,
        time_to_impact: ConnectedTimeToImpact,
        safety: ConnectedExecutionSafety,
    ) -> Self {
        Self {
            urgency,
            confidence,
            value,
            time_to_impact,
            safety,
        }
    }
}

impl FreshConnectedProposal {
    pub(crate) fn fixture(fixture: FreshConnectedProposalFixture) -> Self {
        let FreshConnectedProposalFixture {
            objective,
            anchor,
            deadline,
            case,
            minimum_claims,
            marginal_additions,
        } = fixture;
        let derived_at = deadline.saturating_sub(1);
        let package = ConnectedForcePackage {
            derived_at,
            preparation_deadline: deadline,
            target_anchors: vec![anchor],
            recon: Vec::new(),
            suppression: Vec::new(),
            strike: Vec::new(),
            provider_priority: Vec::new(),
            funded_providers: Vec::new(),
            minimum_capability: NormalizedCapability {
                recon: 0,
                suppression: 0,
                strike: 0,
            },
            useful_capability: NormalizedCapability {
                recon: 0,
                suppression: 0,
                strike: 0,
            },
            useful_bombing: 0,
            target_value: 0,
            current_scrap: 0,
            observed_aa_firepower: 0,
            suppressible_aa_firepower: 0,
            forecast_scrap: 0,
            chosen_capability: NormalizedCapability {
                recon: 0,
                suppression: 0,
                strike: 0,
            },
            chosen_bombing: 0,
        };
        let plan = ConnectedPlan::new(
            ConnectedCommitment {
                player: PlayerId(1),
                primary: objective,
                primary_kind: BuildingKind::Crucible,
                scope: anchor,
                anchors: vec![anchor],
                minimum_capability: package.minimum_capability,
                admitted_at: derived_at,
                deadline,
            },
            package,
        );
        let op = AirOperation {
            target_player: PlayerId(1),
            target_kind: BuildingKind::Crucible,
            target: anchor,
            target_id: Some(objective),
            stage: AirStage::Recon,
            started_at: derived_at,
            phase_started_at: derived_at,
            scout: None,
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: Vec::new(),
            strike_aircraft: Vec::new(),
            strike_issued_at: None,
            membership_frozen_at: None,
        };
        let mut cumulative = minimum_claims.clone();
        let mut variants = vec![ConnectedProposalVariant {
            op: op.clone(),
            plan: plan.clone(),
            claims: minimum_claims,
        }];
        let mut marginal = Vec::with_capacity(marginal_additions.len());
        for (offset, additions) in marginal_additions.into_iter().enumerate() {
            cumulative.units.extend(additions.units.iter().copied());
            cumulative.units.sort_unstable();
            cumulative.units.dedup();
            cumulative
                .paid_providers
                .extend(additions.paid_providers.iter().copied());
            cumulative.paid_providers.sort_unstable();
            cumulative
                .provider_jobs
                .extend(additions.provider_jobs.iter().cloned());
            variants.push(ConnectedProposalVariant {
                op: op.clone(),
                plan: plan.clone(),
                claims: cumulative.clone(),
            });
            marginal.push(ConnectedMarginalVariant {
                variant_index: offset + 1,
                additions,
            });
        }
        Self {
            origin: ConnectedProposalOrigin::Idle {
                standby: AirStandby::default(),
            },
            variants,
            marginal,
            selected_variant: 0,
            case,
        }
    }

    pub(crate) fn into_active_revision_fixture(mut self) -> Self {
        self.origin = ConnectedProposalOrigin::Active {
            op: self.variants[0].op.clone(),
            plan: Box::new(self.variants[0].plan.clone()),
        };
        self
    }
}

impl StrategicPlanner {
    /// Scout-only watch over a remembered connected objective.
    pub(crate) fn remembered_watch_fixture(
        target_player: PlayerId,
        target: TilePos,
        tick: Tick,
    ) -> Self {
        let op = AirOperation {
            target_player,
            target_kind: BuildingKind::Foundry,
            target,
            target_id: None,
            stage: AirStage::Watching,
            started_at: tick,
            phase_started_at: tick,
            scout: None,
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: Vec::new(),
            strike_aircraft: Vec::new(),
            strike_issued_at: None,
            membership_frozen_at: None,
        };
        let plan = AirPlan::Reacquire(ReacquirePlan {
            admitted_at: tick,
            assembly_timeout: CONNECTED_PREPARATION_HORIZON,
            terrain: ReacquireTerrain::Connected,
        });
        Self {
            air: Some(ActiveAirOperation { op, plan }),
            ..Self::new()
        }
    }

    /// A connected operation committed to `fixture.members`, staged in a
    /// preparation or strike phase with its exact roster already assigned.
    pub(crate) fn committed_cluster_fixture(fixture: CommittedClusterFixture) -> Self {
        let CommittedClusterFixture {
            faction,
            primary,
            members,
            phase,
            tick,
            scout,
            artillery,
            strike_aircraft,
        } = fixture;
        let admitted_at = tick.saturating_sub(12);
        let scout_kind = Role::Scout.unit_for(faction);
        let bomber = Role::Bomber.unit_for(faction);
        let minimum_capability = NormalizedCapability {
            recon: 1_000,
            suppression: suppression_capability(UnitKind::Bombard, faction),
            strike: strike_capability(bomber, faction),
        };
        let mut anchors = members;
        anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
        anchors.dedup();
        let tranche = |family, kind, count| ProviderDemandTranche {
            priority: force_package::ProviderPriority::Minimum,
            family,
            kind,
            count,
        };
        let package = ConnectedForcePackage {
            derived_at: admitted_at,
            preparation_deadline: admitted_at.saturating_add(CONNECTED_PREPARATION_HORIZON),
            target_anchors: anchors.clone(),
            recon: vec![ProviderDemand {
                kind: scout_kind,
                count: 1,
            }],
            suppression: vec![ProviderDemand {
                kind: UnitKind::Bombard,
                count: artillery.len(),
            }],
            strike: vec![ProviderDemand {
                kind: bomber,
                count: strike_aircraft.len(),
            }],
            provider_priority: vec![
                tranche(ForceFamily::Recon, scout_kind, 1),
                tranche(ForceFamily::Suppression, UnitKind::Bombard, artillery.len()),
                tranche(ForceFamily::Strike, bomber, strike_aircraft.len()),
            ],
            funded_providers: Vec::new(),
            minimum_capability,
            useful_capability: minimum_capability,
            useful_bombing: 0,
            target_value: 1,
            current_scrap: 0,
            observed_aa_firepower: 0,
            suppressible_aa_firepower: 0,
            forecast_scrap: 0,
            chosen_capability: minimum_capability,
            chosen_bombing: 0,
        };
        let (primary_id, primary_kind, primary_anchor) = primary;
        let commitment = ConnectedCommitment {
            player: PlayerId(1),
            primary: primary_id,
            primary_kind,
            scope: primary_anchor,
            anchors,
            minimum_capability,
            admitted_at,
            deadline: package.preparation_deadline,
        };
        let committed = matches!(
            phase,
            AirOperationPhase::SuppressAa | AirOperationPhase::Verify | AirOperationPhase::Strike
        );
        let op = AirOperation {
            target_player: commitment.player,
            target_kind: primary_kind,
            target: primary_anchor,
            target_id: Some(primary_id),
            stage: match phase {
                AirOperationPhase::Recon => AirStage::Recon,
                AirOperationPhase::Assemble => AirStage::Assemble,
                AirOperationPhase::SuppressAa => AirStage::SuppressAa,
                AirOperationPhase::Verify => AirStage::Verify,
                AirOperationPhase::Strike => AirStage::Strike,
                AirOperationPhase::Recover => panic!("stage an active phase"),
            },
            started_at: admitted_at,
            phase_started_at: admitted_at,
            scout: Some(scout),
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery,
            strike_aircraft,
            strike_issued_at: None,
            membership_frozen_at: committed.then_some(admitted_at),
        };
        Self {
            air: Some(ActiveAirOperation {
                op,
                plan: AirPlan::Connected(Box::new(ConnectedPlan::new(commitment, package))),
            }),
            ..Self::new()
        }
    }

    /// Identity an admitted connected operation committed to.
    pub(crate) fn connected_identity(&self) -> Option<ConnectedOffenseIdentity> {
        Some(self.air.as_ref()?.plan.connected()?.commitment.identity())
    }

    pub(super) fn air_plan(&self) -> Option<&AirPlan> {
        self.air.as_ref().map(|active| &active.plan)
    }

    pub(crate) fn air_assembly_timeout(&self) -> Option<Tick> {
        self.air
            .as_ref()
            .map(|active| active.plan.assembly_timeout(active.op.started_at))
    }

    pub(super) fn air_op_mut(&mut self) -> Option<&mut AirOperation> {
        self.air.as_mut().map(|active| &mut active.op)
    }

    pub(super) fn air_plan_mut(&mut self) -> Option<&mut AirPlan> {
        self.air.as_mut().map(|active| &mut active.plan)
    }
}

pub(super) fn select_target(
    intel: &StrategicIntelligence,
    now: Tick,
    tactical_memory: Tick,
) -> Option<&BuildingContact> {
    select_target_candidates(intel, now, tactical_memory)
        .into_iter()
        .next()
}

pub(super) fn connected_artillery_staging_goal(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let routes = route_projection(obs, Domain::Ground, public_map);
    artillery_staging_with_routes(obs, home, target, public_map, &routes)
}
