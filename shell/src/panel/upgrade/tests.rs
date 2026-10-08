use super::*;
use crate::numeric::Fit;

fn values(kind: BuildingKind, tier: u8, label: &str) -> (String, String) {
    let row = comparison(kind, tier)
        .unwrap()
        .rows
        .into_iter()
        .find(|r| r.label == label)
        .unwrap();
    let current = tier_facts(kind, tier)
        .into_iter()
        .find(|(name, _)| name == label)
        .unwrap()
        .1;
    (current, row.upgraded)
}

#[test]
fn every_upgrade_compares_completed_health_and_stops_at_the_top() {
    for kind in BuildingKind::ALL {
        for tier in 0..kind.tiers().len().fit::<u8>() - 1 {
            assert_eq!(
                values(kind, tier, "Max health"),
                (
                    format!("{} hp", kind.tier_stats(tier).max_hp),
                    format!("{} hp", kind.tier_stats(tier + 1).max_hp),
                )
            );
        }
        assert!(comparison(kind, kind.tiers().len().fit::<u8>() - 1).is_none());
    }
}

#[test]
fn previews_include_tradeoffs_and_non_weapon_benefits() {
    assert_eq!(
        values(BuildingKind::Turret, 1, "Ground reload"),
        ("1.3s".into(), "2.5s".into())
    );
    assert_eq!(
        values(BuildingKind::Turret, 1, "Ground damage"),
        ("20 dmg/hit".into(), "60 dmg/hit".into())
    );
    assert_eq!(
        values(BuildingKind::FlakTurret, 0, "Air splash"),
        ("1.2 tiles".into(), "1.5 tiles".into())
    );
    assert_eq!(
        values(BuildingKind::Reclaimer, 0, "Income"),
        ("50 scrap/min".into(), "120 scrap/min".into())
    );
    assert_eq!(
        values(BuildingKind::Array, 0, "Sight"),
        ("9 tiles".into(), "11 tiles".into())
    );
    assert_eq!(
        values(BuildingKind::Array, 0, "Mine detection"),
        (
            format!("{CHARGE_BASE_ARRAY_DETECT_RADIUS} tiles"),
            format!("{CHARGE_ARRAY_DETECT_RADIUS} tiles")
        )
    );
}
