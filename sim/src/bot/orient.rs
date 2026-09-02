//! Seat orientation: the fairness transform.
//!
//! Every deterministic tie-break in a policy — ring-scan order, `(y, x)`
//! sort keys, "lean toward the enemy" arithmetic — has a compass
//! direction baked in, and on a 180°-symmetric map the two seats
//! experience those directions differently: the northwest seat's
//! placement scan probes its own rear while the southeast seat's probes
//! its front line. Chasing each skew individually is endless; instead
//! the brain *orients* its world. A policy whose home sits in the
//! flipped half sees a flipped observation, thinks exactly the logic
//! its opponent thinks, and its intents are flipped back on the way
//! out. This keeps compass-flavored tie-breaks from systematically
//! favoring one seat.
//!
//! The flip is per-axis (x when home is in the east half, y when in the
//! south half), which also orients the corner seats of future 4-player
//! maps.

use super::PublicMapBriefing;
use super::executive::Intent;
use super::observation::Observation;
use crate::stats::BuildingKind;
use chassis::grid::TilePos;

/// Which axes a brain flips to think in home-in-the-northwest space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orientation {
    flip_x: bool,
    flip_y: bool,
    width: i32,
    height: i32,
}

impl Orientation {
    /// Orientation for a brain whose home footprint anchors at `home`
    /// on a `width` × `height` map: flip whichever axes put home in the
    /// southeast, so the policy always reasons from the northwest.
    pub fn for_home(obs: &Observation, home: TilePos) -> Self {
        Self {
            flip_x: 2 * home.x >= obs.map_width,
            flip_y: 2 * home.y >= obs.map_height,
            width: obs.map_width,
            height: obs.map_height,
        }
    }

    /// True when this orientation changes anything at all.
    pub fn is_identity(&self) -> bool {
        !self.flip_x && !self.flip_y
    }

    /// Maps a tile into (or back out of — the flip is an involution)
    /// oriented space.
    pub fn tile(&self, t: TilePos) -> TilePos {
        TilePos::new(
            if self.flip_x {
                self.width - 1 - t.x
            } else {
                t.x
            },
            if self.flip_y {
                self.height - 1 - t.y
            } else {
                t.y
            },
        )
    }

    /// Maps a footprint anchor (top-left) of a `size` building: flipping
    /// a span moves its anchor to what was its far corner.
    pub fn anchor(&self, a: TilePos, size: (i32, i32)) -> TilePos {
        TilePos::new(
            if self.flip_x {
                self.width - size.0 - a.x
            } else {
                a.x
            },
            if self.flip_y {
                self.height - size.1 - a.y
            } else {
                a.y
            },
        )
    }

    /// A copy of the observation with every position oriented. Sorted
    /// fields are re-sorted so iteration order is oriented too — that
    /// is the point.
    pub fn observe(&self, obs: &Observation) -> Observation {
        if self.is_identity() {
            return obs.clone();
        }
        let mut o = obs.clone();
        o.visible = (0..self.height)
            .flat_map(|y| {
                (0..self.width).map(move |x| {
                    let source = self.tile(TilePos::new(x, y));
                    obs.visible(source)
                })
            })
            .collect();
        o.explored = (0..self.height)
            .flat_map(|y| {
                (0..self.width).map(move |x| {
                    let source = self.tile(TilePos::new(x, y));
                    obs.explored(source)
                })
            })
            .collect();
        for u in o
            .my_units
            .iter_mut()
            .chain(o.ally_units.iter_mut())
            .chain(o.enemy_units.iter_mut())
        {
            u.tile = self.tile(u.tile);
            if let Some(node) = u.harvesting.as_mut() {
                *node = self.tile(*node);
            }
            // A pending found's promise is a footprint, so its anchor
            // flips like a building's — the site audit compares it
            // against anchors recorded in oriented space.
            if let Some((kind, anchor)) = u.founding.as_mut() {
                *anchor = self.anchor(*anchor, kind.base_stats().size);
            }
        }
        for b in o
            .my_buildings
            .iter_mut()
            .chain(o.ally_buildings.iter_mut())
            .chain(o.enemy_buildings.iter_mut())
        {
            b.anchor = self.anchor(b.anchor, {
                let (w, h) = b.kind.base_stats().size;
                (w, h)
            });
        }
        for (pos, _) in o.known_scrap.iter_mut().chain(o.known_wrecks.iter_mut()) {
            *pos = self.tile(*pos);
        }
        // Frames are 2x2 footprints, so their anchors flip like a
        // building's, not like a tile — the same rule founding promises
        // use above. This field was the one positional collection
        // observe() forgot: flipped seats mixed world-space frame
        // anchors with oriented everything else, aimed Extractor
        // claims at mirror-image tiles holding no frame, and fed the
        // policy a seat-dependent nearest-frame feature.
        for f in o.known_frames.iter_mut() {
            *f = self.anchor(*f, (2, 2));
        }
        for pos in o
            .known_rock
            .iter_mut()
            .chain(o.known_peaks.iter_mut())
            .chain(o.blips.iter_mut())
            .chain(o.salvage_incidents.iter_mut())
            .chain(o.incoming_shells.iter_mut())
        {
            *pos = self.tile(*pos);
        }
        o.known_scrap.sort_by_key(|(p, _)| (p.y, p.x));
        o.known_frames.sort_by_key(|p| (p.y, p.x));
        o.known_wrecks.sort_by_key(|(p, _)| (p.y, p.x));
        o.known_rock.sort_by_key(|p| (p.y, p.x));
        o.known_peaks.sort_by_key(|p| (p.y, p.x));
        o.blips.sort_by_key(|p| (p.y, p.x));
        o.salvage_incidents.sort_by_key(|p| (p.y, p.x));
        o.incoming_shells.sort_by_key(|p| (p.y, p.x));
        o.enemy_buildings
            .sort_by_key(|b| (b.anchor.y, b.anchor.x, b.player));
        o
    }

    /// Orients immutable authored map facts into the same frame as a policy's
    /// dynamic observation.
    pub fn briefing(&self, briefing: &PublicMapBriefing) -> PublicMapBriefing {
        debug_assert_eq!(self.width, briefing.map_width);
        debug_assert_eq!(self.height, briefing.map_height);
        if self.is_identity() {
            return briefing.clone();
        }
        let mut oriented = briefing.clone();
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        for start in &mut oriented.starting_foundries {
            start.anchor = self.anchor(start.anchor, foundry_size);
        }
        for (position, _) in &mut oriented.non_ground_terrain {
            *position = self.tile(*position);
        }
        for frame in &mut oriented.extractor_frames {
            *frame = self.anchor(*frame, BuildingKind::Extractor.base_stats().size);
        }
        for (position, _) in &mut oriented.initial_scrap {
            *position = self.tile(*position);
        }
        oriented
            .non_ground_terrain
            .sort_by_key(|(position, _)| (position.y, position.x));
        oriented
            .extractor_frames
            .sort_by_key(|position| (position.y, position.x));
        oriented
            .initial_scrap
            .sort_by_key(|(position, _)| (position.y, position.x));
        oriented
    }

    /// An army as the oriented policy should see it.
    pub fn army(&self, mut a: super::executive::Army) -> super::executive::Army {
        a.staging = self.tile(a.staging);
        a.target = a.target.map(|t| self.tile(t));
        a
    }

    /// Maps a think's intents back into world space.
    pub fn emit(&self, intents: Vec<Intent>) -> Vec<Intent> {
        if self.is_identity() {
            return intents;
        }
        intents
            .into_iter()
            .map(|i| match i {
                Intent::Build { kind, anchor } => Intent::Build {
                    kind,
                    anchor: self.anchor(anchor, {
                        let (w, h) = kind.base_stats().size;
                        (w, h)
                    }),
                },
                Intent::BuildWith {
                    builder,
                    kind,
                    anchor,
                } => Intent::BuildWith {
                    builder,
                    kind,
                    anchor: self.anchor(anchor, {
                        let (w, h) = kind.base_stats().size;
                        (w, h)
                    }),
                },
                Intent::FormArmy { staging, size } => Intent::FormArmy {
                    staging: self.tile(staging),
                    size,
                },
                Intent::PushArmy { army, target } => Intent::PushArmy {
                    army,
                    target: self.tile(target),
                },
                Intent::MoveUnits { units, goal } => Intent::MoveUnits {
                    units,
                    goal: self.tile(goal),
                },
                Intent::AttackMoveUnits { units, goal } => Intent::AttackMoveUnits {
                    units,
                    goal: self.tile(goal),
                },
                Intent::AssignHarvest { unit, node } => Intent::AssignHarvest {
                    unit,
                    node: self.tile(node),
                },
                Intent::Scout { unit, to } => Intent::Scout {
                    unit,
                    to: self.tile(to),
                },
                Intent::RaidAir { target } => Intent::RaidAir {
                    target: self.tile(target),
                },
                Intent::Unload { transport, at } => Intent::Unload {
                    transport,
                    at: self.tile(at),
                },
                // Positionless intents pass through — and the match stays
                // exhaustive on purpose: a new positioned intent that
                // slips through unflipped is a silent seat-bias
                // regression, so adding a variant must break this match.
                keep @ (Intent::TrainAt { .. }
                | Intent::CancelSite { .. }
                | Intent::Repair { .. }
                | Intent::Salvage { .. }
                | Intent::Upgrade { .. }
                | Intent::Load { .. }
                | Intent::AttackUnits { .. }
                | Intent::RepairUnits { .. }
                | Intent::StopUnits { .. }) => keep,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scenario;
    use crate::bot::executive::{Army, ArmyId, ArmyState};
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION, UnitObs};
    use crate::ids::{BuildingId, PlayerId, Target, UnitId};
    use crate::state::Faction;
    use crate::stats::{BuildingKind, UnitKind};

    fn unit(
        id: u32,
        player: u8,
        kind: UnitKind,
        tile: TilePos,
        founding: Option<(BuildingKind, TilePos)>,
    ) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(player),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding,
            repairing: false,
            grounded: false,
        }
    }

    fn building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(player),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn observation() -> Observation {
        let (width, height) = (8, 6);
        let mut visible = vec![false; (width * height) as usize];
        visible[(2 * width + 1) as usize] = true;
        let mut explored = vec![false; (width * height) as usize];
        explored[(3 * width + 2) as usize] = true;
        Observation {
            version: OBSERVATION_VERSION,
            tick: 42,
            me: PlayerId(0),
            scrap: 300,
            map_width: width,
            map_height: height,
            my_units: vec![unit(
                1,
                0,
                UnitKind::Harvester,
                TilePos::new(1, 1),
                Some((BuildingKind::Foundry, TilePos::new(2, 2))),
            )],
            my_buildings: vec![building(1, 0, BuildingKind::Foundry, TilePos::new(1, 1))],
            my_queues: vec![vec![UnitKind::Harvester]],
            my_queued_units: vec![UnitId(1)],
            ally_units: vec![unit(2, 1, UnitKind::Wisp, TilePos::new(2, 3), None)],
            ally_buildings: vec![building(2, 1, BuildingKind::Array, TilePos::new(2, 1))],
            enemy_units: vec![unit(3, 2, UnitKind::Darter, TilePos::new(3, 4), None)],
            enemy_buildings: vec![building(3, 2, BuildingKind::Bastion, TilePos::new(4, 3))],
            visible,
            explored,
            known_scrap: vec![(TilePos::new(1, 0), 50), (TilePos::new(5, 0), 70)],
            known_rock: vec![TilePos::new(1, 1), TilePos::new(5, 1)],
            known_frames: vec![TilePos::new(1, 2), TilePos::new(5, 2)],
            known_peaks: vec![TilePos::new(1, 3), TilePos::new(5, 3)],
            known_wrecks: vec![(TilePos::new(2, 4), 30)],
            salvage_incidents: vec![TilePos::new(3, 4)],
            blips: vec![TilePos::new(4, 4)],
            faction: Faction::Ferrous,
            my_shells: 2,
            incoming_shells: vec![TilePos::new(1, 5), TilePos::new(5, 5)],
        }
    }

    #[test]
    fn public_map_briefing_uses_point_and_footprint_transforms_and_is_involutive() {
        let scenario = Scenario::skirmish();
        let briefing = PublicMapBriefing::from_scenario(&scenario).expect("skirmish briefing");
        let start = briefing.starting_foundries()[0];
        let frame = briefing.extractor_frames()[0];
        let scrap = briefing.initial_scrap()[0];
        let terrain = briefing.non_ground_terrain()[0];
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        let extractor_size = BuildingKind::Extractor.base_stats().size;

        for (flip_x, flip_y) in [(true, false), (false, true), (true, true)] {
            let orientation = Orientation {
                flip_x,
                flip_y,
                width: briefing.map_width(),
                height: briefing.map_height(),
            };
            let oriented = orientation.briefing(&briefing);
            let oriented_start = oriented
                .starting_foundries()
                .iter()
                .find(|candidate| candidate.player == start.player)
                .expect("the player identity is unchanged");

            assert_eq!(
                oriented_start.anchor,
                orientation.anchor(start.anchor, foundry_size)
            );
            assert!(
                oriented
                    .extractor_frames()
                    .contains(&orientation.anchor(frame, extractor_size))
            );
            assert!(
                oriented
                    .initial_scrap()
                    .contains(&(orientation.tile(scrap.0), scrap.1))
            );
            assert_eq!(
                oriented.terrain_at(orientation.tile(terrain.0)),
                Some(terrain.1)
            );
            assert_eq!(orientation.briefing(&oriented), briefing);
            assert!(oriented.non_ground_terrain().windows(2).all(|pair| (
                pair[0].0.y,
                pair[0].0.x
            ) < (
                pair[1].0.y,
                pair[1].0.x
            )));
        }
    }

    #[test]
    fn one_axis_orientation_maps_masks_footprints_and_every_positioned_collection() {
        let obs = observation();
        let orientation = Orientation::for_home(&obs, TilePos::new(6, 1));
        assert!(!orientation.is_identity());
        assert_eq!(orientation.tile(TilePos::new(1, 2)), TilePos::new(6, 2));
        assert_eq!(
            orientation.anchor(TilePos::new(2, 2), (2, 2)),
            TilePos::new(4, 2)
        );

        let oriented = orientation.observe(&obs);
        assert!(oriented.visible(TilePos::new(6, 2)));
        assert!(!oriented.visible(TilePos::new(1, 2)));
        assert!(oriented.explored(TilePos::new(5, 3)));
        assert!(!oriented.explored(TilePos::new(2, 3)));
        assert_eq!(oriented.my_units[0].tile, TilePos::new(6, 1));
        assert_eq!(
            oriented.my_units[0].founding,
            Some((BuildingKind::Foundry, TilePos::new(4, 2)))
        );
        assert_eq!(oriented.ally_units[0].tile, TilePos::new(5, 3));
        assert_eq!(oriented.enemy_units[0].tile, TilePos::new(4, 4));
        assert_eq!(oriented.my_buildings[0].anchor, TilePos::new(5, 1));
        assert_eq!(oriented.ally_buildings[0].anchor, TilePos::new(5, 1));
        assert_eq!(oriented.enemy_buildings[0].anchor, TilePos::new(2, 3));
        assert_eq!(
            oriented.known_scrap,
            vec![(TilePos::new(2, 0), 70), (TilePos::new(6, 0), 50)]
        );
        assert_eq!(
            oriented.known_frames,
            vec![TilePos::new(1, 2), TilePos::new(5, 2)]
        );
        assert_eq!(
            oriented.known_rock,
            vec![TilePos::new(2, 1), TilePos::new(6, 1)]
        );
        assert_eq!(
            oriented.known_peaks,
            vec![TilePos::new(2, 3), TilePos::new(6, 3)]
        );
        assert_eq!(oriented.known_wrecks, vec![(TilePos::new(5, 4), 30)]);
        assert_eq!(oriented.salvage_incidents, vec![TilePos::new(4, 4)]);
        assert_eq!(oriented.blips, vec![TilePos::new(3, 4)]);
        assert_eq!(
            oriented.incoming_shells,
            vec![TilePos::new(2, 5), TilePos::new(6, 5)]
        );
        assert_eq!(orientation.observe(&oriented), obs);
    }

    #[test]
    fn orientation_transforms_an_own_harvest_node_and_is_involutive() {
        let mut obs = observation();
        let node = TilePos::new(2, 4);
        obs.my_units[0].harvesting = Some(node);
        let orientation = Orientation::for_home(&obs, TilePos::new(6, 5));

        let oriented = orientation.observe(&obs);

        assert_eq!(
            oriented.my_units[0].harvesting,
            Some(orientation.tile(node))
        );
        assert_eq!(orientation.observe(&oriented), obs);
    }

    #[test]
    fn emission_maps_every_positioned_intent_and_leaves_id_intents_unchanged() {
        let obs = observation();
        let orientation = Orientation::for_home(&obs, TilePos::new(6, 5));
        let position = TilePos::new(1, 2);
        let tile = orientation.tile(position);
        let foundry_anchor = orientation.anchor(position, BuildingKind::Foundry.base_stats().size);
        let turret_anchor = orientation.anchor(position, BuildingKind::Turret.base_stats().size);
        let positioned = vec![
            Intent::Build {
                kind: BuildingKind::Foundry,
                anchor: position,
            },
            Intent::BuildWith {
                builder: UnitId(1),
                kind: BuildingKind::Turret,
                anchor: position,
            },
            Intent::FormArmy {
                staging: position,
                size: 3,
            },
            Intent::PushArmy {
                army: ArmyId(4),
                target: position,
            },
            Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: position,
            },
            Intent::AttackMoveUnits {
                units: vec![UnitId(3)],
                goal: position,
            },
            Intent::AssignHarvest {
                unit: UnitId(1),
                node: position,
            },
            Intent::Scout {
                unit: UnitId(2),
                to: position,
            },
            Intent::RaidAir { target: position },
            Intent::Unload {
                transport: UnitId(3),
                at: position,
            },
        ];
        assert_eq!(
            orientation.emit(positioned),
            vec![
                Intent::Build {
                    kind: BuildingKind::Foundry,
                    anchor: foundry_anchor,
                },
                Intent::BuildWith {
                    builder: UnitId(1),
                    kind: BuildingKind::Turret,
                    anchor: turret_anchor,
                },
                Intent::FormArmy {
                    staging: tile,
                    size: 3,
                },
                Intent::PushArmy {
                    army: ArmyId(4),
                    target: tile,
                },
                Intent::MoveUnits {
                    units: vec![UnitId(2)],
                    goal: tile,
                },
                Intent::AttackMoveUnits {
                    units: vec![UnitId(3)],
                    goal: tile,
                },
                Intent::AssignHarvest {
                    unit: UnitId(1),
                    node: tile,
                },
                Intent::Scout {
                    unit: UnitId(2),
                    to: tile,
                },
                Intent::RaidAir { target: tile },
                Intent::Unload {
                    transport: UnitId(3),
                    at: tile,
                },
            ]
        );

        let positionless = vec![
            Intent::TrainAt {
                building: BuildingId(1),
                kind: UnitKind::Harvester,
            },
            Intent::Repair {
                building: BuildingId(2),
            },
            Intent::Salvage {
                building: BuildingId(3),
            },
            Intent::Upgrade {
                building: BuildingId(4),
            },
            Intent::Load {
                transport: UnitId(5),
                riders: vec![UnitId(6)],
            },
            Intent::AttackUnits {
                units: vec![UnitId(7)],
                target: Target::Unit(UnitId(8)),
            },
            Intent::RepairUnits {
                welders: vec![UnitId(9)],
                target: UnitId(10),
            },
            Intent::StopUnits {
                units: vec![UnitId(11)],
            },
        ];
        assert_eq!(orientation.emit(positionless.clone()), positionless);

        let army = Army {
            id: ArmyId(1),
            members: vec![UnitId(1)],
            state: ArmyState::Pushing,
            staging: position,
            target: Some(TilePos::new(3, 4)),
            focus: Some(UnitId(9)),
            progress: Some((5, 7)),
            issued: Some((6, TilePos::new(2, 3))),
            bounces: 1,
        };
        let oriented_army = orientation.army(army.clone());
        assert_eq!(oriented_army.staging, tile);
        assert_eq!(
            oriented_army.target,
            Some(orientation.tile(TilePos::new(3, 4)))
        );
        assert_eq!(oriented_army.members, army.members);
        assert_eq!(oriented_army.focus, army.focus);
        assert_eq!(oriented_army.progress, army.progress);
        assert_eq!(oriented_army.issued, army.issued);

        let identity = Orientation::for_home(&obs, TilePos::new(1, 1));
        assert!(identity.is_identity());
        assert_eq!(identity.emit(positionless.clone()), positionless);
        assert_eq!(identity.observe(&obs), obs);
    }
}
