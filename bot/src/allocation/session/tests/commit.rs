use super::*;

#[test]
fn rejected_repair_renewal_keeps_the_original_funding_deadline() {
    use crate::utility::{RepairAssignment, SupportKey};
    let home = TilePos::new(3, 10);
    let mut obs = connected_observation(1_200, 10_000);
    obs.my_buildings[0].hp /= 2;
    obs.my_units
        .push(owned_unit(900, UnitKind::Harvester, home));
    let setup = SessionProfile::new(prime_profile());
    let map = connected_briefing(&obs);
    let assignment = RepairAssignment {
        key: SupportKey {
            worker: UnitId(900),
            patient: oxide_sim::Target::Building(obs.my_buildings[0].id),
        },
        accepted_at: obs.tick,
        funded_until: obs.tick + setup.dials.cadence,
        debit: 10,
        case: ProposalCase {
            urgency: Urgency::Pressing,
            confidence: Confidence::Current,
            value: StrategicValue::Decisive,
            time_to_impact: TimeToImpact::Near,
            safety: ExecutionSafety::Managed,
        },
    };
    let mut initial = UtilityPolicy::new();
    initial
        .prepare_repair_assignment(assignment.clone(), &obs)
        .unwrap()
        .apply(&mut initial, &mut Vec::new());
    obs.tick = assignment.funded_until;
    obs.my_units.last_mut().unwrap().repairing = true;
    obs.my_repair_targets
        .push((assignment.key.worker, assignment.key.patient));
    for accept in [false, true] {
        let mut policy = initial.clone();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();

        let intelligence = StrategicIntelligence::new();
        let mut session = AllocationSession::new(
            setup.context(&obs, home, &map, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(),
            None,
        );
        let observed = session.observe_retained_work();
        let prepared = session.prepare(observed);
        assert_eq!(prepared.repair_renewals.len(), 1);
        assert_eq!(
            prepared.repair_renewals[0].funded_until,
            obs.tick + setup.dials.cadence
        );
        assert!(
            !session
                .participants
                .policy
                .repair_is_funded(UnitId(900), obs.tick)
        );
        let mut resolved = session.resolve(prepared);
        assert!(resolved.settlement.is_ok());
        if !accept {
            resolved.settlement = Err(AllocationFailure::Unsettled);
        }
        let outcome = session.finish_allocation(resolved);
        assert_eq!(outcome.allocation_ok, accept);
        assert_eq!(policy.repair_is_funded(UnitId(900), obs.tick), accept);
        let retained = policy
            .state
            .support_work
            .repairs
            .iter()
            .find(|repair| repair.key == assignment.key)
            .unwrap();
        assert_eq!(retained.accepted_at, assignment.accepted_at);
        assert_eq!(
            retained.funded_until,
            if accept {
                obs.tick + setup.dials.cadence
            } else {
                assignment.funded_until
            }
        );
    }
}

#[test]
fn late_dispatch_rejection_never_installs_an_earlier_selected_owner() {
    use crate::utility::{
        ReconConsumer, ReconObserver, ReconProposal, ReconQuestionKey, RepairAssignment, SupportKey,
    };
    use oxide_sim::Target;
    let home = TilePos::new(3, 10);
    let setup = SessionProfile::new(prime_profile());
    for rejected_owner in ["reconnaissance", "repair", "raid"] {
        let mut obs = connected_observation(1_200, 10_000);
        obs.my_units.extend([
            owned_unit(900, UnitKind::Harvester, home),
            owned_unit(901, UnitKind::Scuttler, home.offset(5, 3)),
            owned_unit(902, UnitKind::Scuttler, home.offset(6, 3)),
        ]);
        let map = connected_briefing(&obs);
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut allocation =
            CrossDomainAllocation::new(&resources, obs.tick + 10_000, setup.dials.cadence).unwrap();
        allocation.offer(connected_investment_proposal(current_connected_proposal(&obs)).unwrap());
        let case = ProposalCase {
            urgency: Urgency::Pressing,
            confidence: Confidence::Current,
            value: StrategicValue::Decisive,
            time_to_impact: TimeToImpact::Near,
            safety: ExecutionSafety::Managed,
        };
        match rejected_owner {
            "reconnaissance" => {
                // Resources fit, but the owner's current questions do not include this work.
                let proposal = ReconProposal {
                    question: serde_json::from_value(serde_json::json!({"key": ReconQuestionKey { consumer: ReconConsumer::Economy, x: 20, y: 10 },
                        "size": [1, 1], "evidence_at": obs.tick, "confidence": case.confidence, "value": case.value, "urgency": case.urgency })).unwrap(),
                    observer: ReconObserver::Live(UnitId(900)), observed_at: obs.tick,
                    deadline: obs.tick + 1_200, goal: TilePos::new(20, 10), origin: home, arrival_at: obs.tick + 120,
                };
                allocation.offer(super::super::super::reconnaissance_investment_proposal(
                    proposal,
                ));
            }
            "repair" => {
                // The patient is healthy, so a resource-compatible stale repair must reject.
                let repair = RepairAssignment {
                    key: SupportKey {
                        worker: UnitId(900),
                        patient: Target::Building(obs.my_buildings[0].id),
                    },
                    accepted_at: obs.tick,
                    funded_until: obs.tick + 24,
                    debit: 1,
                    case,
                };
                allocation.offer(super::super::super::InvestmentProposal::fresh(
                    ProposalKey::Support(repair.key),
                    case,
                    repair.claims(false),
                    super::super::super::DomainPayload::Support(repair),
                ));
            }
            "raid" => {
                let mut request = RaidPlanner::new()
                    .muster_request(
                        crate::raid::RaidPlanningContext::new(
                            &setup.profile,
                            setup.tuning,
                            &obs,
                            home,
                            &[],
                            &[],
                        ),
                        &resources,
                        Some(&map),
                        Some(Orientation::for_home(&obs, home)),
                    )
                    .unwrap();
                assert_eq!(request.missing, 0);
                request.observed_at -= 1;
                allocation.offer(
                    crate::allocation::standing_force_investment_proposal(
                        StandingForceProposal::for_raid(request, &setup.profile),
                    )
                    .unwrap(),
                );
            }
            _ => unreachable!(),
        }
        let settlement = allocation
            .resolve(AllocationPersonality::default(), None)
            .unwrap();
        assert!(!settlement.producer_schedule().is_empty());
        let mut policy = UtilityPolicy::new();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();

        let intelligence = StrategicIntelligence::new();
        let mut session = AllocationSession::new(
            setup.context(&obs, home, &map, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(),
            None,
        );
        let result = session.commit_settlement(&mut prepared(&obs, None), settlement);
        assert!(
            matches!(
                result,
                Err((
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected
                ))
            ),
            "{rejected_owner}"
        );
        assert_eq!(policy.state, UtilityPolicy::new().state, "{rejected_owner}");
        assert_eq!(strategy, StrategicPlanner::new(), "{rejected_owner}");
        assert_eq!(raids, RaidPlanner::new(), "{rejected_owner}");
        assert_eq!(lifts, LiftPlanner::new(), "{rejected_owner}");
        assert_eq!(team, TeamReliefPlanner::new(), "{rejected_owner}");
    }
}
