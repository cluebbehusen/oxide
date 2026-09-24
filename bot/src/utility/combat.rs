//! Air raids, scouting, and ground-army strategy.

use super::*;
#[cfg(test)]
use crate::observation::ObservationData;
use crate::query_work::QueryPurpose;
mod missions;
use crate::intelligence::MAX_CONFIDENCE;
use crate::observation::BuildingObs;
pub(crate) use missions::GroundMissionInputs;

fn coherent_attack_size(dials: &Dials) -> usize {
    (dials.army_size as usize).saturating_add(usize::from(true))
}

// Priced through the same salvo-aware coin as live fight estimates, so
// a remembered bomber force keeps the threat weight it demonstrated
// while visible instead of collapsing when sight is lost.
pub(super) fn utility_scout_preference(unit: &UnitObs, contested: bool) -> Option<(u8, u32)> {
    match unit.kind {
        UnitKind::Kestrel | UnitKind::Gnat => Some((0, 0)),
        UnitKind::Harvester if !contested => Some((1, unit.carrying)),
        UnitKind::Scuttler => Some((2, 0)),
        UnitKind::Sentinel => Some((3, 0)),
        _ => None,
    }
}

pub(crate) fn ground_weapon_reaches_footprint(
    unit: &UnitObs,
    anchor: TilePos,
    size: (i32, i32),
) -> bool {
    let (unit_min_x, unit_max_x) = (unit.tile.x, unit.tile.x + 1);
    let (unit_min_y, unit_max_y) = (unit.tile.y, unit.tile.y + 1);
    let (building_min_x, building_max_x) = (anchor.x, anchor.x + size.0);
    let (building_min_y, building_max_y) = (anchor.y, anchor.y + size.1);
    let axis_distances = |unit_min: i32, unit_max: i32, target_min: i32, target_max: i32| {
        let nearest = (target_min - unit_max).max(unit_min - target_max).max(0);
        let farthest = (target_min - unit_min).max(unit_max - target_max).max(0);
        (
            chassis::fx::Fx::from_num(nearest),
            chassis::fx::Fx::from_num(farthest),
        )
    };
    // Policy sees only the occupied tile, not sub-tile position. Treat the
    // tile as the uncertainty rectangle and ask whether any possible point in
    // it lies inside the simulation's Euclidean firing annulus.
    let (near_x, far_x) = axis_distances(unit_min_x, unit_max_x, building_min_x, building_max_x);
    let (near_y, far_y) = axis_distances(unit_min_y, unit_max_y, building_min_y, building_max_y);
    let nearest_sq = near_x * near_x + near_y * near_y;
    let farthest_sq = far_x * far_x + far_y * far_y;
    unit.kind.stats().weapons.iter().any(|weapon| {
        weapon.targets.covers(Domain::Ground)
            && nearest_sq <= weapon.range * weapon.range
            && farthest_sq >= weapon.minimum_range * weapon.minimum_range
    })
}

impl UtilityPolicy {
    /// Air-raid channel: once a wing of idle ground-attack flyers has
    /// gathered, throw it at the enemy's harvest line — unless known
    /// anti-air stands over the target. Wings are spent, not managed:
    /// the raid is an attack-move and whatever comes back rejoins the
    /// idle pool.
    pub(super) fn air_raid(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: AirRaidContext<'_>,
        intents: &mut Vec<Intent>,
    ) {
        let wings = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                stats.domain == oxide_sim::stats::Domain::Air
                    && stats.can_target(oxide_sim::stats::Domain::Ground)
                    && u.idle
                    && !context.enlisted.contains(&u.id)
                    && !context.reserved.contains(&u.id)
            })
            .count();
        if wings < dials.air_wing {
            return;
        }
        // The juiciest known target: an enemy harvester, else any
        // enemy building — the raid flies at work, not at armies.
        let target = obs
            .enemy_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .map(|u| (u.tile.manhattan(context.home), u.tile.y, u.tile.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
            .or_else(|| Self::enemy_site(obs, context.home));
        let Some(target) = target else { return };
        // Known operational anti-air over the target scrubs the raid: flak turrets
        // and AA crawlers in sight or memory near the objective.
        let aa_guard = obs
            .enemy_buildings
            .iter()
            .filter(|building| {
                building.kind == BuildingKind::FlakTurret
                    // An unfinished site cannot fire.
                    && (building.built)
            })
            .map(|building| building.anchor)
            .chain(
                obs.enemy_units
                    .iter()
                    .filter(|u| u.kind.stats().can_target(oxide_sim::stats::Domain::Air))
                    .map(|u| u.tile),
            )
            .any(|t| t.chebyshev(target) <= RAID_AA_RADIUS);
        if !aa_guard {
            intents.push(Intent::RaidAir { target });
        }
    }

    pub(super) fn clear_visible_public_starts(
        &mut self,
        obs: &Observation,
        public_map: &PublicMapBriefing,
    ) {
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        for start in public_map.hostile_starting_foundries(obs.me) {
            if self
                .state
                .cleared_hostile_starts
                .binary_search(&start.player)
                .is_ok()
            {
                continue;
            }
            let footprint_visible = (0..foundry_size.1)
                .all(|dy| (0..foundry_size.0).all(|dx| obs.visible(start.anchor.offset(dx, dy))));
            let hostile_building_present = obs.enemy_buildings.iter().any(|building| {
                let size = building.kind.base_stats().size;
                building.seen
                    && start.anchor.x < building.anchor.x + size.0
                    && start.anchor.x + foundry_size.0 > building.anchor.x
                    && start.anchor.y < building.anchor.y + size.1
                    && start.anchor.y + foundry_size.1 > building.anchor.y
            });
            if footprint_visible && !hostile_building_present {
                let Err(index) = self
                    .state
                    .cleared_hostile_starts
                    .binary_search(&start.player)
                else {
                    continue;
                };
                self.state
                    .cleared_hostile_starts
                    .insert(index, start.player);
            }
        }
    }

    #[cfg(test)]
    fn public_prior_ground_connected(
        public_map: &PublicMapBriefing,
        obs: &Observation,
        home: TilePos,
        target: TilePos,
        target_size: (i32, i32),
    ) -> bool {
        let (width, height) = (public_map.map_width(), public_map.map_height());
        let initial_scrap = |tile: TilePos| {
            public_map
                .initial_scrap()
                .binary_search_by_key(&(tile.y, tile.x), |(position, _)| (position.y, position.x))
                .is_ok()
        };
        let ground = |tile: TilePos| {
            let scrap = if obs.explored(tile) {
                obs.known_scrap_at(tile)
            } else {
                initial_scrap(tile)
            };
            !scrap
                && public_map
                    .terrain_at(tile)
                    .is_some_and(|terrain| !terrain.blocks_ground())
        };
        if !ground(home) {
            return false;
        }
        (0..target_size.1)
            .flat_map(|dy| (0..target_size.0).map(move |dx| target.offset(dx, dy)))
            .any(|goal| {
                crate::navigation::search::canonical_path(
                    QueryPurpose::NavigationTest,
                    width,
                    height,
                    home,
                    goal,
                    ground,
                )
                .is_some()
            })
    }

    #[cfg(test)]
    pub(super) fn public_start_ground_connected(
        public_map: &PublicMapBriefing,
        obs: &Observation,
        home: TilePos,
        target: TilePos,
    ) -> bool {
        Self::public_prior_ground_connected(
            public_map,
            obs,
            home,
            target,
            BuildingKind::Foundry.base_stats().size,
        )
    }

    /// Army channel: plans ground missions when the decision supplies them.
    pub(super) fn army(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        mode: PolicyMode<'_>,
        intents: &mut Vec<Intent>,
    ) {
        let Some(inputs) = mode.ground_missions else {
            return;
        };
        self.mission_army(
            obs,
            armies,
            missions::MissionContext {
                mode,
                inputs,
                dials,
                home,
            },
            intents,
        );
    }

    /// An offensive ground order is meaningful only when every surviving
    /// member can reach the objective along explored ground. A ground unit
    /// that walked to its current component leaves an explored corridor; one
    /// ferried onto an island leaves no imaginary road across the intervening
    /// fog. This suppresses island-crossing command storms while still
    /// allowing a landed squad to push locally.
    /// `routes` is the caller's lazily built known-ground projection: the
    /// component labeling depends only on the observation, and one think
    /// asks this question for several armies and again at the push gate.
    fn army_reaches<'a>(
        &self,
        obs: &'a Observation,
        routes: &mut Option<crate::navigation::commands::RouteProjection<'a>>,
        members: &[UnitId],
        target: TilePos,
        public_map: Option<&'a PublicMapBriefing>,
    ) -> bool {
        let mut members: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| members.contains(&unit.id))
            .collect();
        members.sort_unstable_by_key(|unit| unit.id);
        let Some(goals) = self.ground_attack_goals(obs, target, members.len()) else {
            return false;
        };
        let routes = routes.get_or_insert_with(|| {
            public_map.map_or_else(
                || {
                    crate::navigation::commands::RouteProjection::known_ground(
                        QueryPurpose::ArmyMovement,
                        obs,
                    )
                },
                |map| {
                    crate::navigation::commands::RouteProjection::with_public_terrain(
                        QueryPurpose::ArmyMovement,
                        obs,
                        Domain::Ground,
                        map,
                    )
                },
            )
        });
        !members.is_empty()
            && members
                .iter()
                .zip(goals)
                .all(|(unit, goal)| routes.unit_reaches(unit, goal))
    }

    /// The nearest known enemy presence — buildings (ghosts included)
    /// before units — or None while the enemy is entirely unlocated.
    /// The unit fallback skips machines hovering over known rock: a site
    /// is a place ground forces could go, and a flyer parked on a crag
    /// once declared a fully land-connected map "sealed" because the
    /// route flood was asked to reach an unstandable goal.
    pub(super) fn enemy_site(obs: &Observation, home: TilePos) -> Option<TilePos> {
        obs.enemy_buildings
            .iter()
            .map(|b| (b.anchor.manhattan(home), b.anchor.y, b.anchor.x))
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
            .or_else(|| {
                obs.enemy_units
                    .iter()
                    .filter(|u| !obs.known_rock_at(u.tile))
                    .map(|u| (u.tile.manhattan(home), u.tile.y, u.tile.x))
                    .min()
                    .map(|(_, y, x)| TilePos::new(x, y))
            })
    }

    /// Where armies gather: the staging army's rally if one exists, else
    /// a fresh point screening the nearest reachable forward Foundry. Without
    /// that screen, a body which correctly refuses a risky attack can still
    /// leave its expansion undefended until the attacker is already on the
    /// footprint. An island expansion is not a ground rally: if no forward
    /// Foundry has a known route from home, gather near home instead. A
    /// mid-map rally sits on the enemy's march path and gets reinforcements
    /// killed piecemeal.
    fn rally_point(
        &self,
        obs: &Observation,
        staging_army: Option<&Army>,
        enemy_site: Option<TilePos>,
        home: TilePos,
    ) -> TilePos {
        let desired = staging_army.map(|army| army.staging).unwrap_or_else(|| {
            let toward = enemy_site.unwrap_or(TilePos::new(obs.map_width / 2, obs.map_height / 2));
            if let Some(frontline) = enemy_site.and_then(|enemy| {
                let routes = RouteProjection::known_ground(QueryPurpose::ArmyMovement, obs);
                obs.my_buildings
                    .iter()
                    .filter(|building| {
                        building.kind == BuildingKind::Foundry
                            && building.built
                            && building.hp > 0
                            && building.anchor != home
                    })
                    .map(|building| {
                        let anchor = building.anchor;
                        let behind = TilePos::new(
                            anchor.x + (home.x - anchor.x).signum() * 2,
                            anchor.y + (home.y - anchor.y).signum() * 2,
                        );
                        let rally = self.durable_rally_near(obs, behind);
                        (anchor.chebyshev(enemy), anchor.y, anchor.x, rally)
                    })
                    .filter(|(_, _, _, rally)| routes.reaches(home, *rally))
                    .min_by_key(|(distance, y, x, _)| (*distance, *y, *x))
                    .map(|(_, _, _, rally)| rally)
            }) {
                return frontline;
            }
            let lean = |from: i32, to: i32| from + ((to - from) / 3).clamp(-3, 3);
            TilePos::new(lean(home.x, toward.x), lean(home.y, toward.y))
        });
        self.durable_rally_near(obs, desired)
    }
}

fn objective_building_strength(building: &BuildingObs, mode: PolicyMode<'_>, now: u64) -> u64 {
    let contact = (!building.seen)
        .then(|| {
            mode.building_contacts.and_then(|contacts| {
                contacts.iter().find(|contact| {
                    contact.player == building.player
                        && contact.anchor == building.anchor
                        && contact.kind == building.kind
                })
            })
        })
        .flatten();
    let tier = contact.map_or(building.tier, |contact| contact.tier);
    let stats = building.kind.tier_stats(tier);
    let strength = if building.built {
        let damage_per_100: u64 = stats
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.ground)
            .map(crate::executive::weapon_burst_dps100)
            .sum();
        u64::from(building.hp) * damage_per_100
    } else {
        0
    };
    if building.seen {
        return strength;
    }

    // A ghost first encountered between this brain's think ticks has no
    // timestamp. Its age is unknown, not ancient, so retain full strength
    // until the controller has a real sighting from which confidence can age.
    let confidence = contact.map_or(MAX_CONFIDENCE, |contact| {
        contact
            .last_seen
            .map_or(MAX_CONFIDENCE, |_| contact.confidence_at(now))
    });
    strength.saturating_mul(u64::from(confidence)) / u64::from(MAX_CONFIDENCE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executive::ArmyId;
    use crate::observation::BuildingObs;
    use oxide_sim::ids::{BuildingId, PlayerId};
    use oxide_sim::scenario::{BotDifficulty, PlayerSpec, Scenario};
    use oxide_sim::state::Faction;

    #[test]
    fn pressure_counts_a_corridor_gun_outside_the_objective_radius() {
        let (mut obs, armies, policy, _, _) = mission_fixture();
        let goal = TilePos::new(54, 16);
        obs.enemy_buildings.clear();
        obs.my_units
            .iter_mut()
            .for_each(|unit| unit.tile = TilePos::new(8, 16));
        let gun = BuildingObs {
            hp: BuildingKind::Turret.tier_stats(2).max_hp,
            tier: 2,
            ..crate::test_support::building(
                99,
                PlayerId(1),
                BuildingKind::Turret,
                TilePos::new(28, 18),
            )
        };
        obs.enemy_buildings.push(gun.clone());
        let risk = policy
            .approach_defense_strength(&obs, &armies[0], goal, player_mode(None))
            .unwrap();
        assert_eq!(
            risk,
            objective_building_strength(&gun, player_mode(None), obs.tick)
        );
        obs.enemy_buildings[0].anchor = TilePos::new(28, 29);
        assert_eq!(
            policy.approach_defense_strength(&obs, &armies[0], goal, player_mode(None)),
            Some(0)
        );
        obs.enemy_buildings[0].anchor = gun.anchor;
        obs.enemy_buildings[0].kind = BuildingKind::FlakTurret;
        assert_eq!(
            policy.approach_defense_strength(&obs, &armies[0], goal, player_mode(None)),
            Some(0)
        );
    }

    fn fighter(id: u32, tile: TilePos) -> UnitObs {
        crate::test_support::unit(id, PlayerId(0), UnitKind::Lancer, tile)
    }

    fn own(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            player: PlayerId(0),
            kind,
            hp: kind.stats().max_hp,
            ..fighter(id, tile)
        }
    }

    fn own_foundry(id: u32, anchor: TilePos) -> BuildingObs {
        crate::test_support::building(id, PlayerId(0), BuildingKind::Foundry, anchor)
    }

    fn defense(id: u32, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        crate::test_support::building(id, PlayerId(1), kind, anchor)
    }

    fn offensive_position(remote_defense: TilePos) -> (Observation, Army) {
        let units = vec![
            fighter(1, TilePos::new(5, 6)),
            fighter(2, TilePos::new(5, 7)),
            fighter(3, TilePos::new(6, 6)),
            fighter(4, TilePos::new(6, 7)),
        ];
        let army = Army::staging(
            ArmyId(7),
            units.iter().map(|unit| unit.id).collect(),
            TilePos::new(6, 6),
        );
        let obs = Observation::from_data(ObservationData {
            tick: 20_000,
            map_width: 40,
            map_height: 24,
            my_units: units,
            enemy_buildings: vec![
                defense(20, BuildingKind::Turret, TilePos::new(12, 6)),
                defense(21, BuildingKind::Bastion, remote_defense),
            ],
            visible: vec![true; 40 * 24],
            explored: vec![true; 40 * 24],
            ..crate::test_support::observation_data()
        });
        (obs, army)
    }

    fn public_briefing(starts: &[TilePos], separator: Option<(i32, char)>) -> PublicMapBriefing {
        public_briefing_with_frames(starts, &[], separator)
    }

    fn public_briefing_with_frames(
        starts: &[TilePos],
        frames: &[TilePos],
        separator: Option<(i32, char)>,
    ) -> PublicMapBriefing {
        let scenario = public_scenario_with_frames(starts, frames, separator);
        PublicMapBriefing::from_scenario(&scenario).expect("public briefing fixture is valid")
    }

    fn public_scenario_with_frames(
        starts: &[TilePos],
        frames: &[TilePos],
        separator: Option<(i32, char)>,
    ) -> Scenario {
        const WIDTH: usize = 40;
        const HEIGHT: usize = 24;

        assert!(!starts.is_empty() && starts.len() <= 8);
        let mut rows = vec![vec![b'.'; WIDTH]; HEIGHT];
        if let Some((x, authored)) = separator {
            let x = usize::try_from(x).expect("separator is in bounds");
            for row in &mut rows {
                row[x] = authored as u8;
            }
        }
        for frame in frames {
            rows[usize::try_from(frame.y).expect("frame y is in bounds")]
                [usize::try_from(frame.x).expect("frame x is in bounds")] = b'E';
        }
        for (index, anchor) in starts.iter().enumerate() {
            rows[usize::try_from(anchor.y).expect("start y is in bounds")]
                [usize::try_from(anchor.x).expect("start x is in bounds")] =
                b'1' + u8::try_from(index).expect("at most eight starts");
        }
        Scenario {
            mode: Default::default(),
            name: "public recon fixture".into(),
            seed: 0,
            map: rows
                .into_iter()
                .map(|row| String::from_utf8(row).expect("ASCII map"))
                .collect(),
            players: starts
                .iter()
                .enumerate()
                .map(|(index, _)| PlayerSpec {
                    name: format!("player {index}"),
                    faction: if index == 0 {
                        Faction::Ferrous
                    } else {
                        Faction::Cupric
                    },
                    team: None,
                    scrap: 500,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        }
    }

    fn recon_observation(home: TilePos, scout: UnitKind) -> Observation {
        let (mut obs, _) = offensive_position(TilePos::new(30, 18));
        obs.tick = 2_000;
        obs.visible.fill(false);
        obs.explored.fill(false);
        obs.enemy_units.clear();
        obs.enemy_buildings.clear();
        obs.my_units = vec![own(1, scout, home)];
        obs
    }

    fn set_visible(obs: &mut Observation, tile: TilePos, visible: bool) {
        let index =
            usize::try_from(tile.y * obs.map_width + tile.x).expect("fixture tile is in bounds");
        obs.visible[index] = visible;
        obs.explored[index] |= visible;
    }

    fn player_mode(building_contacts: Option<&[BuildingContact]>) -> PolicyMode<'_> {
        PolicyMode {
            evidence: Default::default(),
            ground_missions: None,
            admit_voluntary_macro: true,
            unit_contacts: None,
            building_contacts,
            public_map: None,
        }
    }

    #[derive(Clone)]
    struct MissionRoster {
        unavailable: Vec<UnitId>,
        enlisted: Vec<UnitId>,
        tuning: DifficultyTuning,
        relief: Option<(BuildingId, Vec<UnitId>)>,
    }

    #[derive(Clone)]
    struct MissionFixture {
        inputs: MissionRoster,
        battlefield: crate::battlefield::BattlefieldAssessment,
        experience: crate::experience::Experience,
    }

    impl MissionFixture {
        fn mode(&self) -> PolicyMode<'_> {
            PolicyMode {
                evidence: DecisionEvidence {
                    battlefield: &self.battlefield,
                    experience: &self.experience,
                },
                ground_missions: Some(GroundMissionInputs {
                    unavailable: &self.inputs.unavailable,
                    enlisted: &self.inputs.enlisted,
                    tuning: self.inputs.tuning,
                    relief: self
                        .inputs
                        .relief
                        .as_ref()
                        .map(|(id, members)| (*id, members.as_slice())),
                }),
                ..player_mode(None)
            }
        }
    }

    fn mission_fixture() -> (Observation, Vec<Army>, UtilityPolicy, Dials, MissionFixture) {
        let mut obs = Observation::from_data(ObservationData {
            tick: 1200,
            map_width: 64,
            map_height: 32,
            visible: vec![true; 2048],
            explored: vec![true; 2048],
            ..crate::test_support::observation_data()
        });
        obs.my_buildings = vec![
            own_foundry(1, TilePos::new(3, 3)),
            own_foundry(2, TilePos::new(28, 3)),
        ];
        let mut armies = Vec::new();
        for (id, tile) in [
            (0, TilePos::new(8, 8)),
            (1, TilePos::new(31, 8)),
            (2, TilePos::new(12, 22)),
        ] {
            let members: Vec<_> = (0..4)
                .map(|offset| {
                    let unit = fighter(
                        id * 4 + offset,
                        tile.offset(offset as i32 % 2, offset as i32 / 2),
                    );
                    let member = unit.id;
                    obs.my_units.push(unit);
                    member
                })
                .collect();
            armies.push(Army::staging(ArmyId(id), members, tile));
        }
        for (id, tile) in [(100, TilePos::new(10, 3)), (101, TilePos::new(35, 3))] {
            let mut enemy = fighter(id, tile);
            enemy.player = PlayerId(1);
            obs.enemy_units.push(enemy);
        }
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut battlefield = crate::battlefield::Battlefield::default();
        battlefield.observe(&obs, &armies, tuning, None);
        let mission = MissionFixture {
            battlefield: battlefield.assessment().clone(),
            inputs: MissionRoster {
                unavailable: Vec::new(),
                enlisted: armies
                    .iter()
                    .flat_map(|army| army.members.iter().copied())
                    .collect(),
                tuning,
                relief: None,
            },
            experience: Default::default(),
        };
        let dials = Dials {
            army_size: 3,
            minimum_core_equivalents: 2,
            ..Dials::default()
        };

        (obs, armies, UtilityPolicy::new(), dials, mission)
    }

    #[test]
    fn consecutive_army_calls_use_only_the_supplied_mission_ownership() {
        let (obs, armies, mut policy, dials, mut mission) = mission_fixture();
        let home = TilePos::new(3, 3);
        let mut assigned = Vec::new();
        policy.army(&dials, &obs, &armies, home, mission.mode(), &mut assigned);
        assert!(!assigned.is_empty());

        mission.inputs.unavailable = obs.my_units.iter().map(|unit| unit.id).collect();
        let mut unavailable = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            home,
            mission.mode(),
            &mut unavailable,
        );
        assert!(
            unavailable.is_empty(),
            "new ownership must replace the previous roster"
        );
    }

    #[test]
    fn mission_draft_skips_recovering_veterans_before_exact_lowering() {
        let (mut obs, _, mut policy, dials, mut mission) = mission_fixture();
        obs.enemy_units.clear();
        let home = TilePos::new(3, 3);
        let mut executive = crate::executive::Executive::new();
        executive.apply_with_reservations(
            obs.me,
            &obs,
            &[Intent::FormArmy {
                staging: TilePos::new(8, 8),
                size: 4,
            }],
            &[],
        );
        let veterans = executive.armies()[0].members.clone();
        for unit in &mut obs.my_units {
            if veterans.contains(&unit.id) {
                unit.hp = unit.kind.stats().max_hp / 4;
            }
        }
        executive.maintain_player_facing(obs.me, &obs, home);
        assert!(executive.armies().is_empty());
        obs.tick += 1_200;
        executive.maintain_player_facing(obs.me, &obs, home);
        assert!(executive.enlisted().next().is_none());
        let unavailable: Vec<_> = executive.muster_exclusions().collect();
        assert_eq!(unavailable, veterans);
        mission.battlefield = Default::default();
        let inputs = &mut mission.inputs;
        inputs.enlisted.clear();
        inputs.unavailable = unavailable;
        let mut intents = Vec::new();
        policy.army(&dials, &obs, &[], home, mission.mode(), &mut intents);
        let members = intents
            .iter()
            .find_map(|intent| match intent {
                Intent::FormArmyWith { members, .. } => Some(members.clone()),
                _ => None,
            })
            .expect("healthy reserve draft");
        assert!(members.iter().all(|id| !veterans.contains(id)));
        executive.apply_with_reservations(obs.me, &obs, &intents, &[]);
        assert_eq!(executive.armies()[0].members, members);
        assert!(
            veterans
                .iter()
                .all(|id| !executive.enlisted().any(|member| member == *id))
        );
    }

    #[test]
    fn missions_answer_two_fronts_without_redirecting_the_third_body() {
        use crate::executive::ArmyPurpose;
        let (obs, armies, mut policy, dials, mission) = mission_fixture();
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            mission.mode(),
            &mut intents,
        );
        let defense: Vec<_> = intents
            .iter()
            .filter_map(|intent| {
                if let Intent::AssignArmyMission {
                    army,
                    members,
                    mission,
                } = intent
                {
                    Some((*army, members.clone(), mission.purpose))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(defense.len(), 2, "{intents:?}");
        assert_eq!(
            defense[0],
            (
                ArmyId(0),
                armies[0].members.clone(),
                ArmyPurpose::Defend(BuildingId(1))
            )
        );
        assert_eq!(
            defense[1],
            (
                ArmyId(1),
                armies[1].members.clone(),
                ArmyPurpose::Defend(BuildingId(2))
            )
        );
        assert!(
            defense
                .iter()
                .all(|(_, members, _)| members.iter().all(|id| !armies[2].members.contains(id)))
        );
    }

    #[test]
    fn an_accepted_defender_is_credited_before_a_nearer_unassigned_body() {
        use crate::executive::{ArmyMission, ArmyPurpose};
        let (obs, mut armies, mut policy, dials, mission) = mission_fixture();
        armies[2].mission = Some(ArmyMission {
            purpose: ArmyPurpose::Defend(BuildingId(1)),
            goal: TilePos::new(10, 3),
            accepted_at: obs.tick - 24,
            deadline: obs.tick + 1700,
            score: 1024,
        });
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            mission.mode(),
            &mut intents,
        );
        assert!(
            !intents.iter().any(|intent| matches!(intent,
                Intent::AssignArmyMission { mission, .. } | Intent::FormArmyWith { mission, .. }
                    if mission.purpose == ArmyPurpose::Defend(BuildingId(1))
            )),
            "existing arriving coverage must not recruit a second body: {intents:?}"
        );
        assert!(intents.iter().any(|intent| matches!(intent,
            Intent::AssignArmyMission { army: ArmyId(1), mission, .. }
                if mission.purpose == ArmyPurpose::Defend(BuildingId(2))
        )));
    }

    #[test]
    fn an_unreachable_front_does_not_hide_a_reachable_lower_ranked_response() {
        use crate::executive::ArmyPurpose;
        let (mut obs, mut armies, mut policy, dials, mut mission) = mission_fixture();
        let removed = armies.remove(1);
        obs.my_units
            .retain(|unit| !removed.members.contains(&unit.id));
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(20, y)).collect();
        let mut assessment = mission.battlefield.clone();
        assessment.pressure.reverse();
        mission.battlefield = assessment;
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            mission.mode(),
            &mut intents,
        );
        let responses: Vec<_> = intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::AssignArmyMission { mission, .. }
                | Intent::FormArmyWith { mission, .. } => Some(mission.purpose),
                _ => None,
            })
            .collect();
        assert!(
            responses.contains(&ArmyPurpose::Defend(BuildingId(1))),
            "{intents:?}"
        );
        assert!(
            !responses.contains(&ArmyPurpose::Defend(BuildingId(2))),
            "{intents:?}"
        );
    }

    #[test]
    fn mission_ground_response_ignores_air_only_pressure_and_respects_reaction() {
        let (obs, armies, mut policy, dials, mut mission) = mission_fixture();
        let mut assessment = mission.battlefield.clone();
        assessment.pressure[0].air = assessment.pressure[0].ground;
        assessment.pressure[0].ground = 0;
        assessment.pressure[1].evidence_at = obs.tick;
        mission.battlefield = assessment;
        mission.inputs.tuning.reaction_delay = 40;
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            mission.mode(),
            &mut intents,
        );
        assert!(
            intents
                .iter()
                .all(|intent| !matches!(intent, Intent::AssignArmyMission { .. })),
            "{intents:?}"
        );
    }

    #[test]
    fn retained_defense_recalls_after_lost_contact_without_fresh_attention() {
        use crate::executive::{ArmyMission, ArmyPurpose};
        let (mut obs, mut armies, mut policy, dials, mut mission) = mission_fixture();
        obs.enemy_units.clear();
        let inputs = &mut mission.inputs;
        armies[0].mission = Some(ArmyMission {
            purpose: ArmyPurpose::Defend(BuildingId(1)),
            goal: TilePos::new(3, 3),
            accepted_at: 1190,
            deadline: 1800,
            score: 1000,
        });
        inputs.tuning.attention_slots = 0;
        mission.battlefield = Default::default();
        armies[0].state = ArmyState::Engaging;
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            PolicyMode {
                admit_voluntary_macro: false,
                ..mission.mode()
            },
            &mut intents,
        );
        assert!(
            matches!(
                intents.as_slice(),
                [Intent::AssignArmyMission {
                    army: ArmyId(0),
                    mission: ArmyMission {
                        purpose: ArmyPurpose::Recover,
                        ..
                    },
                    ..
                }]
            ),
            "{intents:?}"
        );
    }

    #[test]
    fn returned_mission_reopens_reserve_without_fresh_attention() {
        use crate::executive::{ArmyMission, ArmyPurpose};
        let (mut obs, mut armies, mut policy, dials, mut mission) = mission_fixture();
        obs.enemy_units.clear();
        mission.battlefield = Default::default();
        let inputs = &mut mission.inputs;
        inputs.tuning.attention_slots = 0;
        armies[0].mission = Some(ArmyMission {
            purpose: ArmyPurpose::Recover,
            goal: armies[0].staging,
            accepted_at: 1000,
            deadline: 1800,
            score: 0,
        });
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            PolicyMode {
                admit_voluntary_macro: false,
                ..mission.mode()
            },
            &mut intents,
        );
        assert!(
            matches!(
                intents.as_slice(),
                [Intent::AssignArmyMission {
                    army: ArmyId(0),
                    mission: ArmyMission {
                        purpose: ArmyPurpose::Reserve,
                        ..
                    },
                    ..
                }]
            ),
            "{intents:?}"
        );
    }

    #[test]
    fn pressure_survives_reobserving_its_remembered_building() {
        use crate::executive::{ArmyMission, ArmyPurpose};
        let (mut obs, mut armies, mut policy, dials, mut mission) = mission_fixture();
        obs.enemy_units.clear();
        let objective = defense(200, BuildingKind::Foundry, TilePos::new(40, 6));
        obs.enemy_buildings = vec![objective.clone()];
        mission.battlefield = Default::default();
        armies[0].mission = Some(ArmyMission {
            purpose: ArmyPurpose::Pressure(crate::executive::ArmyObjective::from_building(
                &BuildingObs {
                    provisional: false,
                    id: BuildingId(u32::MAX),
                    seen: false,
                    ..objective.clone()
                },
            )),
            goal: objective.anchor,
            accepted_at: obs.tick - 240,
            deadline: obs.tick + 1560,
            score: 1000,
        });
        let mut intents = Vec::new();
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            PolicyMode {
                admit_voluntary_macro: false,
                ..mission.mode()
            },
            &mut intents,
        );
        assert!(
            intents.is_empty(),
            "reobserving the objective must retain its mission: {intents:?}"
        );

        let committed = armies[0].mission.clone();
        obs.enemy_buildings[0].id = BuildingId(u32::MAX);
        obs.enemy_buildings[0].seen = false;
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            PolicyMode {
                admit_voluntary_macro: false,
                ..mission.mode()
            },
            &mut intents,
        );
        assert!(intents.is_empty(), "the same remembered site remains valid");
        assert_eq!(armies[0].mission, committed);

        obs.enemy_buildings[0].anchor.x += 4;
        policy.army(
            &dials,
            &obs,
            &armies,
            TilePos::new(3, 3),
            PolicyMode {
                admit_voluntary_macro: false,
                ..mission.mode()
            },
            &mut intents,
        );
        assert!(
            matches!(
                intents.as_slice(),
                [Intent::AssignArmyMission {
                    mission: ArmyMission {
                        purpose: ArmyPurpose::Recover,
                        ..
                    },
                    ..
                }]
            ),
            "another ghost with the same placeholder id cannot retain this objective: {intents:?}"
        );
    }

    #[test]
    fn valid_pressure_reassignment_requires_hold_and_material_improvement() {
        use crate::executive::{ArmyMission, ArmyPurpose};
        let (mut obs, mut armies, mut policy, dials, mut mission) = mission_fixture();
        obs.enemy_units.clear();
        obs.enemy_buildings = vec![
            defense(200, BuildingKind::Foundry, TilePos::new(40, 6)),
            defense(201, BuildingKind::Foundry, TilePos::new(48, 6)),
        ];
        armies[0].members = obs.my_units.iter().map(|unit| unit.id).collect();
        armies.truncate(1);
        mission.battlefield = Default::default();
        let choose = |policy: &mut UtilityPolicy, armies: &[Army]| {
            let mut intents = Vec::new();
            policy.army(
                &dials,
                &obs,
                armies,
                TilePos::new(3, 3),
                mission.mode(),
                &mut intents,
            );
            intents.into_iter().find_map(|intent| match intent {
                Intent::AssignArmyMission { mission, .. }
                    if matches!(mission.purpose, ArmyPurpose::Pressure(target) if target.id == Some(BuildingId(200))) =>
                {
                    Some(mission)
                }
                _ => None,
            })
        };
        let score = choose(&mut policy, &armies)
            .expect("a useful fresh objective")
            .score;
        for (age, prior_score, redirect) in [
            (299, 0, false),
            (300, score, false),
            (300, score * 4 / 5, true),
        ] {
            let mut trial = policy.clone();
            let mut trial_armies = armies.clone();
            trial_armies[0].mission = Some(ArmyMission {
                purpose: ArmyPurpose::Pressure(crate::executive::ArmyObjective::from_building(
                    &obs.enemy_buildings[1],
                )),
                goal: TilePos::new(48, 6),
                accepted_at: obs.tick - age,
                deadline: obs.tick + 1000,
                score: prior_score,
            });
            assert_eq!(
                choose(&mut trial, &trial_armies).is_some(),
                redirect,
                "age={age} score={prior_score}"
            );
        }
    }

    #[test]
    fn corroborated_assault_experience_changes_the_next_unpaid_objective_and_decays() {
        use crate::executive::ArmyPurpose;
        use crate::experience::{
            Doctrine, EpisodeId, EpisodeOwner, EpisodeReport, Experience, ExperienceKey,
            ExperienceSubject, Outcome, OutcomeReason,
        };
        let (mut obs, mut armies, mut policy, dials, mut mission) = mission_fixture();
        obs.enemy_units.clear();
        obs.enemy_buildings = vec![
            defense(200, BuildingKind::Foundry, TilePos::new(40, 6)),
            defense(201, BuildingKind::Foundry, TilePos::new(42, 6)),
        ];
        armies[0].members = obs.my_units.iter().map(|unit| unit.id).collect();
        armies.truncate(1);
        mission.battlefield = Default::default();
        let choice = |policy: &mut UtilityPolicy, obs: &Observation, mission: &MissionFixture| {
            let mut intents = Vec::new();
            policy.army(
                &dials,
                obs,
                &armies,
                TilePos::new(3, 3),
                mission.mode(),
                &mut intents,
            );
            intents.into_iter().find_map(|intent| {
                if let Intent::AssignArmyMission { mission, .. } = intent {
                    Some(mission.purpose)
                } else {
                    None
                }
            })
        };
        assert_eq!(
            choice(&mut policy, &obs, &mission),
            Some(ArmyPurpose::Pressure(
                crate::executive::ArmyObjective::from_building(&obs.enemy_buildings[0])
            ))
        );
        let mut experience = Experience::default();
        experience.observe(&obs, 6000);
        for serial in [1, 2] {
            let id = EpisodeId {
                owner: EpisodeOwner::Ground,
                serial,
            };
            experience.report(EpisodeReport {
                id,
                credit: id,
                context: ExperienceKey {
                    doctrine: Doctrine::Siege,
                    x: 40,
                    y: 6,
                    subject: ExperienceSubject::Building(Some(oxide_sim::BuildingId(200))),
                },
                objective: None,
                started_at: 0,
                finished_at: obs.tick,
                participants: vec![UnitId(100 + serial as u32)],
                phase: 2,
                outcome: Outcome::Ineffective,
                reason: OutcomeReason::ObservedCounter,
                observed_progress: 0,
                own_lost_value: 100,
                confidence: 1000,
                doctrine_eligible: true,
            });
        }
        mission.experience = experience.clone();
        assert_eq!(
            choice(&mut policy, &obs, &mission),
            Some(ArmyPurpose::Pressure(
                crate::executive::ArmyObjective::from_building(&obs.enemy_buildings[1])
            ))
        );
        obs.tick += 6000;
        experience.observe(&obs, 6000);
        mission.experience = experience;
        assert_eq!(
            choice(&mut policy, &obs, &mission),
            Some(ArmyPurpose::Pressure(
                crate::executive::ArmyObjective::from_building(&obs.enemy_buildings[0])
            ))
        );
    }

    #[test]
    fn remembered_ghost_does_not_block_current_negative_evidence_at_a_public_start() {
        let home = TilePos::new(4, 12);
        let hostile = TilePos::new(28, 12);
        let public_map = public_briefing(&[home, hostile], None);
        let mut obs = recon_observation(home, UnitKind::Harvester);
        for dy in 0..2 {
            for dx in 0..2 {
                set_visible(&mut obs, hostile.offset(dx, dy), true);
            }
        }
        obs.enemy_buildings = vec![BuildingObs {
            seen: false,
            ..defense(20, BuildingKind::Foundry, hostile)
        }];
        let mut policy = UtilityPolicy::new();

        policy.clear_visible_public_starts(&obs, &public_map);

        assert!(
            policy
                .uncleared_hostile_starts(&public_map, obs.me)
                .is_empty(),
            "a remembered ghost is not current evidence against the fully visible empty footprint"
        );
        assert!(
            !obs.enemy_buildings[0].seen,
            "the policy must retire only the authored prior, not rewrite dynamic memory"
        );
    }

    #[test]
    fn visible_absence_clears_a_public_start_without_admitting_a_scout() {
        let home = TilePos::new(4, 12);
        let hostile = TilePos::new(28, 12);
        let public_map = public_briefing(&[home, hostile], None);
        let mut obs = recon_observation(home, UnitKind::Harvester);
        obs.my_buildings = vec![own_foundry(0, home)];
        obs.my_queues = vec![Vec::new()];
        for dy in 0..2 {
            for dx in 0..2 {
                set_visible(&mut obs, hostile.offset(dx, dy), true);
            }
        }
        let dials = Dials::default();

        let mut policy = UtilityPolicy::new();

        let _ = policy.think_residual(&dials, &obs, &[], &[], &[], &public_map);

        assert!(
            policy
                .uncleared_hostile_starts(&public_map, obs.me)
                .is_empty(),
            "negative evidence must not depend on the scouting channel or worker saturation"
        );
    }

    #[test]
    fn scout_fallbacks_are_an_explicit_role_allowlist() {
        for kind in UnitKind::ALL {
            let unit = own(1, kind, TilePos::new(4, 4));
            let ordinary_allowed = matches!(
                kind,
                UnitKind::Kestrel
                    | UnitKind::Gnat
                    | UnitKind::Harvester
                    | UnitKind::Scuttler
                    | UnitKind::Sentinel
            );
            let contested_allowed = matches!(
                kind,
                UnitKind::Kestrel | UnitKind::Gnat | UnitKind::Scuttler | UnitKind::Sentinel
            );

            assert_eq!(
                utility_scout_preference(&unit, false).is_some(),
                ordinary_allowed,
                "ordinary reconnaissance eligibility drifted for {kind:?}"
            );
            assert_eq!(
                utility_scout_preference(&unit, true).is_some(),
                contested_allowed,
                "contested reconnaissance eligibility drifted for {kind:?}"
            );
        }
    }

    #[test]
    fn player_facing_rallies_do_not_park_on_known_extractor_frames() {
        let frame = TilePos::new(6, 6);
        let (mut obs, mut army) = offensive_position(TilePos::new(30, 18));
        obs.known_frames = vec![frame];
        army.staging = frame;
        let policy = UtilityPolicy::new();

        let player_rally = policy.rally_point(&obs, Some(&army), None, frame);
        assert!(
            player_rally.x < frame.x
                || player_rally.x >= frame.x + 2
                || player_rally.y < frame.y
                || player_rally.y >= frame.y + 2,
            "a durable army rally must not be consumed by later restoration: {player_rally:?}"
        );
    }

    #[test]
    fn fresh_armies_screen_a_reachable_forward_foundry_but_not_an_island_expansion() {
        let home = TilePos::new(4, 4);
        let expansion = TilePos::new(18, 10);
        let enemy = TilePos::new(34, 18);
        let (mut connected, _) = offensive_position(enemy);
        connected.my_buildings = vec![own_foundry(0, home), own_foundry(1, expansion)];
        let policy = UtilityPolicy::new();

        assert_eq!(
            policy.rally_point(&connected, None, Some(enemy), home),
            TilePos::new(16, 8),
            "a new army should assemble on the homeward side of the forward base"
        );

        let mut island = connected;
        island.known_rock = (0..island.map_height)
            .map(|y| TilePos::new(12, y))
            .collect();
        assert_eq!(
            policy.rally_point(&island, None, Some(enemy), home),
            TilePos::new(7, 7),
            "a ground army must not be assigned to screen an unreachable island Foundry"
        );
    }

    #[test]
    fn public_start_clears_only_after_complete_current_negative_evidence() {
        let home = TilePos::new(4, 12);
        let hostile = TilePos::new(28, 12);
        let public_map = public_briefing(&[home, hostile], None);
        let mut obs = recon_observation(home, UnitKind::Harvester);
        let mut policy = UtilityPolicy::new();

        for tile in [hostile, hostile.offset(1, 0), hostile.offset(0, 1)] {
            set_visible(&mut obs, tile, true);
        }
        obs.explored.fill(true);
        policy.clear_visible_public_starts(&obs, &public_map);
        assert_eq!(
            policy.uncleared_hostile_starts(&public_map, obs.me).len(),
            1
        );
        obs.my_units[0].tile = TilePos::new(23, 12);

        set_visible(&mut obs, hostile.offset(1, 1), true);
        obs.enemy_buildings = vec![BuildingObs {
            seen: true,
            ..defense(20, BuildingKind::Turret, hostile.offset(1, 1))
        }];
        policy.clear_visible_public_starts(&obs, &public_map);
        assert_eq!(
            policy.uncleared_hostile_starts(&public_map, obs.me).len(),
            1,
            "a currently visible hostile footprint prevents negative-evidence clearing"
        );

        obs.enemy_buildings.clear();
        policy.clear_visible_public_starts(&obs, &public_map);
        assert!(
            policy
                .uncleared_hostile_starts(&public_map, obs.me)
                .is_empty()
        );
        assert_eq!(policy.state.cleared_hostile_starts, [PlayerId(1)]);
    }

    #[test]
    fn cleared_public_start_stays_retired_after_the_footprint_goes_dark() {
        let home = TilePos::new(4, 12);
        let hostile = TilePos::new(28, 12);
        let public_map = public_briefing(&[home, hostile], None);
        let mut obs = recon_observation(home, UnitKind::Harvester);
        for dy in 0..2 {
            for dx in 0..2 {
                set_visible(&mut obs, hostile.offset(dx, dy), true);
            }
        }
        let mut policy = UtilityPolicy::new();

        policy.clear_visible_public_starts(&obs, &public_map);
        assert_eq!(policy.state.cleared_hostile_starts, [PlayerId(1)]);

        obs.visible.fill(false);
        policy.clear_visible_public_starts(&obs, &public_map);
        assert_eq!(policy.state.cleared_hostile_starts, [PlayerId(1)]);
        assert!(
            policy
                .uncleared_hostile_starts(&public_map, obs.me)
                .is_empty(),
            "later darkness must not resurrect an authored prior disproved by current sight"
        );
    }

    #[test]
    fn clearing_one_public_start_leaves_each_other_hostile_start_actionable() {
        let home = TilePos::new(4, 12);
        let cleared = TilePos::new(28, 6);
        let remaining = TilePos::new(28, 18);
        let public_map = public_briefing(&[home, cleared, remaining], None);
        let mut obs = recon_observation(home, UnitKind::Harvester);
        for dy in 0..2 {
            for dx in 0..2 {
                set_visible(&mut obs, cleared.offset(dx, dy), true);
            }
        }
        let mut policy = UtilityPolicy::new();

        policy.clear_visible_public_starts(&obs, &public_map);

        assert_eq!(policy.state.cleared_hostile_starts, [PlayerId(1)]);
        assert_eq!(
            policy.uncleared_hostile_starts(&public_map, obs.me),
            [StartingFoundry {
                player: PlayerId(2),
                anchor: remaining,
            }]
        );
    }
}
