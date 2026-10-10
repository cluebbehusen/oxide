//! Selected-factory production, including commands staged before the next tick.

use crate::action::{Action, BindingMap};
use crate::building_actions::SelectedBuildings;
use crate::game::{Game, Scene};
use crate::numeric::Fit;
use crate::panel::{Card, CardAction, CardIcon, unit_flavor, unit_stat_line, weapon_lines};
use crate::typography::entity_name;
use oxide_sim::{Building, BuildingId, Command, UnitKind};

pub(crate) struct Production {
    selected: SelectedBuildings,
}

pub(crate) struct Batch {
    pub kind: UnitKind,
    pub recipients: Vec<BuildingId>,
    pub total: usize,
    pub reason: Option<String>,
}

/// A collective tile counts paid units, including the heads already building.
pub(crate) struct QueueGroup {
    pub count: usize,
    pub active: usize,
    pub next_ticks: Option<u32>,
}

impl Production {
    pub fn inspect(game: &Scene<'_>) -> Self {
        Self::from_selected(SelectedBuildings::inspect(game))
    }

    pub fn from_selected(selected: SelectedBuildings) -> Self {
        Self { selected }
    }

    pub fn homogeneous(&self) -> bool {
        self.selected.homogeneous()
            && self
                .selected
                .buildings
                .first()
                .is_some_and(|b| !b.kind.base_stats().produces.is_empty())
    }

    fn roster(&self, building: &Building) -> impl Iterator<Item = UnitKind> + '_ {
        building
            .kind
            .base_stats()
            .produces
            .iter()
            .copied()
            .filter(|kind| kind.faction().is_none_or(|f| f == self.selected.faction))
    }

    pub fn batch(&self, slot: usize) -> Option<Batch> {
        let homogeneous = self.homogeneous();
        let first = self
            .selected
            .buildings
            .iter()
            .find(|b| (homogeneous || b.built()) && self.roster(b).nth(slot).is_some())?;
        let kind = self.roster(first).nth(slot)?;
        let candidates: Vec<_> = self
            .selected
            .buildings
            .iter()
            .filter(|b| {
                if homogeneous {
                    b.kind == first.kind
                } else {
                    b.id == first.id
                }
            })
            .collect();
        let mut batch = Batch {
            kind,
            recipients: Vec::new(),
            total: candidates.len(),
            reason: None,
        };
        if !self.selected.accepts {
            batch.reason = Some("production unavailable".into());
            return Some(batch);
        }
        if candidates
            .iter()
            .all(|b| b.queue.len() >= oxide_sim::stats::QUEUE_CAP)
        {
            batch.reason = Some(if batch.total == 1 {
                "queue is full".into()
            } else {
                format!("{} queues full", batch.total)
            });
            return Some(batch);
        }
        if let Some(req) = kind
            .stats()
            .requires
            .iter()
            .find(|req| !self.selected.tech.contains(req))
        {
            batch.reason = Some(format!("needs a standing {}", entity_name(req.name())));
            return Some(batch);
        }
        let mut bank = self.selected.scrap;
        let mut full = 0;
        let mut offline = 0;
        let mut unfunded = 0;
        for building in candidates {
            if !building.built() || !building.stats().produces.contains(&kind) {
                offline += 1;
            } else if building.queue.len() >= oxide_sim::stats::QUEUE_CAP {
                full += 1;
            } else if bank < kind.stats().cost {
                unfunded += 1;
            } else {
                bank -= kind.stats().cost;
                batch.recipients.push(building.id);
            }
        }
        let mut reasons = Vec::new();
        if full > 0 {
            reasons.push(if batch.total == 1 {
                "queue is full".into()
            } else {
                format!("{full} {} full", if full == 1 { "queue" } else { "queues" })
            });
        }
        if offline > 0 {
            reasons.push(format!("{offline} offline"));
        }
        if unfunded > 0 {
            reasons.push(if batch.total == 1 {
                format!("needs {} scrap", kind.stats().cost)
            } else {
                "insufficient scrap".into()
            });
        }
        if !reasons.is_empty() {
            batch.reason = Some(reasons.join(", "));
        }
        Some(batch)
    }

    pub fn cards(&self, bindings: &BindingMap) -> Vec<Card> {
        let Some(first) = self.selected.buildings.first() else {
            return Vec::new();
        };
        self.roster(first)
            .enumerate()
            .filter_map(|(slot, kind)| {
                let batch = self.batch(slot)?;
                let count = batch.recipients.len();
                let mut desc = vec![unit_flavor(kind).into(), unit_stat_line(kind)];
                desc.extend(weapon_lines(kind));
                if batch.total > 1 {
                    desc.push(format!(
                        "One per factory: {count} of {} can queue now.",
                        batch.total
                    ));
                    desc.push(format!(
                        "{} scrap each; {} scrap total.",
                        kind.stats().cost,
                        kind.stats().cost * count.fit::<u32>()
                    ));
                    if let Some(reason) = &batch.reason {
                        desc.push(reason.clone());
                    }
                }
                Some(Card {
                    icon: CardIcon::Unit(kind),
                    title: entity_name(kind.name()),
                    cost: Some(kind.stats().cost),
                    hotkey: bindings.labels(Action::TrainSlot(slot.fit::<u8>())),
                    action: CardAction::Dispatch(Action::TrainSlot(slot.fit::<u8>())),
                    enabled: count > 0,
                    why: (count == 0).then_some(batch.reason).flatten(),
                    desc,
                    progress: None,
                })
            })
            .collect()
    }

    fn cancel_target(&self, kind: UnitKind) -> Option<(BuildingId, u8)> {
        // Preserve active work: take a waiting slot from the back of a
        // factory queue first, then the least-progressed head.
        self.selected
            .buildings
            .iter()
            .flat_map(|b| {
                b.queue
                    .iter()
                    .enumerate()
                    .filter(move |(_, k)| **k == kind)
                    .map(move |(index, _)| {
                        (
                            (
                                index == 0,
                                if index == 0 { b.training_progress() } else { 0 },
                                b.id,
                                std::cmp::Reverse(index),
                            ),
                            (b.id, index.fit::<u8>()),
                        )
                    })
            })
            .min_by_key(|(key, _)| *key)
            .map(|(_, target)| target)
    }

    pub fn collective_queue(&self) -> (Vec<Card>, Vec<QueueGroup>) {
        let mut kinds: Vec<_> = self
            .selected
            .buildings
            .iter()
            .flat_map(|b| b.queue.iter().copied())
            .collect();
        kinds.sort_by_key(|kind| kind.name());
        kinds.dedup();
        let mut cards = Vec::new();
        let mut groups = Vec::new();
        for kind in kinds {
            let count = self
                .selected
                .buildings
                .iter()
                .map(|b| b.queue.iter().filter(|k| **k == kind).count())
                .sum();
            let active = self
                .selected
                .buildings
                .iter()
                .filter(|b| b.queue.front() == Some(&kind))
                .count();
            let next_ticks = self
                .selected
                .buildings
                .iter()
                .filter(|b| b.queue.front() == Some(&kind))
                .map(|b| {
                    kind.stats()
                        .train_ticks
                        .saturating_sub(b.training_progress())
                })
                .min();
            let mut desc = vec![
                format!("{active} building; {} waiting.", count - active),
                format!(
                    "{} to cancel one; waiting units first. Full refund.",
                    crate::platform::tap_or_click_capitalized(crate::platform::hands().touch())
                ),
                "Select one factory to inspect its exact queue.".into(),
            ];
            if let Some(ticks) = next_ticks {
                desc.push(if ticks == 0 {
                    "Ready; waiting for an open exit.".into()
                } else {
                    format!(
                        "Next completion in {}.",
                        crate::panel::tick_time_label(ticks)
                    )
                });
            }
            if let Some((id, index)) = self.cancel_target(kind) {
                let b = self
                    .selected
                    .buildings
                    .iter()
                    .find(|b| b.id == id)
                    .expect("selected cancel target");
                desc.push(format!(
                    "Next cancel: {} at {},{} (slot {}).",
                    entity_name(b.kind.name()),
                    b.anchor.x,
                    b.anchor.y,
                    index + 1
                ));
            }
            cards.push(Card {
                icon: CardIcon::Unit(kind),
                title: format!("{} x {count}", entity_name(kind.name())),
                cost: None,
                hotkey: String::new(),
                action: CardAction::CancelProduction(kind),
                enabled: true,
                why: None,
                desc,
                progress: None,
            });
            groups.push(QueueGroup {
                count,
                active,
                next_ticks,
            });
        }
        (cards, groups)
    }
}

pub(crate) fn train(game: &mut Game, slot: usize) {
    let Some(batch) = Production::inspect(&game.view()).batch(slot) else {
        return;
    };
    let count = batch.recipients.len();
    for building in batch.recipients {
        game.issue(Command::Train {
            building,
            kind: batch.kind,
        });
    }
    if let Some(reason) = batch.reason {
        game.presentation.toast(if batch.total > 1 {
            format!("Queued {count} of {}: {reason}", batch.total)
        } else {
            reason
        });
    }
}

/// Empties every selected producer's queue with full refunds, last job
/// first so each index still names its job when its command runs.
pub(crate) fn cancel_all(game: &mut Game) {
    let selected = SelectedBuildings::inspect(&game.view());
    if !selected.accepts {
        return;
    }
    let jobs: Vec<_> = selected
        .buildings
        .iter()
        .flat_map(|b| {
            (0..b.queue.len())
                .rev()
                .map(move |index| (b.id, index.fit::<u8>()))
        })
        .collect();
    for (building, index) in jobs {
        game.issue(Command::CancelTrain { building, index });
    }
}

pub(crate) fn cancel_one(game: &mut Game, kind: UnitKind) {
    if let Some((building, index)) = Production::inspect(&game.view()).cancel_target(kind) {
        game.issue(Command::CancelTrain { building, index });
    }
}

#[cfg(test)]
mod tests;
