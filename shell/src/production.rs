//! Selected-factory production, including commands staged before the next tick.

use crate::action::{Action, BindingMap};
use crate::game::Game;
use crate::panel::{Card, CardAction, CardIcon, unit_flavor, unit_stat_line, weapon_lines};
use crate::typography::entity_name;
use oxide_sim::{Building, BuildingId, BuildingKind, Command, Faction, UnitKind};

pub(crate) struct Production {
    buildings: Vec<Building>,
    tech: Vec<BuildingKind>,
    scrap: u32,
    faction: Faction,
    accepts: bool,
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
    pub fn inspect(game: &Game) -> Self {
        let snapshot = |buildings: &[Building], scrap, accepts| Self {
            buildings: buildings
                .iter()
                .filter(|b| b.player == game.human && game.selection.buildings.contains(&b.id))
                .cloned()
                .collect(),
            tech: buildings
                .iter()
                .filter(|b| b.player == game.human && b.built)
                .map(|b| b.kind)
                .collect(),
            scrap,
            faction: game.state.player(game.human).faction,
            accepts,
        };
        if game.pending.is_empty() {
            let player = game.state.player(game.human);
            snapshot(
                game.state.buildings(),
                player.scrap,
                game.state.result().is_none() && !player.resigned && game.home_foundry().is_some(),
            )
        } else {
            game.state.inspect_command_phase(&game.pending, |state| {
                snapshot(
                    state.buildings(),
                    state.scrap(game.human).unwrap_or(0),
                    state.accepts_commands(game.human),
                )
            })
        }
    }

    pub fn homogeneous(&self) -> bool {
        self.buildings.first().is_some_and(|first| {
            !first.kind.base_stats().produces.is_empty()
                && self.buildings.iter().all(|b| b.kind == first.kind)
        })
    }

    fn roster(&self, building: &Building) -> impl Iterator<Item = UnitKind> + '_ {
        building
            .kind
            .base_stats()
            .produces
            .iter()
            .copied()
            .filter(|kind| kind.faction().is_none_or(|f| f == self.faction))
    }

    pub fn batch(&self, slot: usize) -> Option<Batch> {
        let homogeneous = self.homogeneous();
        let first = self
            .buildings
            .iter()
            .find(|b| (homogeneous || b.built) && self.roster(b).nth(slot).is_some())?;
        let kind = self.roster(first).nth(slot)?;
        let candidates: Vec<_> = self
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
        if !self.accepts {
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
            .find(|req| !self.tech.contains(req))
        {
            batch.reason = Some(format!("needs a standing {}", entity_name(req.name())));
            return Some(batch);
        }
        let mut bank = self.scrap;
        let mut full = 0;
        let mut offline = 0;
        let mut unfunded = 0;
        for building in candidates {
            if !building.built || !building.stats().produces.contains(&kind) {
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
        let Some(first) = self.buildings.first() else {
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
                        kind.stats().cost * count as u32
                    ));
                    if let Some(reason) = &batch.reason {
                        desc.push(reason.clone());
                    }
                }
                Some(Card {
                    icon: CardIcon::Unit(kind),
                    title: entity_name(kind.name()),
                    cost: Some(kind.stats().cost),
                    hotkey: bindings.labels(Action::TrainSlot(slot as u8)),
                    action: CardAction::Dispatch(Action::TrainSlot(slot as u8)),
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
        self.buildings
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
                                if index == 0 { b.progress } else { 0 },
                                b.id,
                                std::cmp::Reverse(index),
                            ),
                            (b.id, index as u8),
                        )
                    })
            })
            .min_by_key(|(key, _)| *key)
            .map(|(_, target)| target)
    }

    pub fn collective_queue(&self) -> (Vec<Card>, Vec<QueueGroup>) {
        let mut kinds: Vec<_> = self
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
                .buildings
                .iter()
                .map(|b| b.queue.iter().filter(|k| **k == kind).count())
                .sum();
            let active = self
                .buildings
                .iter()
                .filter(|b| b.queue.front() == Some(&kind))
                .count();
            let next_ticks = self
                .buildings
                .iter()
                .filter(|b| b.queue.front() == Some(&kind))
                .map(|b| kind.stats().train_ticks.saturating_sub(b.progress))
                .min();
            let mut desc = vec![
                format!("{active} building; {} waiting.", count - active),
                "Click to cancel one; waiting units first. Full refund.".into(),
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
    let Some(batch) = Production::inspect(game).batch(slot) else {
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
        game.toast(if batch.total > 1 {
            format!("Queued {count} of {}: {reason}", batch.total)
        } else {
            reason
        });
    }
}

pub(crate) fn cancel_one(game: &mut Game, kind: UnitKind) {
    if let Some((building, index)) = Production::inspect(game).cancel_target(kind) {
        game.issue(Command::CancelTrain { building, index });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;
    use oxide_sim::{PlayerCommand, Scenario};

    fn factories(scrap: u32) -> Game {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = scrap;
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Foundry,
            x: 9,
            y: 3,
        });
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        game.selection.buildings = game
            .state
            .buildings()
            .iter()
            .filter(|b| b.player == game.human)
            .map(|b| b.id)
            .collect();
        game
    }

    #[test]
    fn provisional_foundry_does_not_keep_production_or_the_home_target_alive() {
        let mut game = factories(500);
        let home = game.home_foundry().unwrap().clone();
        let worker = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind.stats().harvest.is_some())
            .unwrap()
            .id;
        let mut snapshot = serde_json::to_value(&*game.state).unwrap();
        snapshot["buildings"].as_array_mut().unwrap().retain(|b| {
            b["player"] != serde_json::json!(game.human) || b["id"] == serde_json::json!(home.id)
        });
        let site = snapshot["buildings"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|b| b["id"] == serde_json::json!(home.id))
            .unwrap();
        site["provisional"] = true.into();
        site["built"] = false.into();
        site["hp"] = (home.stats().max_hp / 5).into();
        let unit = snapshot["units"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|u| u["id"] == serde_json::json!(worker))
            .unwrap();
        unit["order"] = serde_json::to_value(oxide_sim::Order::Found {
            kind: home.kind,
            anchor: home.anchor,
        })
        .unwrap();
        *game.state = serde_json::from_value(snapshot).unwrap();
        game.state.validate_invariants().unwrap();
        assert!(game.state.result().is_none());
        assert!(!game.state.player(game.human).resigned);
        assert!(game.home_foundry().is_none());
        assert!(!Production::inspect(&game).accepts);
    }

    #[test]
    fn grouped_production_spends_once_per_factory_in_id_order_and_projects_pending_commands() {
        let mut game = factories(150);
        let ids = game.selection.buildings.clone();
        game.selection.buildings = vec![ids[1], ids[0], ids[1]];
        let before = game.state.hash();
        train(&mut game, 0);
        train(&mut game, 0);
        train(&mut game, 0);
        assert_eq!(
            game.state.hash(),
            before,
            "preview never advances the match"
        );
        let targets: Vec<_> = game
            .pending
            .iter()
            .map(|pc| match pc.command {
                Command::Train { building, .. } => building,
                _ => panic!("ordinary train only"),
            })
            .collect();
        assert_eq!(targets, vec![ids[0], ids[1], ids[0]]);
        assert!(
            game.toasts
                .iter()
                .any(|t| t.text == "Queued 1 of 2: insufficient scrap")
        );
        let (cards, counts) = Production::inspect(&game).collective_queue();
        assert_eq!(cards[0].title, "Harvester x 3");
        assert_eq!((counts[0].count, counts[0].active), (3, 2));
        let events = game.do_tick().events;
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
        );
        assert_eq!(game.state.building(ids[0]).unwrap().queue.len(), 2);
        assert_eq!(game.state.building(ids[1]).unwrap().queue.len(), 1);
    }

    #[test]
    fn grouped_production_skips_full_queues_including_staged_purchases() {
        let mut game = factories(5000);
        let ids = game.selection.buildings.clone();
        for _ in 0..oxide_sim::stats::QUEUE_CAP {
            game.issue(Command::Train {
                building: ids[0],
                kind: UnitKind::Harvester,
            });
        }
        let batch = Production::inspect(&game).batch(0).unwrap();
        assert_eq!(batch.recipients, vec![ids[1]]);
        assert_eq!(batch.reason.as_deref(), Some("1 queue full"));
        for _ in 0..oxide_sim::stats::QUEUE_CAP + 2 {
            train(&mut game, 0);
        }
        assert_eq!(game.pending.len(), oxide_sim::stats::QUEUE_CAP * 2);
        let panel = crate::panel::build_for_palette(&game, &BindingMap::classic(), false).unwrap();
        assert_eq!(panel.queue.len(), 1, "sixteen paid units occupy one tile");
        assert_eq!(panel.queue_groups[0].count, 16);
        assert!(
            panel
                .cards
                .iter()
                .filter(|c| c.cost.is_some())
                .all(|c| !c.enabled)
        );
        assert!(
            !game
                .do_tick()
                .events
                .iter()
                .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
        );
    }

    #[test]
    fn collective_cancellation_preserves_heads_and_repeated_clicks_use_updated_slots() {
        let mut game = factories(150);
        let ids = game.selection.buildings.clone();
        train(&mut game, 0);
        train(&mut game, 0);
        cancel_one(&mut game, UnitKind::Harvester);
        assert!(
            matches!(game.pending.last().unwrap().command, Command::CancelTrain { building, index: 1 } if building == ids[0])
        );
        assert_eq!(Production::inspect(&game).scrap, 50);
        train(&mut game, 1); // 75 scrap must still be refused.
        assert!(matches!(
            game.pending.last().unwrap().command,
            Command::CancelTrain { .. }
        ));
        cancel_one(&mut game, UnitKind::Harvester);
        train(&mut game, 1); // The second refund can now buy one Sentinel.
        assert!(matches!(
            game.pending.last().unwrap().command,
            Command::Train {
                kind: UnitKind::Sentinel,
                ..
            }
        ));
        cancel_one(&mut game, UnitKind::Harvester);
        let len = game.pending.len();
        cancel_one(&mut game, UnitKind::Harvester);
        assert_eq!(
            game.pending.len(),
            len,
            "an exhausted stale tile cancels nothing else"
        );
        assert!(
            !game
                .do_tick()
                .events
                .iter()
                .any(|e| matches!(e, oxide_sim::Event::CommandRejected { .. }))
        );
        assert_eq!(
            game.state.building(ids[0]).unwrap().queue.front(),
            Some(&UnitKind::Sentinel)
        );
        assert!(game.state.building(ids[1]).unwrap().queue.is_empty());
    }

    #[test]
    fn grouped_production_honors_foreign_ownership_tech_and_pending_elimination() {
        let mut game = factories(5000);
        let ids = game.selection.buildings.clone();
        let foreign = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player != game.human)
            .unwrap()
            .id;
        game.selection.buildings = vec![foreign];
        train(&mut game, 0);
        assert!(game.pending.is_empty());
        game.selection.buildings = ids;
        // Excavators require an Array.
        let slot = BuildingKind::Foundry
            .base_stats()
            .produces
            .iter()
            .position(|k| *k == UnitKind::Excavator)
            .unwrap();
        let batch = Production::inspect(&game).batch(slot).unwrap();
        assert!(batch.recipients.is_empty());
        assert!(batch.reason.unwrap().contains("standing"));
        game.issue(Command::Surrender);
        train(&mut game, 0);
        assert_eq!(game.pending.len(), 1);
    }

    #[test]
    fn grouped_production_observes_pending_invalid_purchases_and_refunds_without_charging_twice() {
        let mut game = factories(50);
        let id = game.selection.buildings[0];
        game.stage(PlayerCommand {
            player: game.human,
            command: Command::Train {
                building: id,
                kind: UnitKind::Condor,
            },
        });
        train(&mut game, 0);
        assert_eq!(Production::inspect(&game).scrap, 0);
        assert_eq!(Production::inspect(&game).buildings[0].queue.len(), 1);
        cancel_one(&mut game, UnitKind::Harvester);
        assert_eq!(Production::inspect(&game).scrap, 50);
    }

    #[test]
    fn collective_rosters_fit_the_dock_for_both_factions() {
        for faction in [Faction::Ferrous, Faction::Cupric] {
            for kind in [
                BuildingKind::Foundry,
                BuildingKind::Fabricator,
                BuildingKind::Airworks,
                BuildingKind::Crucible,
            ] {
                let roster: Vec<_> = kind
                    .base_stats()
                    .produces
                    .iter()
                    .filter(|k| k.faction().is_none_or(|f| f == faction))
                    .collect();
                assert!(
                    roster.len() <= 8,
                    "{kind:?} must retain access to every aggregate"
                );
            }
        }
    }
}
