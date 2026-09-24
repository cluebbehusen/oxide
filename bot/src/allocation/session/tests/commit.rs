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
    initial.commit_repair_assignment(assignment.clone(), &mut Vec::new());
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
