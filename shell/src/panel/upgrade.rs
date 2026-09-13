//! Completed-tier comparisons for upgrade previews.

use super::info::SelectionInfo;
use oxide_sim::{BuildingKind, stats::*};

#[derive(Debug)]
pub(crate) struct UpgradeRow {
    pub label: String,
    pub upgraded: String,
}

#[derive(Debug)]
pub(crate) struct UpgradeComparison {
    pub rows: Vec<UpgradeRow>,
}

fn tier_facts(kind: BuildingKind, tier: u8) -> Vec<(String, String)> {
    let stats = kind.tier_stats(tier);
    let mut facts = vec![
        ("Max health".into(), format!("{} hp", stats.max_hp)),
        ("Sight".into(), format!("{} tiles", stats.vision)),
    ];
    for (index, weapon) in stats.weapons.iter().enumerate() {
        let mut info = SelectionInfo::default();
        info.weapons(std::slice::from_ref(weapon));
        let target = info.rows[0].label.clone();
        let prefix = if stats.weapons.len() > 1 {
            format!("{target} {}", index + 1)
        } else {
            target
        };
        for (i, row) in info.rows.into_iter().enumerate() {
            let label = if i == 0 {
                "damage".into()
            } else {
                row.label.to_lowercase()
            };
            facts.push((format!("{prefix} {label}"), row.value));
        }
    }
    match kind {
        BuildingKind::Reclaimer => {
            let period = if tier == 0 {
                RECLAIMER_PERIOD
            } else {
                REFINERY_PERIOD
            };
            facts.push((
                "Income".into(),
                format!(
                    "{} scrap/min",
                    60 * u64::from(oxide_sim::TICKS_PER_SECOND) / period
                ),
            ));
        }
        BuildingKind::Array => {
            let radius = if tier == 0 {
                CHARGE_BASE_ARRAY_DETECT_RADIUS
            } else {
                CHARGE_ARRAY_DETECT_RADIUS
            };
            facts.push(("Mine detection".into(), format!("{radius} tiles")));
        }
        _ => {}
    }
    facts
}

pub(super) fn comparison(kind: BuildingKind, tier: u8) -> Option<UpgradeComparison> {
    kind.upgrade_from(tier)?;
    let current = tier_facts(kind, tier);
    let upgraded = tier_facts(kind, tier + 1);
    let mut rows = Vec::new();
    for (label, value) in &current {
        let next = upgraded
            .iter()
            .find(|(name, _)| name == label)
            .map_or("—", |(_, value)| value);
        if value != next {
            rows.push(UpgradeRow {
                label: label.clone(),
                upgraded: next.into(),
            });
        }
    }
    for (label, value) in upgraded {
        if !current.iter().any(|(name, _)| *name == label) {
            rows.push(UpgradeRow {
                label,
                upgraded: value,
            });
        }
    }
    Some(UpgradeComparison { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            for tier in 0..kind.tiers().len() as u8 - 1 {
                assert_eq!(
                    values(kind, tier, "Max health"),
                    (
                        format!("{} hp", kind.tier_stats(tier).max_hp),
                        format!("{} hp", kind.tier_stats(tier + 1).max_hp),
                    )
                );
            }
            assert!(comparison(kind, kind.tiers().len() as u8 - 1).is_none());
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
}
