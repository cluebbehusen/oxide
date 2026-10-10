use super::*;

#[test]
fn every_authored_weapon_report_raises_combat_pressure() {
    for kind in [
        SoundKind::Alert,
        SoundKind::SentinelFire,
        SoundKind::ScuttlerFire,
        SoundKind::LancerFire,
        SoundKind::BombardFire,
        SoundKind::FlakhoundFire,
        SoundKind::BuzzardFire,
        SoundKind::TalonFire,
        SoundKind::BastionFire,
        SoundKind::FlakTurretFire,
        SoundKind::ArtilleryLaunch,
        SoundKind::WardenFire,
        SoundKind::BreakerFire,
        SoundKind::AvalancheFire,
        SoundKind::BombRelease,
        SoundKind::DemolitionBoom,
    ] {
        assert!(spec(kind).combat, "{kind:?} must pressure the score");
    }
}

#[test]
fn every_sound_kind_mixes_from_its_manifest_row() {
    for kind in SoundKind::ALL {
        let spec = mixer_spec(kind);
        assert!(spec.volume > 0.0 && spec.min_gap > 0.0, "{kind:?}");
    }
    assert_eq!(MIXER_SPECS.get("laser2"), MIXER_SPECS.get("laser"));
}
