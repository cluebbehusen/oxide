use super::super::observation::{BuildingObs, Observation};
use crate::stats::Domain;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

/// Potential fire from known geometry; this does not assert hidden enemy vision.
pub(in crate::bot) fn building_threatens(
    obs: &Observation,
    building: &BuildingObs,
    target: TilePos,
    domain: Domain,
) -> bool {
    if !building.built || building.hp == 0 {
        return false;
    }
    let stats = building.kind.tier_stats(building.tier);
    let center = Vec2Fx::new(
        Fx::from_num(building.anchor.x) + Fx::from_num(stats.size.0) / Fx::from_num(2),
        Fx::from_num(building.anchor.y) + Fx::from_num(stats.size.1) / Fx::from_num(2),
    );
    let target = target.center();
    let distance = center.dist_sq(target);
    stats.weapons.iter().any(|weapon| {
        if !weapon.targets.covers(domain)
            || distance < weapon.minimum_range * weapon.minimum_range
            || distance > weapon.range * weapon.range
        {
            return false;
        }
        let crosses = |tile: TilePos| {
            if !weapon.indirect && domain == Domain::Ground {
                !obs.known_rock_at(tile)
            } else {
                obs.known_peaks
                    .binary_search_by_key(&(tile.y, tile.x), |p| (p.y, p.x))
                    .is_err()
            }
        };
        crosses(TilePos::containing(target))
            && !chassis::path::line_blocked(center, target, crosses)
    })
}

pub(in crate::bot) fn building_strength(building: &BuildingObs, domain: Domain) -> u64 {
    if !building.built {
        return 0;
    }
    u64::from(building.hp)
        * super::weapon_dps100(building.kind.tier_stats(building.tier).weapons, domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildingId, BuildingKind, PlayerId};

    #[test]
    fn static_risk_respects_known_cover_domain_and_construction() {
        let mut obs = Observation::default();
        let mut building = BuildingObs {
            id: BuildingId(1),
            player: PlayerId(1),
            kind: BuildingKind::Turret,
            anchor: TilePos::new(4, 4),
            hp: 900,
            built: true,
            seen: true,
            tier: 2,
        };
        let target = TilePos::new(7, 4);
        assert!(building_threatens(&obs, &building, target, Domain::Ground));
        assert!(!building_threatens(&obs, &building, target, Domain::Air));
        obs.known_rock.push(TilePos::new(6, 4));
        assert!(!building_threatens(&obs, &building, target, Domain::Ground));
        obs.known_rock.clear();
        building.built = false;
        assert!(!building_threatens(&obs, &building, target, Domain::Ground));
    }
}
