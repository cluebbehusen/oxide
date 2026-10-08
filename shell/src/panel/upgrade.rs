//! Completed-tier comparisons for upgrade previews.

use super::info::SelectionInfo;
use oxide_sim::{
    BuildingKind,
    stats::{
        CHARGE_ARRAY_DETECT_RADIUS, CHARGE_BASE_ARRAY_DETECT_RADIUS, RECLAIMER_PERIOD,
        REFINERY_PERIOD,
    },
};

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
mod tests;
