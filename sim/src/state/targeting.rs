//! Attack objectives resolved through the issuing team's knowledge.

use super::State;
use crate::stats::Domain;
use crate::{AttackTarget, PlayerId, RememberedBuilding, Target};
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

/// Current aiming information available to the firing team.
#[derive(Debug, Clone, Copy)]
pub struct AttackView {
    /// A positively identified victim, never an unseen entity.
    pub entity: Option<Target>,
    /// Reported position or remembered footprint center.
    pub position: Vec2Fx,
    /// Remembered building rectangle, when this is a static objective.
    pub footprint: Option<(TilePos, (i32, i32))>,
    /// Known domain. Radar contacts deliberately leave it absent.
    pub domain: Option<Domain>,
    /// Motion estimated from radar observations.
    pub velocity: Vec2Fx,
}

impl AttackView {
    /// Closest point on a known footprint, or the reported contact position.
    pub fn aim_from(self, from: Vec2Fx) -> Vec2Fx {
        match self.footprint {
            Some((anchor, (w, h))) => Vec2Fx::new(
                from.x
                    .clamp(Fx::from_num(anchor.x), Fx::from_num(anchor.x + w)),
                from.y
                    .clamp(Fx::from_num(anchor.y), Fx::from_num(anchor.y + h)),
            ),
            None => self.position,
        }
    }
}

impl State {
    /// Normalize a command using only its issuer's current knowledge.
    pub fn attack_objective(&self, player: PlayerId, target: AttackTarget) -> Option<AttackTarget> {
        let view = self.vision(player);
        match target {
            AttackTarget::Unit(id) => {
                self.visible_hostile_target_domain(player, Target::Unit(id))?;
                let track = view.tracks().iter().find(|t| t.visible_unit == Some(id))?;
                Some(AttackTarget::Contact(track.id))
            }
            AttackTarget::Building(id) => {
                self.visible_hostile_target_domain(player, Target::Building(id))?;
                let b = self.building(id)?;
                Some(AttackTarget::RememberedBuilding(RememberedBuilding {
                    owner: b.player,
                    building_kind: b.kind,
                    anchor: b.anchor,
                }))
            }
            AttackTarget::RememberedBuilding(memory) => view
                .ghosts()
                .iter()
                .any(|g| {
                    g.owner == memory.owner
                        && g.kind == memory.building_kind
                        && g.anchor == memory.anchor
                })
                .then_some(target),
            AttackTarget::Contact(id) => view.track(id).map(|_| target),
        }
    }

    /// Resolve an attack without using the concealed world's identity or state.
    pub fn attack_view(&self, player: PlayerId, target: AttackTarget) -> Option<AttackView> {
        let target = self.attack_objective(player, target)?;
        match target {
            AttackTarget::Contact(id) => {
                let track = self.vision(player).track(id)?;
                if let Some(id) = track.visible_unit {
                    let domain = self.visible_hostile_target_domain(player, Target::Unit(id))?;
                    let u = self.unit(id)?;
                    Some(AttackView {
                        entity: Some(Target::Unit(id)),
                        position: u.pos,
                        footprint: None,
                        domain: Some(domain),
                        velocity: track.velocity(),
                    })
                } else {
                    Some(AttackView {
                        entity: None,
                        position: track.tile.center(),
                        footprint: None,
                        domain: None,
                        velocity: track.velocity(),
                    })
                }
            }
            AttackTarget::RememberedBuilding(memory) => {
                let size = memory.building_kind.base_stats().size;
                let entity = self
                    .buildings()
                    .iter()
                    .find(|b| {
                        b.anchor == memory.anchor
                            && b.kind == memory.building_kind
                            && b.player == memory.owner
                            && self
                                .visible_hostile_target_domain(player, Target::Building(b.id))
                                .is_some()
                    })
                    .map(|b| Target::Building(b.id));
                Some(AttackView {
                    entity,
                    position: Vec2Fx::new(
                        Fx::from_num(memory.anchor.x) + Fx::from_num(size.0) / Fx::from_num(2),
                        Fx::from_num(memory.anchor.y) + Fx::from_num(size.1) / Fx::from_num(2),
                    ),
                    footprint: Some((memory.anchor, size)),
                    domain: Some(Domain::Ground),
                    velocity: Vec2Fx::ZERO,
                })
            }
            AttackTarget::Unit(_) | AttackTarget::Building(_) => unreachable!("normalized above"),
        }
    }

    pub(super) fn valid_attack_reference(&self, player: PlayerId, target: AttackTarget) -> bool {
        match target {
            AttackTarget::Contact(id) => self.vision(player).minted_contact(id),
            AttackTarget::RememberedBuilding(memory) => {
                usize::from(memory.owner.0) < self.players.len()
                    && self.hostile(player, memory.owner)
                    && memory.anchor.x >= 0
                    && memory.anchor.y >= 0
                    && memory
                        .anchor
                        .x
                        .checked_add(memory.building_kind.base_stats().size.0)
                        .is_some_and(|edge| edge <= self.map().width())
                    && memory
                        .anchor
                        .y
                        .checked_add(memory.building_kind.base_stats().size.1)
                        .is_some_and(|edge| edge <= self.map().height())
            }
            other => other.entity().is_some_and(|entity| self.minted(entity)),
        }
    }
}
