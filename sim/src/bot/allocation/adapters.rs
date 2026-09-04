//! Exact translations between domain-owned proposals and shared allocation.

use super::{
    AllocationConflict, AllocationPersonality, AllocationResult, ClaimBundle, ClaimBundleError,
    Confidence, ConnectedOffenseKey, DeferrableCapitalClaim, ExecutionSafety, ForecastClaim,
    FoundryExpansionKey, ImportedObligation, InvestmentProposal, ObligationClass, ObligationKey,
    ProducerJobClaim, ProposalCase, ProposalKey, ScheduledProducerJob, StrategicValue,
    TimeToImpact, Urgency,
};
use crate::bot::profile::ResolvedProfile;
use crate::bot::standing_force::StandingForceProposal;
use crate::bot::strategy::{
    ConnectedConfidence, ConnectedExecutionSafety, ConnectedMarginalVariant,
    ConnectedOffenseClaims, ConnectedOpportunityCase, ConnectedStrategicValue,
    ConnectedTimeToImpact, ConnectedUrgency, FreshConnectedProposal,
};
use crate::bot::utility::{
    FoundryConfidence, FoundryExecutionSafety, FoundryOpportunityCase, FoundryStrategicValue,
    FoundryTimeToImpact, FoundryUrgency, FreshFoundryProposal,
};
use crate::stats::BuildingKind;

/// Exact domain token carried opaquely through shared allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DomainPayload {
    /// Frozen safe Foundry expansion.
    Foundry(FreshFoundryProposal),
    /// Frozen connected-operation package.
    Connected(Box<FreshConnectedProposal>),
    /// One independently useful standing-force purchase.
    StandingForce(StandingForceProposal),
}

/// Exact domain payloads compared during cross-domain allocation.
pub(crate) type DomainInvestmentProposal = InvestmentProposal<DomainPayload>;

/// Allocation output retaining the exact selected domain plans.
pub(crate) type DomainAllocationResult = AllocationResult<DomainPayload>;

impl AllocationPersonality {
    /// Resolves positive cross-domain emphasis without granting or removing work.
    pub(crate) const fn from_profile(profile: &ResolvedProfile) -> Self {
        Self {
            economy: profile.traits.greed as u16,
            offense: (profile.traits.air as u16 + profile.traits.siege as u16) / 2,
            standing_force: (profile.traits.support as u16
                + profile.traits.fortification as u16
                + profile.traits.guile as u16)
                / 3,
        }
    }
}

/// Retains one expansion domain's frozen payload and exact shared claims.
pub(crate) fn foundry_investment_proposal(
    proposal: FreshFoundryProposal,
) -> Result<DomainInvestmentProposal, ClaimBundleError> {
    let claims = ClaimBundle::new(
        0,
        Vec::new(),
        vec![proposal.builder()],
        Vec::new(),
        vec![proposal_site(&proposal)],
        Vec::new(),
    )?
    .with_deferrable_capital(DeferrableCapitalClaim {
        through: proposal.forecast_deadline(),
        amount: proposal.construction_capital(),
    })?;
    Ok(InvestmentProposal::fresh(
        ProposalKey::FoundryExpansion(FoundryExpansionKey {
            anchor: proposal.anchor(),
        }),
        proposal.case().into(),
        claims,
        DomainPayload::Foundry(proposal),
    ))
}

/// Retains one connected domain's exact minimum and ordered producer choices.
pub(crate) fn connected_investment_proposal(
    proposal: FreshConnectedProposal,
) -> Result<DomainInvestmentProposal, ClaimBundleError> {
    let claims = connected_claim_bundle(proposal.minimum_claims())?;
    Ok(InvestmentProposal::retained(
        ProposalKey::ConnectedOffenseMinimum(ConnectedOffenseKey {
            objective: proposal.objective(),
            anchor: proposal.anchor(),
        }),
        proposal.case().into(),
        proposal.accepted_at(),
        claims,
        DomainPayload::Connected(Box::new(proposal)),
    ))
}

/// Retains one independently useful standing-force purchase or bounded wait.
pub(crate) fn standing_force_investment_proposal(
    proposal: StandingForceProposal,
) -> Result<DomainInvestmentProposal, ClaimBundleError> {
    let personality_preference = u16::from(proposal.personality_emphasis());
    let claims = if let Some((through, current_scrap, forecast_scrap)) = proposal.accumulation() {
        ClaimBundle::new(
            current_scrap,
            vec![ForecastClaim {
                through,
                amount: forecast_scrap,
            }],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )?
    } else {
        let job = ProducerJobClaim::immediate(
            proposal.key_kind(),
            proposal.observed_at(),
            proposal.ready_before(),
            proposal.eligible_producers().to_vec(),
        );
        ClaimBundle::new(0, Vec::new(), Vec::new(), Vec::new(), Vec::new(), vec![job])?
    }
    .with_minimum_residual_scrap(proposal.minimum_residual_scrap());
    Ok(InvestmentProposal::fresh(
        ProposalKey::StandingForce(proposal.key()),
        proposal.case(),
        claims,
        DomainPayload::StandingForce(proposal),
    )
    .with_personality_preference(personality_preference))
}

/// Retains the standing-force domain's deterministic best-first alternatives.
pub(crate) fn standing_force_investment_proposals(
    proposals: Vec<StandingForceProposal>,
) -> Result<Vec<DomainInvestmentProposal>, ClaimBundleError> {
    proposals
        .into_iter()
        .enumerate()
        .map(|(preference, proposal)| {
            standing_force_investment_proposal(proposal)
                .map(|proposal| proposal.with_domain_preference(preference))
        })
        .collect()
}

/// Retains a re-derived active minimum as mandatory work at its original
/// acceptance priority.
pub(crate) fn active_connected_revision_obligation(
    proposal: &FreshConnectedProposal,
) -> Result<ImportedObligation, ClaimBundleError> {
    debug_assert!(proposal.revises_active_operation());
    let identity = proposal.identity();
    let delta = proposal
        .active_revision_provider_delta()
        .expect("an active revision retains its bound producer schedule");
    let minimum = proposal.minimum_claims();
    let provider_jobs = delta
        .allocation_order(minimum.provider_jobs().len())
        .into_iter()
        .map(|proposal_ordinal| {
            let job = &delta.jobs()[proposal_ordinal];
            if let Some(retained) = job.retained() {
                let timing = retained.timing();
                ProducerJobClaim::fixed(
                    retained.producer(),
                    retained.kind(),
                    timing.enqueued_at(),
                    timing.starts_at(),
                    timing.ready_at(),
                    timing.ready_before(),
                )
            } else {
                let request = job.request();
                ProducerJobClaim::flexible(
                    request.kind(),
                    request.enqueue_not_before(),
                    request.ready_before(),
                    request.eligible_producers().to_vec(),
                )
            }
        })
        .collect();
    Ok(super::imported_obligation(
        ObligationClass::PersistentPlan,
        proposal.accepted_at(),
        ObligationKey::ConnectedOffense {
            objective: identity.objective(),
            anchor: identity.anchor(),
        },
        ClaimBundle::new(
            0,
            Vec::new(),
            Vec::new(),
            minimum.units().to_vec(),
            Vec::new(),
            provider_jobs,
        )?,
    ))
}

/// Carries an active revision's opaque payload through portfolio selection.
/// Its minimum is already mandatory; only marginal additions consume proposal
/// capacity.
pub(crate) fn active_connected_revision_investment_proposal(
    proposal: FreshConnectedProposal,
) -> DomainInvestmentProposal {
    debug_assert!(proposal.revises_active_operation());
    InvestmentProposal::retained(
        ProposalKey::ConnectedOffenseMinimum(ConnectedOffenseKey {
            objective: proposal.objective(),
            anchor: proposal.anchor(),
        }),
        proposal.case().into(),
        proposal.accepted_at(),
        ClaimBundle::default(),
        DomainPayload::Connected(Box::new(proposal)),
    )
}

/// Converts one cumulative additions-only scale step without charging jobs twice.
pub(crate) fn connected_marginal_claims(
    marginal: &ConnectedMarginalVariant,
) -> Result<ClaimBundle, ClaimBundleError> {
    connected_claim_bundle(marginal.additions())
}

fn connected_claim_bundle(
    claims: &ConnectedOffenseClaims,
) -> Result<ClaimBundle, ClaimBundleError> {
    ClaimBundle::new(
        0,
        Vec::new(),
        Vec::new(),
        claims.units().to_vec(),
        Vec::new(),
        claims
            .provider_jobs()
            .iter()
            .map(|job| {
                ProducerJobClaim::flexible(
                    job.kind(),
                    job.enqueue_not_before(),
                    job.ready_before(),
                    job.eligible_producers().to_vec(),
                )
            })
            .collect(),
    )
}

fn proposal_site(proposal: &FreshFoundryProposal) -> crate::bot::resources::SiteFootprint {
    crate::bot::resources::SiteFootprint::new(
        proposal.anchor(),
        BuildingKind::Foundry.base_stats().size,
    )
    .expect("Foundries have a positive footprint")
}

impl From<FoundryOpportunityCase> for ProposalCase {
    fn from(case: FoundryOpportunityCase) -> Self {
        Self {
            urgency: match case.urgency() {
                FoundryUrgency::Developmental => Urgency::Developmental,
                FoundryUrgency::Timely => Urgency::Timely,
                FoundryUrgency::Pressing => Urgency::Pressing,
            },
            confidence: match case.confidence() {
                FoundryConfidence::Supported => Confidence::Supported,
                FoundryConfidence::Corroborated => Confidence::Current,
            },
            value: match case.value() {
                FoundryStrategicValue::Incremental => StrategicValue::Incremental,
                FoundryStrategicValue::Material => StrategicValue::Material,
                FoundryStrategicValue::Decisive => StrategicValue::Decisive,
            },
            time_to_impact: match case.time_to_impact() {
                FoundryTimeToImpact::Patient => TimeToImpact::Patient,
                FoundryTimeToImpact::Near => TimeToImpact::Near,
            },
            safety: match case.safety() {
                FoundryExecutionSafety::Managed => ExecutionSafety::Managed,
                FoundryExecutionSafety::Secure => ExecutionSafety::Secure,
            },
        }
    }
}

impl From<ConnectedOpportunityCase> for ProposalCase {
    fn from(case: ConnectedOpportunityCase) -> Self {
        Self {
            urgency: match case.urgency() {
                ConnectedUrgency::Developmental => Urgency::Developmental,
                ConnectedUrgency::Timely => Urgency::Timely,
                ConnectedUrgency::Pressing => Urgency::Pressing,
            },
            confidence: match case.confidence() {
                ConnectedConfidence::Prior => Confidence::Prior,
                ConnectedConfidence::Supported => Confidence::Supported,
                ConnectedConfidence::Current => Confidence::Current,
            },
            value: match case.value() {
                ConnectedStrategicValue::Incremental => StrategicValue::Incremental,
                ConnectedStrategicValue::Material => StrategicValue::Material,
                ConnectedStrategicValue::Decisive => StrategicValue::Decisive,
            },
            time_to_impact: match case.time_to_impact() {
                ConnectedTimeToImpact::Patient => TimeToImpact::Patient,
                ConnectedTimeToImpact::Near => TimeToImpact::Near,
                ConnectedTimeToImpact::Immediate => TimeToImpact::Immediate,
            },
            safety: match case.safety() {
                ConnectedExecutionSafety::Speculative => ExecutionSafety::Speculative,
                ConnectedExecutionSafety::Managed => ExecutionSafety::Managed,
                ConnectedExecutionSafety::Secure => ExecutionSafety::Secure,
            },
        }
    }
}

/// Exact selected domain payloads after allocation and optional scale extension.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AcceptedDomainPayloads {
    foundry: Option<FreshFoundryProposal>,
    connected: Option<FreshConnectedProposal>,
    standing_force: Option<StandingForceProposal>,
}

impl AcceptedDomainPayloads {
    /// Selected expansion payload, if the economic proposal won.
    pub(crate) fn take_foundry(&mut self) -> Option<FreshFoundryProposal> {
        self.foundry.take()
    }

    /// Selected connected-operation payload, if the offensive proposal won.
    pub(crate) fn take_connected(&mut self) -> Option<FreshConnectedProposal> {
        self.connected.take()
    }

    /// Selected standing-force purchase, if it won allocation.
    pub(crate) fn take_standing_force(&mut self) -> Option<StandingForceProposal> {
        self.standing_force.take()
    }
}

/// Failure to atomically retain one domain-provided marginal scale step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectedMarginalError {
    /// The connected minimum did not win this allocation pass.
    NoAcceptedConnectedProposal,
    /// The scale token does not belong to the retained connected proposal.
    StaleVariant,
    /// The domain supplied internally inconsistent exact claims.
    MalformedClaims(ClaimBundleError),
    /// Shared resources cannot fit the complete additions-only step.
    Conflict(AllocationConflict),
}

impl DomainAllocationResult {
    /// Exact connected opportunity key retained by selection, if any.
    pub(crate) fn accepted_connected_key(&self) -> Option<ConnectedOffenseKey> {
        self.accepted.iter().find_map(|proposal| {
            validate_payload_key(proposal);
            match proposal.key() {
                ProposalKey::ConnectedOffenseMinimum(key) => Some(key),
                ProposalKey::FoundryExpansion(_) | ProposalKey::StandingForce(_) => None,
            }
        })
    }

    /// Retained cumulative scale choices, still owned by the domain payload.
    pub(crate) fn connected_marginal_variants(&self) -> Option<&[ConnectedMarginalVariant]> {
        self.accepted.iter().find_map(|proposal| {
            validate_payload_key(proposal);
            match proposal.payload() {
                DomainPayload::Connected(payload) => Some(payload.marginal_variants()),
                DomainPayload::Foundry(_) | DomainPayload::StandingForce(_) => None,
            }
        })
    }

    /// Applies one retained additions-only scale step and updates its payload together.
    pub(crate) fn try_accept_connected_marginal(
        &mut self,
        capacity: &super::AllocationCapacity,
        marginal: &ConnectedMarginalVariant,
    ) -> Result<ClaimBundle, ConnectedMarginalError> {
        let (key, belongs) = self
            .accepted
            .iter()
            .find_map(|proposal| {
                validate_payload_key(proposal);
                match (proposal.key(), proposal.payload()) {
                    (
                        ProposalKey::ConnectedOffenseMinimum(key),
                        DomainPayload::Connected(payload),
                    ) => Some((key, payload.marginal_variants().contains(marginal))),
                    (ProposalKey::FoundryExpansion(_), DomainPayload::Foundry(_))
                    | (ProposalKey::StandingForce(_), DomainPayload::StandingForce(_)) => None,
                    _ => unreachable!("payload validation is exhaustive above"),
                }
            })
            .ok_or(ConnectedMarginalError::NoAcceptedConnectedProposal)?;
        if !belongs {
            return Err(ConnectedMarginalError::StaleVariant);
        }
        let claims =
            connected_marginal_claims(marginal).map_err(ConnectedMarginalError::MalformedClaims)?;
        self.try_extend_connected_offense(capacity, key, &claims)
            .map_err(ConnectedMarginalError::Conflict)?;
        let selected = self
            .accepted
            .iter_mut()
            .find_map(|proposal| match proposal.payload_mut() {
                DomainPayload::Connected(payload) => Some(payload),
                DomainPayload::Foundry(_) | DomainPayload::StandingForce(_) => None,
            });
        assert!(
            selected.is_some_and(|payload| payload.select_marginal(marginal)),
            "a validated retained marginal token remains selectable"
        );
        Ok(claims)
    }

    /// Consumes allocation only after diagnostics and scale selection are complete.
    pub(crate) fn into_domain_payloads(self) -> AcceptedDomainPayloads {
        let mut payloads = AcceptedDomainPayloads::default();
        for proposal in self.accepted {
            let (key, claims, payload) = proposal.into_parts();
            match (key, payload) {
                (ProposalKey::FoundryExpansion(_), DomainPayload::Foundry(mut payload)) => {
                    debug_assert!(payloads.foundry.is_none());
                    assert!(
                        payload.rebind_funding(
                            claims.current_scrap(),
                            claims
                                .forecast_scrap()
                                .iter()
                                .map(|claim| claim.amount)
                                .fold(0, u32::saturating_add),
                        ),
                        "allocation preserves a Foundry proposal's exact total capital"
                    );
                    payloads.foundry = Some(payload);
                }
                (ProposalKey::ConnectedOffenseMinimum(_), DomainPayload::Connected(payload)) => {
                    debug_assert!(payloads.connected.is_none());
                    payloads.connected = Some(*payload);
                }
                (ProposalKey::StandingForce(_), DomainPayload::StandingForce(payload)) => {
                    debug_assert!(payloads.standing_force.is_none());
                    payloads.standing_force = Some(payload);
                }
                _ => panic!("an accepted allocation payload must match its proposal domain"),
            }
        }
        payloads
    }

    /// Final deterministic schedule after every accepted marginal extension.
    pub(crate) fn final_producer_schedule(&self) -> &[ScheduledProducerJob] {
        &self.producer_schedule
    }

    /// Whether exact lane binding funded the proposal that discharges the
    /// shared current-only remainder.
    pub(crate) const fn voluntary_scrap_guard_satisfied(&self) -> bool {
        self.voluntary_scrap_guard_satisfied
    }
}

fn validate_payload_key(proposal: &DomainInvestmentProposal) {
    assert!(
        matches!(
            (proposal.key(), proposal.payload()),
            (ProposalKey::FoundryExpansion(_), DomainPayload::Foundry(_))
                | (
                    ProposalKey::ConnectedOffenseMinimum(_),
                    DomainPayload::Connected(_)
                )
                | (
                    ProposalKey::StandingForce(_),
                    DomainPayload::StandingForce(_)
                )
        ),
        "an allocation payload must match its proposal domain"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::allocation::{
        AllocationCapacity, ImportedObligation, ObligationClass, ObligationKey, ProposalKey,
        StandingForceKey, allocate,
    };
    use crate::bot::profile::{PersonalityTraits, Specialty};
    use crate::bot::resources::{
        BuilderResource, ProducerPlanningProjection, ResourcePlanningFixture,
        ResourcePlanningProjection,
    };
    use crate::bot::standing_force::{
        StandingForceFixture, StandingForceProposal, StandingForceReason,
    };
    use crate::bot::strategy::FreshConnectedProposalFixture;
    use crate::ids::{BuildingId, UnitId};
    use crate::scenario::{BotDifficulty, BotStance};
    use crate::stats::UnitKind;
    use chassis::Tick;
    use chassis::grid::TilePos;

    const NOW: Tick = 120;
    const DEADLINE: Tick = 1_200;

    fn foundry_case() -> FoundryOpportunityCase {
        FoundryOpportunityCase::fixture(
            FoundryUrgency::Timely,
            FoundryConfidence::Supported,
            FoundryStrategicValue::Material,
            FoundryTimeToImpact::Near,
            FoundryExecutionSafety::Secure,
        )
    }

    fn connected_case() -> ConnectedOpportunityCase {
        ConnectedOpportunityCase::fixture(
            ConnectedUrgency::Timely,
            ConnectedConfidence::Current,
            ConnectedStrategicValue::Material,
            ConnectedTimeToImpact::Near,
            ConnectedExecutionSafety::Secure,
        )
    }

    fn capacity(
        current_scrap: u32,
        units: Vec<UnitId>,
        builders: Vec<BuilderResource>,
        producers: Vec<ProducerPlanningProjection>,
    ) -> AllocationCapacity {
        AllocationCapacity::fixture(
            ResourcePlanningProjection::fixture(ResourcePlanningFixture {
                current_scrap,
                observed_at: NOW,
                horizon: DEADLINE,
                cadence: 12,
                forecast_income: Vec::new(),
                units,
                builders,
                producers,
            })
            .expect("the adapter fixture uses a valid planning horizon"),
        )
    }

    fn foundry(builder: UnitId, current: u32, forecast: u32) -> FreshFoundryProposal {
        FreshFoundryProposal::fixture(
            TilePos::new(17, 9),
            builder,
            current,
            forecast,
            23,
            DEADLINE,
            foundry_case(),
        )
    }

    fn connected(
        claims: ConnectedOffenseClaims,
        marginals: Vec<ConnectedOffenseClaims>,
    ) -> FreshConnectedProposal {
        FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(90),
            anchor: TilePos::new(31, 7),
            deadline: DEADLINE,
            case: connected_case(),
            minimum_claims: claims,
            marginal_additions: marginals,
            protected_current_scrap: 777,
            protected_forecast_scrap: 555,
        })
    }

    #[test]
    fn foundry_adapter_preserves_frozen_capital_actor_site_deadline_and_payload() {
        let cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let current = cost - 17;
        let original = foundry(UnitId(4), current, 17);
        let proposal = foundry_investment_proposal(original.clone())
            .expect("the Foundry footprint and claims are valid");

        assert_eq!(
            proposal.key(),
            ProposalKey::FoundryExpansion(FoundryExpansionKey {
                anchor: original.anchor()
            })
        );
        assert_eq!(proposal.case(), ProposalCase::from(foundry_case()));
        assert_eq!(proposal.claims().current_scrap(), 0);
        assert!(proposal.claims().forecast_scrap().is_empty());
        assert_eq!(
            proposal.claims().deferrable_capital(),
            Some(DeferrableCapitalClaim {
                through: DEADLINE,
                amount: cost,
            })
        );
        assert_eq!(proposal.claims().builders(), &[UnitId(4)]);
        assert_eq!(proposal.claims().sites(), &[proposal_site(&original)]);
        assert!(proposal.claims().producer_jobs().is_empty());

        match proposal.payload() {
            DomainPayload::Foundry(payload) => assert_eq!(payload, &original),
            DomainPayload::Connected(_) | DomainPayload::StandingForce(_) => {
                panic!("the Foundry adapter returned the wrong domain payload");
            }
        }
    }

    #[test]
    fn standing_force_adapter_preserves_immediate_current_only_request_and_payload() {
        let case = ProposalCase {
            urgency: Urgency::Pressing,
            confidence: Confidence::Current,
            value: StrategicValue::Material,
            time_to_impact: TimeToImpact::Immediate,
            safety: ExecutionSafety::Secure,
        };
        let original = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: NOW,
            ready_before: DEADLINE,
            kind: UnitKind::Warden,
            reason: StandingForceReason::GroundPressure,
            specialty: Specialty::Support,
            personality_emphasis: 67,
            case,
            eligible_producers: vec![BuildingId(9), BuildingId(3), BuildingId(9)],
        })
        .with_minimum_residual_scrap(150);
        let proposal = standing_force_investment_proposal(original.clone())
            .expect("one immediate producer request is a valid claim bundle");

        assert_eq!(
            proposal.key(),
            ProposalKey::StandingForce(StandingForceKey::fixture(UnitKind::Warden))
        );
        assert_eq!(proposal.case(), case);
        assert_eq!(proposal.personality_preference(), Some(67));
        assert_eq!(proposal.claims().current_scrap(), 0);
        assert_eq!(proposal.claims().minimum_residual_scrap(), 150);
        assert!(proposal.claims().forecast_scrap().is_empty());
        assert!(proposal.claims().builders().is_empty());
        assert!(proposal.claims().units().is_empty());
        assert!(proposal.claims().sites().is_empty());
        let [job] = proposal.claims().producer_jobs() else {
            panic!("the adapter must retain exactly one immediate producer request")
        };
        assert_eq!(job.kind(), UnitKind::Warden);
        assert_eq!(job.enqueue_not_before(), NOW);
        assert_eq!(job.enqueue_not_after(), NOW);
        assert_eq!(job.ready_before(), DEADLINE);
        assert!(job.requires_current_funding());
        assert_eq!(job.eligible_producers(), &[BuildingId(3), BuildingId(9)]);

        match proposal.payload() {
            DomainPayload::StandingForce(payload) => assert_eq!(payload, &original),
            DomainPayload::Foundry(_) | DomainPayload::Connected(_) => {
                panic!("the standing-force adapter returned the wrong domain payload");
            }
        }
    }

    #[test]
    fn standing_force_adapter_makes_provider_wait_compete_as_capital_not_a_command() {
        let original = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: NOW,
            ready_before: DEADLINE,
            kind: UnitKind::Warden,
            reason: StandingForceReason::ForceProjection,
            specialty: Specialty::Support,
            personality_emphasis: 67,
            case: ProposalCase {
                urgency: Urgency::Developmental,
                confidence: Confidence::Supported,
                value: StrategicValue::Material,
                time_to_impact: TimeToImpact::Near,
                safety: ExecutionSafety::Secure,
            },
            eligible_producers: vec![BuildingId(3)],
        })
        .with_accumulation(DEADLINE, UnitKind::Sentinel.stats().cost);
        let proposal = standing_force_investment_proposal(original.clone())
            .expect("a bounded wait is valid deferrable capital");

        assert!(proposal.claims().producer_jobs().is_empty());
        assert_eq!(
            proposal.claims().current_scrap(),
            UnitKind::Sentinel.stats().cost
        );
        assert_eq!(
            proposal.claims().forecast_scrap(),
            &[ForecastClaim {
                through: DEADLINE,
                amount: UnitKind::Warden
                    .stats()
                    .cost
                    .saturating_sub(UnitKind::Sentinel.stats().cost),
            }]
        );
        assert_eq!(proposal.claims().deferrable_capital(), None);
        assert!(matches!(
            proposal.payload(),
            DomainPayload::StandingForce(payload) if payload == &original
        ));
    }

    #[test]
    fn competing_investment_can_displace_provider_wait_without_hiding_fallback() {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let builder = UnitId(1);
        let producer = BuildingId(3);
        let case = ProposalCase {
            urgency: Urgency::Timely,
            confidence: Confidence::Supported,
            value: StrategicValue::Material,
            time_to_impact: TimeToImpact::Near,
            safety: ExecutionSafety::Secure,
        };
        let wait = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: NOW,
            ready_before: DEADLINE,
            kind: UnitKind::Warden,
            reason: StandingForceReason::ForceProjection,
            specialty: Specialty::Support,
            personality_emphasis: 60,
            case,
            eligible_producers: vec![producer],
        })
        .with_accumulation(DEADLINE, UnitKind::Warden.stats().cost);
        let fallback = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: NOW,
            ready_before: DEADLINE,
            kind: UnitKind::Sentinel,
            reason: StandingForceReason::ForceProjection,
            specialty: Specialty::Fortification,
            personality_emphasis: 60,
            case,
            eligible_producers: vec![producer],
        });
        let mut proposals = vec![
            foundry_investment_proposal(foundry(builder, foundry_cost, 0))
                .expect("the Foundry request adapts"),
        ];
        proposals.extend(
            standing_force_investment_proposals(vec![wait, fallback])
                .expect("the wait and fallback adapt as alternatives"),
        );
        let producer = ProducerPlanningProjection::fixture(
            producer,
            NOW,
            12,
            NOW,
            vec![NOW],
            vec![UnitKind::Sentinel],
        )
        .expect("the producer fixture is valid");
        let result = allocate(
            &capacity(
                foundry_cost.saturating_add(UnitKind::Sentinel.stats().cost),
                vec![builder],
                vec![BuilderResource {
                    id: builder,
                    kind: UnitKind::Harvester,
                    obligation: None,
                }],
                vec![producer],
            ),
            Vec::new(),
            proposals,
            AllocationPersonality::default(),
        )
        .expect("the Foundry and fallback form a feasible portfolio");

        assert!(
            result
                .accepted
                .iter()
                .any(|proposal| { matches!(proposal.key(), ProposalKey::FoundryExpansion(_)) })
        );
        assert!(result.accepted.iter().any(|proposal| {
            proposal.key()
                == ProposalKey::StandingForce(StandingForceKey::fixture(UnitKind::Sentinel))
        }));
        assert!(!result.accepted.iter().any(|proposal| {
            proposal.key()
                == ProposalKey::StandingForce(StandingForceKey::fixture(UnitKind::Warden))
        }));
    }

    #[test]
    fn standing_force_adapter_retains_the_domains_best_first_alternative_order() {
        let case = ProposalCase {
            urgency: Urgency::Timely,
            confidence: Confidence::Supported,
            value: StrategicValue::Material,
            time_to_impact: TimeToImpact::Near,
            safety: ExecutionSafety::Managed,
        };
        let preferred = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: NOW,
            ready_before: DEADLINE,
            kind: UnitKind::Warden,
            reason: StandingForceReason::GroundPressure,
            specialty: Specialty::Support,
            personality_emphasis: 73,
            case,
            eligible_producers: vec![BuildingId(3)],
        });
        let fallback = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: NOW,
            ready_before: DEADLINE,
            kind: UnitKind::Sentinel,
            reason: StandingForceReason::GroundPressure,
            specialty: Specialty::Fortification,
            personality_emphasis: 61,
            case,
            eligible_producers: vec![BuildingId(3)],
        });

        let proposals = standing_force_investment_proposals(vec![preferred, fallback])
            .expect("each alternative adapts to one immediate claim");

        assert_eq!(
            proposals
                .iter()
                .map(|proposal| (proposal.key(), proposal.domain_preference()))
                .collect::<Vec<_>>(),
            vec![
                (
                    ProposalKey::StandingForce(StandingForceKey::fixture(UnitKind::Warden)),
                    0,
                ),
                (
                    ProposalKey::StandingForce(StandingForceKey::fixture(UnitKind::Sentinel)),
                    1,
                ),
            ]
        );
    }

    #[test]
    fn air_standing_emphasis_competes_with_economy_as_proposal_specific_personality() {
        let kind = UnitKind::Warden;
        let producer = BuildingId(3);
        let standing = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: NOW,
            ready_before: DEADLINE,
            kind,
            reason: StandingForceReason::AirDefense,
            specialty: Specialty::Air,
            personality_emphasis: 90,
            case: foundry_case().into(),
            eligible_producers: vec![producer],
        });
        let standing = standing_force_investment_proposal(standing)
            .expect("the immediate standing request adapts");
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let foundry = foundry_investment_proposal(foundry(UnitId(1), foundry_cost, 0))
            .expect("the exact Foundry request adapts");
        let producer =
            ProducerPlanningProjection::fixture(producer, NOW, 12, NOW, vec![NOW], vec![kind])
                .expect("the producer fixture is valid");
        let result = allocate(
            &capacity(
                foundry_cost.max(kind.stats().cost),
                vec![UnitId(1)],
                vec![BuilderResource {
                    id: UnitId(1),
                    kind: UnitKind::Harvester,
                    obligation: None,
                }],
                vec![producer],
            ),
            Vec::new(),
            vec![foundry, standing],
            AllocationPersonality {
                economy: 80,
                offense: 0,
                standing_force: 0,
            },
        )
        .expect("both proposals are independently valid");

        assert_eq!(result.accepted.len(), 1);
        assert!(matches!(
            result.accepted[0].key(),
            ProposalKey::StandingForce(_)
        ));
        assert_eq!(result.decisions[0].personality_weight, 180);
        assert_eq!(result.decisions[1].personality_weight, 190);
    }

    #[test]
    fn accepted_foundry_payload_receives_the_jointly_assigned_funding_split() {
        let cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let builder = UnitId(4);
        let original = foundry(builder, cost - 17, 17);
        let proposal = foundry_investment_proposal(original.clone()).unwrap();
        let resources = AllocationCapacity::fixture(
            ResourcePlanningProjection::fixture(ResourcePlanningFixture {
                current_scrap: cost - 17,
                observed_at: NOW,
                horizon: DEADLINE,
                cadence: 12,
                forecast_income: vec![crate::bot::resources::ForecastAvailability {
                    available_at: DEADLINE,
                    amount: 27,
                }],
                units: vec![builder],
                builders: vec![BuilderResource {
                    id: builder,
                    kind: UnitKind::Harvester,
                    obligation: None,
                }],
                producers: Vec::new(),
            })
            .unwrap(),
        );
        let survival = ImportedObligation {
            class: ObligationClass::Survival,
            accepted_at: NOW,
            key: ObligationKey::OpeningCore { sequence: 0 },
            claims: ClaimBundle::new(
                10,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
        };

        let result = allocate(
            &resources,
            vec![survival],
            vec![proposal],
            AllocationPersonality::default(),
        )
        .expect("the Foundry total remains feasible after the survival reserve");
        let assignment = result.capital_assignments[0];
        assert_eq!(assignment.current_scrap, cost - 27);
        assert_eq!(assignment.forecast_scrap, 27);
        let mut payloads = result.into_domain_payloads();
        let rebound = payloads.take_foundry().expect("the Foundry was accepted");
        assert_eq!(rebound.anchor(), original.anchor());
        assert_eq!(rebound.builder(), original.builder());
        assert_eq!(rebound.case(), original.case());
        assert_eq!(rebound.current_construction_capital(), cost - 27);
        assert_eq!(rebound.forecast_construction_capital(), 27);
    }

    #[test]
    fn connected_jobs_keep_domain_order_and_are_charged_exactly_once() {
        let kind = UnitKind::Moth;
        let job = crate::bot::strategy::ConnectedProviderJob::fixture(
            kind,
            NOW,
            DEADLINE,
            vec![BuildingId(8)],
        );
        let original = connected(
            ConnectedOffenseClaims::fixture(Vec::new(), vec![job]),
            Vec::new(),
        );
        let proposal = connected_investment_proposal(original.clone())
            .expect("the exact connected claims are valid");
        assert_eq!(proposal.claims().current_scrap(), 0);
        assert!(proposal.claims().forecast_scrap().is_empty());
        assert_eq!(
            proposal.claims().claimed_capital(),
            u128::from(kind.stats().cost)
        );
        assert_eq!(proposal.claims().producer_jobs()[0].kind(), kind);

        let producer =
            ProducerPlanningProjection::fixture(BuildingId(8), NOW, 12, NOW, vec![NOW], vec![kind])
                .expect("the producer fixture is valid");
        let result = allocate(
            &capacity(kind.stats().cost, Vec::new(), Vec::new(), vec![producer]),
            Vec::new(),
            vec![proposal],
            AllocationPersonality::default(),
        )
        .expect("one exact unit cost funds the job");
        assert_eq!(result.final_producer_schedule().len(), 1);
        assert_eq!(
            result.final_producer_schedule()[0].current_scrap,
            kind.stats().cost
        );
        let mut payloads = result.into_domain_payloads();
        assert_eq!(payloads.take_connected(), Some(original));
    }

    #[test]
    fn marginal_extension_keeps_the_exact_domain_variant_and_adds_only_its_jobs() {
        let minimum_kind = UnitKind::Moth;
        let marginal_kind = UnitKind::Condor;
        let minimum = ConnectedOffenseClaims::fixture(
            Vec::new(),
            vec![crate::bot::strategy::ConnectedProviderJob::fixture(
                minimum_kind,
                NOW,
                DEADLINE,
                vec![BuildingId(8)],
            )],
        );
        let addition = ConnectedOffenseClaims::fixture(
            Vec::new(),
            vec![crate::bot::strategy::ConnectedProviderJob::fixture(
                marginal_kind,
                NOW,
                DEADLINE,
                vec![BuildingId(9)],
            )],
        );
        let original = connected(minimum, vec![addition]);
        let marginal = original.marginal_variants()[0].clone();
        let mut expected = original.clone();
        assert!(expected.select_marginal(&marginal));
        let proposal =
            connected_investment_proposal(original).expect("the exact connected minimum is valid");
        let producers = [
            (BuildingId(8), minimum_kind),
            (BuildingId(9), marginal_kind),
        ]
        .into_iter()
        .map(|(producer, kind)| {
            ProducerPlanningProjection::fixture(producer, NOW, 12, NOW, vec![NOW], vec![kind])
                .expect("the producer fixture is valid")
        })
        .collect();
        let mut result = allocate(
            &capacity(
                minimum_kind
                    .stats()
                    .cost
                    .saturating_add(marginal_kind.stats().cost),
                Vec::new(),
                Vec::new(),
                producers,
            ),
            Vec::new(),
            vec![proposal],
            AllocationPersonality::default(),
        )
        .expect("the exact connected minimum allocates");
        let claims = result
            .try_accept_connected_marginal(
                &capacity(
                    minimum_kind
                        .stats()
                        .cost
                        .saturating_add(marginal_kind.stats().cost),
                    Vec::new(),
                    Vec::new(),
                    [
                        (BuildingId(8), minimum_kind),
                        (BuildingId(9), marginal_kind),
                    ]
                    .into_iter()
                    .map(|(producer, kind)| {
                        ProducerPlanningProjection::fixture(
                            producer,
                            NOW,
                            12,
                            NOW,
                            vec![NOW],
                            vec![kind],
                        )
                        .expect("the producer fixture is valid")
                    })
                    .collect(),
                ),
                &marginal,
            )
            .expect("the independent marginal lane and capital fit atomically");
        assert_eq!(claims.producer_jobs().len(), 1);
        assert_eq!(claims.producer_jobs()[0].kind(), marginal_kind);
        assert_eq!(result.final_producer_schedule().len(), 2);
        let mut payloads = result.into_domain_payloads();
        assert_eq!(payloads.take_connected(), Some(expected));
    }

    #[test]
    fn active_revision_minimum_is_mandatory_while_foundry_and_marginal_remain_compatible() {
        let minimum_kind = UnitKind::Moth;
        let marginal_kind = UnitKind::Condor;
        let builder = UnitId(4);
        let revision = connected(
            ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![crate::bot::strategy::ConnectedProviderJob::fixture(
                    minimum_kind,
                    NOW,
                    DEADLINE,
                    vec![BuildingId(8)],
                )],
            ),
            vec![ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![crate::bot::strategy::ConnectedProviderJob::fixture(
                    marginal_kind,
                    NOW,
                    DEADLINE,
                    vec![BuildingId(9)],
                )],
            )],
        )
        .into_active_revision_fixture();
        let marginal = revision.marginal_variants()[0].clone();
        let obligation = active_connected_revision_obligation(&revision)
            .expect("the revision minimum adapts as retained work");
        let carrier = active_connected_revision_investment_proposal(revision);
        let foundry = foundry_investment_proposal(foundry(builder, 50, 0))
            .expect("the compatible Foundry adapts exactly");
        let producers = [
            (BuildingId(8), minimum_kind),
            (BuildingId(9), marginal_kind),
        ]
        .into_iter()
        .map(|(producer, kind)| {
            ProducerPlanningProjection::fixture(producer, NOW, 12, NOW, vec![NOW], vec![kind])
                .expect("the producer fixture is valid")
        })
        .collect::<Vec<_>>();
        let resources = capacity(
            50_u32
                .saturating_add(minimum_kind.stats().cost)
                .saturating_add(marginal_kind.stats().cost),
            vec![builder],
            vec![BuilderResource {
                id: builder,
                kind: UnitKind::Harvester,
                obligation: None,
            }],
            producers,
        );
        let mut result = allocate(
            &resources,
            vec![obligation],
            vec![foundry, carrier],
            AllocationPersonality::default(),
        )
        .expect("mandatory revision and compatible Foundry fit together");
        assert_eq!(result.accepted.len(), 2);
        result
            .try_accept_connected_marginal(&resources, &marginal)
            .expect("the marginal uses residual capacity after the retained minimum");
        let schedule = result.final_producer_schedule().to_vec();
        let mut payloads = result.into_domain_payloads();
        assert!(payloads.take_foundry().is_some());
        let mut revision = payloads
            .take_connected()
            .expect("the zero-claim carrier cannot lose to the empty portfolio");
        let assignments =
            super::super::active_connected_revision_producer_assignments(&revision, &schedule);
        assert_eq!(
            assignments
                .iter()
                .map(|assignment| (assignment.request_ordinal(), assignment.kind()))
                .collect::<Vec<_>>(),
            vec![(0, minimum_kind), (1, marginal_kind)]
        );
        revision
            .bind_producer_assignments(assignments)
            .expect("the two allocation owners reassemble into one exact package");
    }

    #[test]
    fn real_domain_semantic_tie_is_decided_by_positive_personality_emphasis() {
        let actor = UnitId(4);
        let foundry = foundry_investment_proposal(FreshFoundryProposal::fixture(
            TilePos::new(17, 9),
            actor,
            50,
            0,
            23,
            DEADLINE,
            FoundryOpportunityCase::fixture(
                FoundryUrgency::Timely,
                FoundryConfidence::Corroborated,
                FoundryStrategicValue::Material,
                FoundryTimeToImpact::Near,
                FoundryExecutionSafety::Secure,
            ),
        ))
        .expect("the Foundry proposal is valid");
        let offense = connected_investment_proposal(connected(
            ConnectedOffenseClaims::fixture(vec![actor], Vec::new()),
            Vec::new(),
        ))
        .expect("the connected proposal is valid");
        assert_eq!(foundry.case(), offense.case());
        let resources = capacity(
            50,
            vec![actor],
            vec![BuilderResource {
                id: actor,
                kind: UnitKind::Harvester,
                obligation: None,
            }],
            Vec::new(),
        );
        let choose = |personality| {
            allocate(
                &resources,
                Vec::new(),
                vec![foundry.clone(), offense.clone()],
                personality,
            )
            .expect("both tied domains are independently feasible")
            .accepted[0]
                .key()
        };
        assert!(matches!(
            choose(AllocationPersonality {
                economy: 90,
                offense: 10,
                standing_force: 0,
            }),
            ProposalKey::FoundryExpansion(_)
        ));
        assert!(matches!(
            choose(AllocationPersonality {
                economy: 10,
                offense: 90,
                standing_force: 0,
            }),
            ProposalKey::ConnectedOffenseMinimum(_)
        ));
    }

    #[test]
    fn profile_adapter_balances_air_and_siege_without_erasing_either_domain() {
        let profile = ResolvedProfile {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            personality_seed: 7,
            primary: Specialty::Air,
            secondary: Specialty::Siege,
            traits: PersonalityTraits {
                air: 80,
                siege: 40,
                support: 50,
                fortification: 50,
                greed: 73,
                guile: 50,
            },
        };
        assert_eq!(
            AllocationPersonality::from_profile(&profile),
            AllocationPersonality {
                economy: 73,
                offense: 60,
                standing_force: 50,
            }
        );
    }
}
