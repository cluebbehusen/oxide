//! Focused fixtures for maintained repair proposals and assignments.

use super::*;
#[cfg(test)]
use crate::observation::ObservationData;

impl UtilityPolicy {
    fn test_admit_repairs(
        &mut self,
        obs: &Observation,
        mode: PolicyMode<'_>,
        available: u32,
        buildings: bool,
        intents: &mut Vec<Intent>,
    ) {
        let map = super::tests::public_map(obs);
        let profile =
            crate::profile::ResolvedProfile::resolve(oxide_sim::scenario::BotConfig::default());
        let resources = ResourceSnapshot::from_observation(obs);
        let mut unavailable = self.worker_safety_reservations().to_vec();
        for intent in intents.iter() {
            Self::claim_non_preemptible_intent_units(intent, &mut unavailable);
        }
        let context = EconomicInvestmentContext {
            evidence: Default::default(),
            obligations: &[],
            obs,
            resources: &resources,
            profile: &profile,
            briefing: mode.public_map.unwrap_or(&map),
            orientation: super::super::orient::Orientation::for_home(
                obs,
                obs.my_buildings
                    .first()
                    .map_or(TilePos::new(0, 0), |b| b.anchor),
            ),
            unavailable: &unavailable,
            demands: &[],
            unit_contacts: mode.unit_contacts.unwrap_or(&[]),
            building_contacts: mode.building_contacts.unwrap_or(&[]),
            cadence: 24,
            protected_scrap: 0,
            air_work: &[],
        };
        let snapshot = self.support_work_snapshot(context);
        self.observe_support_work(&snapshot, obs.tick);
        let renewed = self.renew_prepared_repairs(context, &snapshot, available, true);
        self.commit_repair_renewals(renewed.clone(), obs.tick);
        let mut remaining = available.saturating_sub(renewed.iter().map(|p| p.debit).sum());
        for candidate in self.prepared_repair_assignments(context, &snapshot) {
            if matches!(candidate.key.patient, oxide_sim::Target::Building(_)) == buildings
                && candidate.debit <= remaining
            {
                let worker_busy = self
                    .state
                    .support_work
                    .repairs
                    .iter()
                    .any(|work| work.key.worker == candidate.key.worker);
                if !worker_busy {
                    remaining -= candidate.debit;
                    self.commit_repair_assignment(candidate, intents);
                }
            }
        }
    }

    pub(super) fn test_admit_building_repairs(
        &mut self,
        obs: &Observation,
        mode: PolicyMode<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        self.test_admit_repairs(obs, mode, *budget, true, intents);
    }

    pub(super) fn test_admit_mobile_repairs(
        &mut self,
        obs: &Observation,
        available: u32,
        intents: &mut Vec<Intent>,
    ) {
        self.test_admit_repairs(
            obs,
            PolicyMode {
                evidence: Default::default(),
                ground_missions: None,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            available,
            false,
            intents,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use oxide_sim::ids::{PlayerId, UnitId};

    fn unit(id: u32, kind: UnitKind, tile: TilePos, hp: u32) -> UnitObs {
        UnitObs {
            hp,
            ..crate::test_support::unit(id, PlayerId(0), kind, tile)
        }
    }

    fn observation() -> Observation {
        Observation::from_data(ObservationData {
            tick: 0,
            scrap: 200,
            map_width: 20,
            map_height: 12,
            my_units: vec![
                unit(2, UnitKind::Tender, TilePos::new(2, 2), 150),
                unit(5, UnitKind::Tender, TilePos::new(12, 8), 150),
                unit(10, UnitKind::Sentinel, TilePos::new(4, 2), 20),
                unit(11, UnitKind::Bombard, TilePos::new(11, 8), 10),
            ],
            visible: vec![true; 20 * 12],
            explored: vec![true; 20 * 12],
            ..crate::test_support::observation_data()
        })
    }

    #[test]
    fn building_repairs_require_funding_and_do_not_reverse_active_salvage() {
        let state = oxide_sim::Scenario::skirmish().build().unwrap();
        let mut obs = Observation::omniscient(&state, PlayerId(0));
        let foundry = obs
            .my_buildings
            .iter_mut()
            .find(|building| building.kind == BuildingKind::Foundry)
            .unwrap();
        foundry.hp /= 2;
        let target = foundry.id;
        let admit = |obs: &Observation, budget| {
            let mut intents = Vec::new();
            UtilityPolicy::new().test_admit_building_repairs(
                obs,
                PolicyMode {
                    evidence: Default::default(),
                    ground_missions: None,
                    admit_voluntary_macro: true,
                    unit_contacts: None,
                    building_contacts: None,
                    public_map: None,
                },
                &mut { budget },
                &mut intents,
            );
            intents
        };
        assert!(admit(&obs, 0).is_empty());
        assert!(matches!(admit(&obs, 1_000).as_slice(),
            [Intent::RepairWith { building, .. }] if *building == target));
        obs.my_units[0].salvaging = Some(target);
        assert!(admit(&obs, 1_000).is_empty());
        obs.my_units[0].salvaging = None;
        assert!(matches!(admit(&obs, 1_000).as_slice(),
            [Intent::RepairWith { building, .. }] if *building == target));
    }

    #[test]
    fn idle_tenders_pair_with_wounded_combatants_by_need_then_distance() {
        let mut intents = Vec::new();

        UtilityPolicy::new().test_admit_mobile_repairs(&observation(), 200, &mut intents);

        assert_eq!(
            intents,
            vec![
                Intent::RepairUnits {
                    welders: vec![UnitId(5)],
                    target: UnitId(11),
                },
                Intent::RepairUnits {
                    welders: vec![UnitId(2)],
                    target: UnitId(10),
                },
            ]
        );
    }

    #[test]
    fn mobile_support_uses_the_uncommitted_budget_not_the_gross_bank() {
        let obs = observation();
        let mut intents = Vec::new();

        UtilityPolicy::new().test_admit_mobile_repairs(&obs, 0, &mut intents);

        assert!(intents.is_empty());
    }

    #[test]
    fn player_facing_support_refuses_a_patient_behind_a_known_wall() {
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(14, 5), 20),
        ];
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(8, y)).collect();
        let mut intents = Vec::new();

        UtilityPolicy::new().test_admit_mobile_repairs(&obs, obs.scrap, &mut intents);

        assert!(intents.is_empty());
    }

    #[test]
    fn support_uses_authored_terrain_through_unexplored_ground() {
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(14, 5), 20),
        ];
        for y in 0..obs.map_height {
            for x in 5..12 {
                {
                    let obs = &mut *obs;
                    obs.explored[(y * obs.map_width + x) as usize] = false;
                }
            }
        }
        let mut intents = Vec::new();

        UtilityPolicy::new().test_admit_mobile_repairs(&obs, obs.scrap, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::RepairUnits {
                welders: vec![UnitId(2)],
                target: UnitId(10)
            }]
        );
    }

    #[test]
    fn player_facing_support_uses_a_local_route_on_an_otherwise_unknown_map() {
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(6, 5), 20),
        ];
        obs.explored.fill(false);
        for x in 2..=6 {
            {
                let obs = &mut *obs;
                obs.explored[(5 * obs.map_width + x) as usize] = true;
            }
        }
        let mut intents = Vec::new();

        UtilityPolicy::new().test_admit_mobile_repairs(&obs, obs.scrap, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::RepairUnits {
                welders: vec![UnitId(2)],
                target: UnitId(10),
            }]
        );
    }

    #[test]
    fn player_facing_support_uses_a_known_gap() {
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(14, 5), 20),
        ];
        obs.known_rock = (0..obs.map_height)
            .filter(|y| *y != 5)
            .map(|y| TilePos::new(8, y))
            .collect();
        let mut intents = Vec::new();

        UtilityPolicy::new().test_admit_mobile_repairs(&obs, obs.scrap, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::RepairUnits {
                welders: vec![UnitId(2)],
                target: UnitId(10),
            }]
        );
    }
}
