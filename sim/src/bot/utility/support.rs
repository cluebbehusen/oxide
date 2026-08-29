//! Mobile sustain for player-facing support identities.

use std::cmp::Reverse;

use super::*;

impl UtilityPolicy {
    /// Sends idle Tenders to the most damaged ground combatants. Existing
    /// repair orders are left alone because their welders are not idle.
    pub(super) fn mobile_support(
        &self,
        dials: &Dials,
        obs: &Observation,
        player_facing: bool,
        intents: &mut Vec<Intent>,
    ) {
        if !dials.adaptive_composition
            || !dials.repair
            || obs.scrap < UnitKind::Sentinel.stats().cost
        {
            return;
        }

        let mut patients: Vec<&UnitObs> = obs
            .my_units
            .iter()
            .filter(|unit| is_mobile_support_patient(unit))
            .collect();
        patients.sort_by_key(|unit| {
            let stats = unit.kind.stats();
            (
                unit.hp.saturating_mul(1_000) / stats.max_hp.max(1),
                Reverse(stats.cost),
                unit.id,
            )
        });

        let mut tenders: Vec<&UnitObs> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Tender && unit.idle)
            .collect();
        tenders.sort_by_key(|unit| unit.id);
        let mut routes = crate::bot::routing::RouteProjection::known_ground(obs);

        for patient in patients.into_iter().take(dials.support_target) {
            let Some((index, _)) = tenders
                .iter()
                .enumerate()
                .filter(|(_, tender)| !player_facing || routes.unit_reaches(tender, patient.tile))
                .min_by_key(|(_, tender)| (tender.tile.manhattan(patient.tile), tender.id))
            else {
                break;
            };
            let tender = tenders.remove(index);
            intents.push(Intent::RepairUnits {
                welders: vec![tender.id],
                target: patient.id,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::OBSERVATION_VERSION;
    use crate::ids::{PlayerId, UnitId};
    use crate::state::Faction;

    fn unit(id: u32, kind: UnitKind, tile: TilePos, hp: u32) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind,
            tile,
            hp,
            idle: true,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn observation() -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 200,
            map_width: 20,
            map_height: 12,
            my_units: vec![
                unit(2, UnitKind::Tender, TilePos::new(2, 2), 150),
                unit(5, UnitKind::Tender, TilePos::new(12, 8), 150),
                unit(10, UnitKind::Sentinel, TilePos::new(4, 2), 20),
                unit(11, UnitKind::Bombard, TilePos::new(11, 8), 10),
            ],
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![true; 20 * 12],
            explored: vec![true; 20 * 12],
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
        }
    }

    #[test]
    fn idle_tenders_pair_with_wounded_combatants_by_need_then_distance() {
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        dials.support_target = 2;
        let mut intents = Vec::new();

        UtilityPolicy::new().mobile_support(&dials, &observation(), true, &mut intents);

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
    fn mobile_support_preserves_the_fighting_reserve() {
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let mut obs = observation();
        obs.scrap = UnitKind::Sentinel.stats().cost - 1;
        let mut intents = Vec::new();

        UtilityPolicy::new().mobile_support(&dials, &obs, true, &mut intents);

        assert!(intents.is_empty());
    }

    #[test]
    fn player_facing_support_refuses_a_patient_behind_a_known_wall() {
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(14, 5), 20),
        ];
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(8, y)).collect();
        let mut intents = Vec::new();

        UtilityPolicy::new().mobile_support(&dials, &obs, true, &mut intents);

        assert!(intents.is_empty());
    }

    #[test]
    fn player_facing_support_does_not_imagine_a_road_across_unexplored_ground() {
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(14, 5), 20),
        ];
        for y in 0..obs.map_height {
            for x in 5..12 {
                obs.explored[(y * obs.map_width + x) as usize] = false;
            }
        }
        let mut intents = Vec::new();

        UtilityPolicy::new().mobile_support(&dials, &obs, true, &mut intents);

        assert!(intents.is_empty());
    }

    #[test]
    fn player_facing_support_uses_a_local_route_on_an_otherwise_unknown_map() {
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(6, 5), 20),
        ];
        obs.explored.fill(false);
        for x in 2..=6 {
            obs.explored[(5 * obs.map_width + x) as usize] = true;
        }
        let mut intents = Vec::new();

        UtilityPolicy::new().mobile_support(&dials, &obs, true, &mut intents);

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
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
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

        UtilityPolicy::new().mobile_support(&dials, &obs, true, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::RepairUnits {
                welders: vec![UnitId(2)],
                target: UnitId(10),
            }]
        );
    }

    #[test]
    fn profile_free_support_keeps_its_historical_route_agnostic_assignment() {
        let mut dials = Dials::balanced();
        dials.adaptive_composition = true;
        let mut obs = observation();
        obs.my_units = vec![
            unit(2, UnitKind::Tender, TilePos::new(2, 5), 150),
            unit(10, UnitKind::Sentinel, TilePos::new(14, 5), 20),
        ];
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(8, y)).collect();
        let mut intents = Vec::new();

        UtilityPolicy::new().mobile_support(&dials, &obs, false, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::RepairUnits {
                welders: vec![UnitId(2)],
                target: UnitId(10),
            }]
        );
    }
}
