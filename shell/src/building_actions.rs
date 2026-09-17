//! Building capabilities and orders shared by single and grouped selections.

use crate::action::{Action, BindingMap};
use crate::game::{Game, Scene};
use crate::panel::{Card, CardAction, CardIcon, VerbIcon, tick_time_label};
use crate::typography::entity_name;
use oxide_sim::{Building, BuildingId, BuildingKind, Command, Faction};

#[derive(Clone)]
pub(crate) struct SelectedBuildings {
    pub buildings: Vec<Building>,
    pub tech: Vec<BuildingKind>,
    pub scrap: u32,
    pub faction: Faction,
    pub accepts: bool,
}

pub(crate) struct UpgradeBatch {
    pub recipients: Vec<BuildingId>,
    pub cost: u32,
    pub reason: Option<String>,
}

impl SelectedBuildings {
    pub fn inspect(game: &Scene<'_>) -> Self {
        Self::inspect_ids(game, &game.presentation.selection.buildings)
    }

    pub fn inspect_ids(game: &Scene<'_>, ids: &[BuildingId]) -> Self {
        let snapshot = |buildings: &[Building], scrap, accepts| Self {
            buildings: buildings
                .iter()
                .filter(|b| b.player == game.presentation.human && ids.contains(&b.id))
                .cloned()
                .collect(),
            tech: buildings
                .iter()
                .filter(|b| b.player == game.presentation.human && b.built)
                .map(|b| b.kind)
                .collect(),
            scrap,
            faction: game.state.player(game.presentation.human).faction,
            accepts,
        };
        if game.pending.is_empty() {
            let player = game.state.player(game.presentation.human);
            snapshot(
                game.state.buildings(),
                player.scrap,
                game.state.result().is_none() && !player.resigned && game.home_foundry().is_some(),
            )
        } else {
            game.state.inspect_command_phase(game.pending, |state| {
                snapshot(
                    state.buildings(),
                    state.scrap(game.presentation.human).unwrap_or(0),
                    state.accepts_commands(game.presentation.human),
                )
            })
        }
    }

    pub fn homogeneous(&self) -> bool {
        self.buildings
            .first()
            .is_some_and(|first| self.buildings.iter().all(|b| b.kind == first.kind))
    }

    pub fn producers(&self) -> Vec<BuildingId> {
        self.buildings
            .iter()
            .filter(|b| self.accepts && b.built && !b.stats().produces.is_empty())
            .map(|b| b.id)
            .collect()
    }

    pub fn defenses(&self, domain: Option<oxide_sim::stats::Domain>) -> Vec<BuildingId> {
        self.buildings
            .iter()
            .filter(|b| {
                self.accepts
                    && b.built
                    && b.kind.base_stats().weapons.first().is_some_and(|weapon| {
                        domain.is_none_or(|domain| weapon.targets.covers(domain))
                    })
            })
            .map(|b| b.id)
            .collect()
    }

    pub fn sites(&self) -> Vec<BuildingId> {
        if !self.accepts || !self.homogeneous() {
            return Vec::new();
        }
        self.buildings
            .iter()
            .filter(|b| !b.built && b.tier == 0)
            .map(|b| b.id)
            .collect()
    }

    pub fn upgrade_batch(&self) -> Option<UpgradeBatch> {
        if !self.homogeneous()
            || !self
                .buildings
                .iter()
                .any(|b| b.kind.upgrade_from(b.tier).is_some())
        {
            return None;
        }
        let mut batch = UpgradeBatch {
            recipients: Vec::new(),
            cost: 0,
            reason: None,
        };
        if !self.accepts {
            batch.reason = Some("building orders unavailable".into());
            return Some(batch);
        }
        let mut bank = self.scrap;
        let mut reasons = Vec::new();
        for b in &self.buildings {
            let reason = if !b.built {
                Some("offline".into())
            } else if let Some(upgrade) = b.kind.upgrade_from(b.tier) {
                if let Some(req) = upgrade.requires.iter().find(|req| !self.tech.contains(req)) {
                    Some(format!("needs a standing {}", entity_name(req.name())))
                } else if bank < upgrade.cost {
                    Some(format!("needs {} scrap", upgrade.cost))
                } else {
                    bank -= upgrade.cost;
                    batch.cost += upgrade.cost;
                    batch.recipients.push(b.id);
                    None
                }
            } else {
                Some("max tier".into())
            };
            if let Some(reason) = reason {
                reasons.push(reason);
            }
        }
        reasons.sort();
        reasons.dedup();
        if !reasons.is_empty() {
            batch.reason = Some(reasons.join(", "));
        }
        Some(batch)
    }

    pub fn cards(&self, bindings: &BindingMap) -> Vec<Card> {
        let mut cards = Vec::new();
        let producers = self.producers();
        let plural = self.buildings.len() > 1;
        if !producers.is_empty() {
            let any_rally = self
                .buildings
                .iter()
                .any(|b| producers.contains(&b.id) && b.rally.is_some());
            let noun = if plural { "rallies" } else { "rally" };
            cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Rally),
                title: format!("{} {noun}", if any_rally { "Reset" } else { "Set" }),
                cost: None,
                hotkey: bindings.labels(Action::SetRally),
                action: CardAction::ArmRally,
                enabled: true,
                why: None,
                desc: vec![
                    format!("Set one destination for {} producers.", producers.len()),
                    "A scrap rally sends new Harvesters straight to work.".into(),
                ],
                progress: None,
            });
            cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Rally),
                title: format!("Clear {noun}"),
                cost: None,
                hotkey: bindings.labels(Action::ClearRally),
                action: CardAction::ClearRally,
                enabled: any_rally,
                why: (!any_rally).then(|| "No rally points set.".into()),
                desc: vec!["Return new units to their producer doors.".into()],
                progress: None,
            });
        }
        let defenses = self.defenses(None);
        if !defenses.is_empty() {
            cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Stop),
                title: "Stop".into(),
                cost: None,
                hotkey: bindings.labels(Action::StopOrScrap),
                action: CardAction::Dispatch(Action::StopOrScrap),
                enabled: true,
                why: None,
                desc: vec![format!(
                    "Clear target preference for {} defenses; resume automatic fire.",
                    defenses.len()
                )],
                progress: None,
            });
        }
        let sites = self.sites();
        if !sites.is_empty() {
            cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Cancel),
                title: if plural { format!("Scrap {} sites", sites.len()) } else { "Scrap site".into() },
                cost: None,
                hotkey: if defenses.is_empty() { bindings.labels(Action::StopOrScrap) } else { String::new() },
                action: CardAction::ScrapSites,
                enabled: true,
                why: None,
                desc: vec!["Abandon fresh construction sites; full refund before work starts, partial refund afterward.".into(), "Committed upgrades cannot be cancelled.".into()],
                progress: None,
            });
        }
        // A single upgrading works retains its progress card. Group actions
        // remain available for other members that are still in service.
        if let [b] = self.buildings.as_slice()
            && !b.built
            && b.tier > 0
        {
            cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Cancel),
                title: "Upgrading".into(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::None,
                enabled: false,
                why: Some("upgrades cannot be cancelled".into()),
                desc: vec!["The works returns to service when the upgrade finishes.".into()],
                progress: b
                    .stats()
                    .construction
                    .map(|c| (b.progress as f32 / c.build_ticks.max(1) as f32).clamp(0.0, 1.0)),
            });
        } else if let Some(batch) = self.upgrade_batch() {
            let first = self
                .buildings
                .iter()
                .find(|b| b.kind.upgrade_from(b.tier).is_some())
                .unwrap();
            let mut desc = Vec::new();
            let mut tiers: Vec<_> = self.buildings.iter().map(|b| b.tier).collect();
            tiers.sort_unstable();
            tiers.dedup();
            for tier in tiers {
                if let Some(upgrade) = first.kind.upgrade_from(tier) {
                    desc.push(format!(
                        "{} to {}: {} scrap each. Offline for {} while upgrading.",
                        entity_name(first.kind.tier_name(tier)),
                        entity_name(first.kind.tier_name(tier + 1)),
                        upgrade.cost,
                        tick_time_label(upgrade.build_ticks)
                    ));
                }
            }
            if plural {
                desc.push(format!(
                    "Upgrade {} of {} buildings; {} scrap total.",
                    batch.recipients.len(),
                    self.buildings.len(),
                    batch.cost
                ));
                if let Some(reason) = &batch.reason {
                    desc.push(format!("Skipped: {reason}."));
                }
            }
            cards.push(Card {
                icon: CardIcon::Building(first.kind, first.tier),
                title: if plural {
                    format!(
                        "Upgrade {}/{}",
                        batch.recipients.len(),
                        self.buildings.len()
                    )
                } else {
                    format!(
                        "Upgrade: {}",
                        entity_name(first.kind.tier_name(first.tier + 1))
                    )
                },
                cost: Some(if plural {
                    batch.cost
                } else {
                    first.kind.upgrade_from(first.tier).unwrap().cost
                }),
                hotkey: bindings.labels(Action::Upgrade),
                action: CardAction::Upgrade,
                enabled: !batch.recipients.is_empty(),
                why: batch
                    .recipients
                    .is_empty()
                    .then_some(batch.reason)
                    .flatten(),
                desc,
                progress: None,
            });
        }
        cards
    }
}

pub(crate) fn upgrade(game: &mut Game) {
    let selected = SelectedBuildings::inspect(&game.view());
    let Some(batch) = selected.upgrade_batch() else {
        return;
    };
    let count = batch.recipients.len();
    for building in batch.recipients {
        game.issue(Command::UpgradeBuilding { building });
    }
    if let Some(reason) = batch.reason {
        game.presentation.toast(if selected.buildings.len() > 1 {
            format!(
                "Upgrading {count} of {}: {reason}",
                selected.buildings.len()
            )
        } else {
            reason
        });
    }
}

pub(crate) fn scrap_sites(game: &mut Game) {
    let sites = SelectedBuildings::inspect(&game.view()).sites();
    for &building in &sites {
        game.issue(Command::Cancel { building });
    }
    game.presentation
        .selection
        .buildings
        .retain(|id| !sites.contains(id));
}

pub(crate) fn stop_or_scrap(game: &mut Game) {
    let selected = SelectedBuildings::inspect(&game.view());
    let buildings = selected.defenses(None);
    if !buildings.is_empty() {
        game.issue(Command::ClearFocus { buildings });
    } else if !selected.sites().is_empty() {
        scrap_sites(game);
    } else if selected.buildings.iter().any(|b| !b.built && b.tier > 0) {
        game.presentation.toast("upgrades cannot be cancelled");
    }
}

#[cfg(test)]
pub(crate) mod tests;
