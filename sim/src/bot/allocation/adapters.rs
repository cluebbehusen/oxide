//! Exact translations between domain-owned proposals and shared allocation.

use super::{
    AllocationConflict, AllocationPersonality, AllocationResult, ClaimBundle, ClaimBundleError,
    Confidence, ConnectedOffenseKey, DeferrableCapitalClaim, ExecutionSafety, FoundryExpansionKey,
    InvestmentProposal, ProducerJobClaim, ProposalCase, ScheduledProducerJob, StrategicValue,
    TimeToImpact, Urgency,
};
use crate::bot::profile::ResolvedProfile;
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

/// The two exact payload types compared during the first allocation migration.
pub(crate) type DomainInvestmentProposal =
    InvestmentProposal<FreshFoundryProposal, FreshConnectedProposal>;

/// Allocation output retaining the exact expansion and connected-operation plans.
pub(crate) type DomainAllocationResult =
    AllocationResult<FreshFoundryProposal, FreshConnectedProposal>;

impl AllocationPersonality {
    /// Resolves positive cross-domain emphasis without granting or removing work.
    pub(crate) const fn from_profile(profile: &ResolvedProfile) -> Self {
        Self {
            economy: profile.traits.greed as u16,
            offense: (profile.traits.air as u16 + profile.traits.siege as u16) / 2,
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
    Ok(InvestmentProposal::FoundryExpansion {
        key: FoundryExpansionKey {
            anchor: proposal.anchor(),
        },
        case: proposal.case().into(),
        claims,
        payload: proposal,
    })
}

/// Retains one connected domain's exact minimum and ordered producer choices.
pub(crate) fn connected_investment_proposal(
    proposal: FreshConnectedProposal,
) -> Result<DomainInvestmentProposal, ClaimBundleError> {
    let claims = connected_claim_bundle(proposal.minimum_claims())?;
    Ok(InvestmentProposal::ConnectedOffenseMinimum {
        key: ConnectedOffenseKey {
            objective: proposal.objective(),
            anchor: proposal.anchor(),
        },
        case: proposal.case().into(),
        accepted_at: proposal.accepted_at(),
        claims,
        payload: proposal,
    })
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
        self.accepted.iter().find_map(|proposal| match proposal {
            InvestmentProposal::ConnectedOffenseMinimum { key, .. } => Some(*key),
            InvestmentProposal::FoundryExpansion { .. } => None,
        })
    }

    /// Retained cumulative scale choices, still owned by the domain payload.
    pub(crate) fn connected_marginal_variants(&self) -> Option<&[ConnectedMarginalVariant]> {
        self.accepted.iter().find_map(|proposal| match proposal {
            InvestmentProposal::ConnectedOffenseMinimum { payload, .. } => {
                Some(payload.marginal_variants())
            }
            InvestmentProposal::FoundryExpansion { .. } => None,
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
            .find_map(|proposal| match proposal {
                InvestmentProposal::ConnectedOffenseMinimum { key, payload, .. } => {
                    Some((*key, payload.marginal_variants().contains(marginal)))
                }
                InvestmentProposal::FoundryExpansion { .. } => None,
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
            .find_map(|proposal| match proposal {
                InvestmentProposal::ConnectedOffenseMinimum { payload, .. } => Some(payload),
                InvestmentProposal::FoundryExpansion { .. } => None,
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
            match proposal {
                InvestmentProposal::FoundryExpansion {
                    claims,
                    mut payload,
                    ..
                } => {
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
                InvestmentProposal::ConnectedOffenseMinimum { payload, .. } => {
                    debug_assert!(payloads.connected.is_none());
                    payloads.connected = Some(payload);
                }
            }
        }
        payloads
    }

    /// Final deterministic schedule after every accepted marginal extension.
    pub(crate) fn final_producer_schedule(&self) -> &[ScheduledProducerJob] {
        &self.producer_schedule
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::allocation::{
        AllocationCapacity, ImportedObligation, ObligationClass, ObligationKey, ProposalKey,
        allocate,
    };
    use crate::bot::profile::{PersonalityTraits, Specialty};
    use crate::bot::resources::{
        BuilderResource, ProducerPlanningProjection, ResourcePlanningFixture,
        ResourcePlanningProjection,
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

        match proposal {
            InvestmentProposal::FoundryExpansion { payload, .. } => {
                assert_eq!(payload, original);
            }
            InvestmentProposal::ConnectedOffenseMinimum { .. } => {
                panic!("the Foundry adapter returned the wrong domain variant");
            }
        }
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
                offense: 10
            }),
            ProposalKey::FoundryExpansion(_)
        ));
        assert!(matches!(
            choose(AllocationPersonality {
                economy: 10,
                offense: 90
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
                offense: 60
            }
        );
    }
}
