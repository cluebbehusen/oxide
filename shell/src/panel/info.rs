//! Selection facts, independent of their placement in the HUD.

use super::*;
use crate::game::Scene;
use oxide_sim::stats::{
    BuildingKind, CHARGE_ARRAY_DETECT_RADIUS, CHARGE_BASE_ARRAY_DETECT_RADIUS, CHARGE_BLAST_RADIUS,
    CHARGE_DAMAGE, CHARGE_SCOUT_DETECT_RADIUS, CHARGE_TRIGGER_RADIUS, CRUCIBLE_SMELT_RADIUS,
    Domain, FOUNDRY_DRIP_START_TICK, RADAR_DETECT_RADIUS, REPAIR_BAY_PERIOD, REPAIR_BAY_RADIUS,
    REPAIR_BAY_STEP, UnitKind, WeaponStats,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatIcon {
    Capability(CapabilityIcon),
    Verb(VerbIcon),
}

#[derive(Debug, Clone)]
pub(crate) struct StatRow {
    pub label: String,
    pub value: String,
    pub icon: Option<StatIcon>,
    pub section: bool,
}

#[derive(Debug, Default)]
pub(crate) struct SelectionInfo {
    pub health: Option<(u32, u32)>,
    pub status: Vec<String>,
    pub rows: Vec<StatRow>,
    pub upgrade: Option<super::upgrade::UpgradeComparison>,
}

impl SelectionInfo {
    fn row(&mut self, label: &str, value: String, icon: Option<StatIcon>) {
        self.rows.push(StatRow {
            label: label.into(),
            value,
            icon,
            section: false,
        });
    }

    pub(super) fn weapons(&mut self, weapons: &[WeaponStats]) {
        for weapon in weapons {
            let label = match (
                weapon.targets.covers(Domain::Ground),
                weapon.targets.covers(Domain::Air),
            ) {
                (true, true) => "Ground + air",
                (false, true) => "Air",
                _ => "Ground",
            };
            self.row(
                label,
                format!("{} dmg/hit", weapon.damage),
                Some(StatIcon::Capability(weapon_capability_icon(weapon))),
            );
            self.rows.last_mut().unwrap().section = true;
            let range = if weapon.minimum_range > chassis::fx::Fx::ZERO {
                format!(
                    "{:.1}-{:.1} tiles",
                    weapon.minimum_range.to_num::<f32>(),
                    weapon.range.to_num::<f32>()
                )
            } else {
                format!("{:.1} tiles", weapon.range.to_num::<f32>())
            };
            self.row("Range", range, None);
            self.row("Reload", tick_time_label(weapon.cooldown_ticks), None);
            if weapon.salvo > 1 {
                self.row("Salvo", format!("{} bombs", weapon.salvo), None);
            }
            if let Some(radius) = weapon.splash {
                self.row(
                    "Splash",
                    format!("{:.1} tiles", radius.to_num::<f32>()),
                    None,
                );
            }
        }
    }

    fn ownership(&mut self, game: &Scene<'_>, owner: oxide_sim::PlayerId) {
        if owner != game.presentation.human {
            let relation = if game.state.hostile(game.presentation.human, owner) {
                "Hostile"
            } else {
                "Ally"
            };
            self.status
                .push(format!("{relation}: {}", game.state.player(owner).name));
            if let Some(controller) = bot_controller_label(game, owner) {
                self.status.push(controller);
            }
        }
    }
}

pub(crate) fn selection_info(game: &Scene<'_>, panel: &Panel) -> SelectionInfo {
    use StatIcon::{Capability as Cap, Verb};
    let mut info = SelectionInfo::default();
    if let Some(tile) = game.presentation.selection.pile {
        if let Some(salvage) = game.known_salvage(tile) {
            info.row(
                "Scrap left",
                salvage.amount().to_string(),
                Some(Verb(VerbIcon::Harvest)),
            );
            // Own harvesters only: another player's orders are intent
            // the viewer has no license to read.
            let harvesters = game
                .state
                .units()
                .iter()
                .filter(|u| {
                    u.player == game.presentation.human
                        && matches!(u.order, oxide_sim::Order::Harvest { node, .. } if node == tile)
                })
                .count();
            info.row("Harvesters", harvesters.to_string(), None);
        }
        return info;
    }
    if game.presentation.selection.buildings.len() == 1 {
        let Some(b) = game
            .state
            .building(game.presentation.selection.buildings[0])
        else {
            return info;
        };
        let stats = b.stats();
        if b.player == game.presentation.human && b.built {
            info.upgrade = super::upgrade::comparison(b.kind, b.tier);
        }
        info.health = Some((b.hp, stats.max_hp));
        info.ownership(game, b.player);
        if !b.built {
            info.status.push(
                if b.tier > 0 {
                    "Upgrading"
                } else {
                    "Under construction"
                }
                .into(),
            );
        }
        info.row(
            "Sight",
            format!("{} tiles", stats.vision),
            Some(Cap(CapabilityIcon::Vision)),
        );
        if b.player == game.presentation.human {
            if let Some(income) = game.state.extractor_income(b.id) {
                info.row(
                    "Income",
                    format!("{} scrap/min", income.scrap_per_minute()),
                    Some(Verb(VerbIcon::Harvest)),
                );
                info.row(
                    "Support",
                    if income.is_supported() {
                        "Foundry".to_string()
                    } else {
                        "Remote".to_string()
                    },
                    Some(Cap(CapabilityIcon::EconomySupport)),
                );
            } else if matches!(b.kind, BuildingKind::Foundry | BuildingKind::Reclaimer) {
                let income = building_income(game, b);
                if b.built && b.kind == BuildingKind::Foundry && income == 0 {
                    let remaining = FOUNDRY_DRIP_START_TICK
                        .saturating_sub(game.state.current_tick())
                        .div_ceil(u64::from(oxide_sim::TICKS_PER_SECOND));
                    info.row(
                        "Income starts",
                        format!("{remaining}s"),
                        Some(Verb(VerbIcon::Harvest)),
                    );
                } else {
                    info.row(
                        "Income",
                        format!("{income} scrap/min"),
                        Some(Verb(VerbIcon::Harvest)),
                    );
                }
            }
        }
        match b.kind {
            BuildingKind::Array => {
                info.row(
                    "Radar",
                    format!("{RADAR_DETECT_RADIUS} tiles"),
                    Some(Cap(CapabilityIcon::Radar)),
                );
                let radius = if b.tier == 0 {
                    CHARGE_BASE_ARRAY_DETECT_RADIUS
                } else {
                    CHARGE_ARRAY_DETECT_RADIUS
                };
                info.row(
                    "Mine detection",
                    format!("{radius} tiles"),
                    Some(Cap(CapabilityIcon::Radar)),
                );
            }
            BuildingKind::RepairBay => {
                info.row(
                    "Repair radius",
                    format!("{:.1} tiles", REPAIR_BAY_RADIUS.to_num::<f32>()),
                    Some(Cap(CapabilityIcon::Repair)),
                );
                info.row(
                    "Repair rate",
                    format!(
                        "{:.1} hp/s",
                        REPAIR_BAY_STEP as f32 * oxide_sim::TICKS_PER_SECOND as f32
                            / REPAIR_BAY_PERIOD as f32
                    ),
                    None,
                );
            }
            BuildingKind::Crucible => info.row(
                "Smelt reach",
                format!("{:.1} tiles", CRUCIBLE_SMELT_RADIUS.to_num::<f32>()),
                Some(Verb(VerbIcon::Harvest)),
            ),
            BuildingKind::ScuttleCharge => {
                info.row(
                    "Ground blast",
                    format!("{CHARGE_DAMAGE} damage"),
                    Some(Cap(CapabilityIcon::Weapon)),
                );
                info.row(
                    "Blast radius",
                    format!("{:.1} tiles", CHARGE_BLAST_RADIUS.to_num::<f32>()),
                    None,
                );
                info.row(
                    "Trigger",
                    format!("{:.1} tiles", CHARGE_TRIGGER_RADIUS.to_num::<f32>()),
                    None,
                );
            }
            _ => {}
        }
        info.weapons(stats.weapons);
    } else if game.presentation.selection.units.len() == 1 {
        let Some(u) = game.state.unit(game.presentation.selection.units[0]) else {
            return info;
        };
        let stats = u.kind.stats();
        info.health = Some((u.hp, stats.max_hp));
        info.ownership(game, u.player);
        if u.landed {
            info.status.push("Landed".into());
        }
        info.row(
            "Speed",
            format!(
                "{} tiles/s",
                unit_speed_label(u.kind).trim_end_matches(" tiles/sec")
            ),
            Some(Verb(VerbIcon::Move)),
        );
        info.row(
            "Sight",
            format!("{} tiles", stats.vision),
            Some(Cap(CapabilityIcon::Vision)),
        );
        if matches!(u.kind, UnitKind::Kestrel | UnitKind::Gnat) {
            info.row(
                "Mine detection",
                format!("{CHARGE_SCOUT_DETECT_RADIUS} tiles"),
                Some(Cap(CapabilityIcon::Radar)),
            );
        }
        if stats.transport_capacity > 0 {
            let value = if game.state.hostile(game.presentation.human, u.player) {
                format!("{} points", stats.transport_capacity)
            } else {
                let held: u8 = u.cargo.iter().map(|r| r.kind.stats().transport_size).sum();
                format!("{held}/{} points", stats.transport_capacity)
            };
            info.row("Cargo", value, Some(Verb(VerbIcon::Build)));
        }
        if let Some(harvest) = stats.harvest {
            info.row(
                "Scrap load",
                if u.player == game.presentation.human {
                    format!("{}/{}", u.carrying, harvest.capacity)
                } else {
                    format!("{} capacity", harvest.capacity)
                },
                Some(Verb(VerbIcon::Harvest)),
            );
            info.row(
                "Gather",
                format!(
                    "{:.1} scrap/s",
                    oxide_sim::TICKS_PER_SECOND as f32 / harvest.ticks_per_scrap as f32
                ),
                None,
            );
        }
        if let Some(demolition) = stats.demolition {
            info.row(
                "Structure hit",
                format!("{} damage", demolition.structure_damage),
                Some(Cap(CapabilityIcon::Weapon)),
            );
            info.row(
                "Ground blast",
                format!("{} damage", demolition.splash_damage),
                None,
            );
            info.row(
                "Blast radius",
                format!("{:.1} tiles", demolition.blast_radius.to_num::<f32>()),
                None,
            );
        }
        info.weapons(stats.weapons);
    } else {
        if !panel.summary.is_empty() {
            info.status.push(panel.summary.clone());
        }
        if let Some(u) = subject_unit(game).and_then(|id| game.state.unit(id)) {
            info.ownership(game, u.player);
        } else if let Some(b) = game
            .presentation
            .selection
            .buildings
            .first()
            .and_then(|id| game.state.building(*id))
        {
            info.ownership(game, b.player);
        }
    }
    info
}

#[cfg(test)]
mod tests;
