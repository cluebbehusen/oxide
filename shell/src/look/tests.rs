use super::*;
use oxide_sim::stats::Domain;

/// A flyer held up on lift rotors: no forward-flight turn rate at all, as
/// the simulation's stats state.
fn rotorcraft(kind: UnitKind) -> bool {
    let stats = kind.stats();
    stats.domain == Domain::Air && stats.turn_rate == 0 && stats.cruise_turn_rate == 0
}

#[test]
fn looks_agree_with_what_the_simulation_states() {
    for kind in UnitKind::ALL {
        let look = unit(kind);
        let stats = kind.stats();
        assert_eq!(
            look.airframe.is_some(),
            stats.crash.is_some(),
            "{kind:?}: only a large airframe has its own shadow"
        );
        assert_eq!(
            matches!(look.gait, Gait::Rotor { .. }),
            rotorcraft(kind),
            "{kind:?}: rotor gait follows the stats"
        );
        assert_eq!(
            look.rig,
            stats.turret_turn_rate > 0 || rotorcraft(kind),
            "{kind:?}: a separate mount layer is a turret or a rotor"
        );
        assert_eq!(
            look.weapon.is_some(),
            stats.can_fight(),
            "{kind:?}: every kind that fights has a report"
        );
        assert_eq!(
            look.tool.is_some(),
            stats.harvest.is_some() || stats.welder || stats.contact_reach.is_some(),
            "{kind:?}: a worker body belongs to the kinds that work with a tool"
        );
        assert!(
            look.belts.len().is_multiple_of(2),
            "{kind:?}: belt runs come in pairs"
        );
    }
}

#[test]
fn a_direct_report_belongs_to_every_gun_without_a_shell() {
    for kind in UnitKind::ALL {
        let shells = kind
            .stats()
            .weapons
            .iter()
            .any(|weapon| weapon.projectile.is_some());
        let demolition = kind.stats().demolition.is_some();
        if let Some(weapon) = unit(kind).weapon {
            assert_eq!(
                weapon.shot.is_none(),
                shells || demolition,
                "{kind:?}: shells and charges draw their own way"
            );
        }
    }
}

#[test]
fn markers_name_each_kind_for_what_it_does_on_the_field() {
    use MarkerRole as M;
    for (kind, role) in [
        (UnitKind::Harvester, M::Worker),
        (UnitKind::Excavator, M::Worker),
        (UnitKind::Sentinel, M::Gun),
        (UnitKind::Buzzard, M::Gun),
        (UnitKind::Bombard, M::Siege),
        (UnitKind::Condor, M::Siege),
        (UnitKind::Flakhound, M::AntiAir),
        (UnitKind::Shrike, M::AntiAir),
        (UnitKind::Kestrel, M::Scout),
        (UnitKind::Tender, M::Support),
        (UnitKind::Skyhook, M::Transport),
        (UnitKind::Scuttler, M::Demolition),
        (UnitKind::Sapper, M::Demolition),
    ] {
        assert_eq!(unit(kind).marker, role, "{kind:?}");
    }
    for kind in UnitKind::ALL {
        assert_eq!(
            unit(kind).marker == M::Scout,
            scout(kind),
            "{kind:?}: the scout marker follows the simulation's scouts"
        );
    }
}

#[test]
fn defense_looks_agree_with_each_rung_of_the_ladder() {
    for kind in BuildingKind::ALL {
        let rungs = kind.tiers();
        let armed = rungs.iter().any(|stats| !stats.weapons.is_empty());
        let look = defense(kind);
        assert_eq!(look.is_some(), armed, "{kind:?}");
        let Some(look) = look else { continue };
        let direct = rungs
            .iter()
            .flat_map(|stats| stats.weapons)
            .all(|weapon| weapon.projectile.is_none());
        assert_eq!(look.report.is_some(), direct, "{kind:?}");
        if let Some(report) = look.report {
            assert_eq!(report.shots.len(), rungs.len(), "{kind:?}: a shot per rung");
        }
    }
}
