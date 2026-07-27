//! The shared command panel: one HUD grammar for everything selected.
//!
//! Click a building and its cards appear — portrait, production cards
//! with sprites and costs, the queue as cancelable thumbnails. Click a
//! harvester and the *same* panel shows the build palette and its order
//! queue. Every card is a button routed through the exact action its
//! hotkey dispatches (keyboard stays first-class), and hovering any
//! card raises a tooltip: what it is, what it costs, how it fights, and
//! the key that does the same thing.

use crate::action::{Action, BindingMap};
use crate::game::Game;
use oxide_sim::stats::{BuildingKind, UnitKind};
use oxide_sim::{BuildingId, Order};

/// What a card wears.
#[derive(Clone, Copy, PartialEq)]
pub enum CardIcon {
    /// A unit sprite (drawn in the human's faction colors).
    Unit(UnitKind),
    /// A building sprite.
    Building(BuildingKind),
    /// A text glyph — order verbs and other spriteless notions.
    Glyph(&'static str),
}

/// What clicking a card does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardAction {
    /// Route through the action system, exactly like its hotkey.
    Dispatch(Action),
    /// Arm building placement (what the palette digit does).
    ArmBuild(BuildingKind),
    /// Remove a queued unit from a producer (full refund).
    CancelQueue(BuildingId, u8),
    /// Clear a producer's rally point.
    ClearRally(BuildingId),
    /// Display only.
    None,
}

/// One button (or display chip) on the panel.
pub struct Card {
    /// Face of the card.
    pub icon: CardIcon,
    /// Name shown in the tooltip header.
    pub title: String,
    /// Scrap cost, when the card buys something.
    pub cost: Option<u32>,
    /// The hotkey performing the same act, from the live bindings.
    pub hotkey: String,
    /// What clicking does.
    pub action: CardAction,
    /// Whether the card can act right now.
    pub enabled: bool,
    /// Why not, when disabled — surfaced in the tooltip.
    pub why: Option<String>,
    /// Tooltip body: description plus weapon lines.
    pub desc: Vec<String>,
}

/// The panel for the current selection.
pub struct Panel {
    /// Header line: name (and count for multi-selections).
    pub title: String,
    /// Sub-line: hp, status.
    pub sub: String,
    /// Portrait icon.
    pub portrait: CardIcon,
    /// Whose colors the portrait and queue sprites wear — the SELECTED
    /// entity's owner, not the viewer (an inspected Cupric ally must
    /// not draw in Ferrous rust).
    pub faction: oxide_sim::Faction,
    /// Command cards.
    pub cards: Vec<Card>,
    /// Queue thumbnails (production or orders).
    pub queue: Vec<Card>,
    /// What the queue strip is labeled.
    pub queue_label: &'static str,
}

/// One-line flavor per unit kind — tooltip and codex copy.
pub fn unit_flavor(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Harvester => {
            "Hauls scrap, raises buildings, welds repairs. The economy on treads."
        }
        UnitKind::Sentinel => "Line infantry. Holds ground and pokes back at the sky.",
        UnitKind::Scuttler => "Fast raider. Eats undefended harvest lines.",
        UnitKind::Lancer => "Rail sniper. Outranges turrets; melts if reached.",
        UnitKind::Bombard => "Artillery. Fires beyond its sight; needs a spotter.",
        UnitKind::Flakhound => "Tracked anti-air. The sky answers to it.",
        UnitKind::Buzzard => "Heavy ground-attack flyer.",
        UnitKind::Talon => "Air-superiority hunter.",
        UnitKind::Stinger => "Cheap anti-air crawler.",
        UnitKind::Darter => "Darting ground-attack flyer.",
        UnitKind::Wisp => "Swarm interceptor. Owns no ground, only sky.",
    }
}

/// One-line flavor per building kind.
pub fn building_flavor(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::Foundry => "Trains the basics. Lose every Foundry and the seat falls.",
        BuildingKind::Fabricator => "Unlocks the advanced roster and the air wing.",
        BuildingKind::Turret => {
            "Static ground defense. Holds a line by standing on it - the answer to a swarm."
        }
        BuildingKind::FlakTurret => "Static anti-air. The roof over your harvest line.",
        BuildingKind::Bastion => "Siege gun emplacement. Arcs shells beyond its sight.",
        BuildingKind::Array => "Radar mast. True sight close, nameless contacts far.",
        BuildingKind::Reclaimer => "Slow-drips scrap from the ground it stands on.",
    }
}

/// Human lines for a kind's weapons, from the stats table.
pub fn weapon_lines(kind: UnitKind) -> Vec<String> {
    let stats = kind.stats();
    stats
        .weapons
        .iter()
        .map(|w| {
            let targets = match (
                w.targets.covers(oxide_sim::stats::Domain::Ground),
                w.targets.covers(oxide_sim::stats::Domain::Air),
            ) {
                (true, true) => "ground+air",
                (true, false) => "ground",
                (false, true) => "air",
                (false, false) => "nothing",
            };
            let flavor = if w.projectile {
                ", live shell"
            } else if w.indirect {
                ", indirect"
            } else {
                ""
            };
            let splash = if w.splash.is_some() { ", splash" } else { "" };
            format!(
                "{} dmg / {:.1} range vs {targets}{flavor}{splash}",
                w.damage,
                w.range.to_num::<f32>(),
            )
        })
        .collect()
}

fn order_card(order: &Order, active: bool) -> Card {
    let (glyph, title, desc): (&str, &str, &str) = match order {
        Order::Idle => (
            "·",
            "Idle",
            "Standing by; fighters auto-engage in aggro range.",
        ),
        Order::Move { .. } => ("M", "Move", "Walking; oblivious to enemies on the way."),
        Order::Harvest { .. } => ("H", "Harvest", "Working a scrap node, hauling home."),
        Order::Attack { .. } => ("A", "Attack", "Chasing one target until it is gone."),
        Order::Build { .. } => ("B", "Build", "Standing up a construction site."),
        Order::Repair { .. } => (
            "R",
            "Repair",
            "Welding a damaged building; costs a trickle.",
        ),
        Order::AttackMove { .. } => ("X", "Attack-move", "Marching; engages everything met."),
        Order::Salvage { .. } => (
            "S",
            "Salvage",
            "Stripping a building down for a partial refund.",
        ),
    };
    Card {
        icon: CardIcon::Glyph(glyph),
        title: if active {
            format!("{title} (now)")
        } else {
            title.to_string()
        },
        cost: None,
        hotkey: String::new(),
        action: CardAction::None,
        enabled: true,
        why: None,
        desc: vec![desc.to_string()],
    }
}

fn chord(bindings: &BindingMap, action: Action) -> String {
    bindings
        .chord_for(action)
        .map(BindingMap::chord_label)
        .unwrap_or_default()
}

/// Builds the panel for the current selection, or `None` when nothing
/// warrants one. Pure: reads game + bindings, owns no state.
pub fn build(game: &Game, bindings: &BindingMap) -> Option<Panel> {
    let faction = game.state.player(game.human).faction;
    if let Some(id) = game.selection.building {
        let building = game.state.building(id)?;
        let stats = building.kind.stats();
        let owner = building.player;
        let mut panel = Panel {
            title: building.kind.name().to_uppercase(),
            sub: format!("{}/{} hp", building.hp, stats.max_hp),
            portrait: CardIcon::Building(building.kind),
            faction: game.state.player(owner).faction,
            cards: Vec::new(),
            queue: Vec::new(),
            queue_label: "queue",
        };
        if owner != game.human {
            // Foreign buildings inspect read-only: an ally's works say
            // whose they are; a hostile shows hp and kind, nothing
            // more — no queue chips, no cards, no rally, no reach
            // into anyone's production.
            let hostile = game.state.hostile(game.human, owner);
            panel.sub = format!(
                "{} · {}",
                if hostile { "hostile" } else { "ally" },
                panel.sub
            );
            if !hostile {
                panel.cards.push(Card {
                    icon: CardIcon::Glyph("·"),
                    title: "Ally works".into(),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::None,
                    enabled: true,
                    why: None,
                    desc: vec![
                        "Read-only: allies coordinate by position,".into(),
                        "not by each other's controls.".into(),
                    ],
                });
            }
            return Some(panel);
        }
        if !building.built {
            panel.sub = format!("under construction · {}", panel.sub);
            panel.cards.push(Card {
                icon: CardIcon::Glyph("X"),
                title: "Scrap site".into(),
                cost: None,
                hotkey: chord(bindings, Action::StopOrScrap),
                action: CardAction::Dispatch(Action::StopOrScrap),
                enabled: true,
                why: None,
                desc: vec!["Abandon the site for a partial refund.".into()],
            });
            return Some(panel);
        }
        let scrap = game.state.player(game.human).scrap;
        let queue_full = building.queue.len() >= oxide_sim::stats::QUEUE_CAP;
        for (i, &kind) in stats
            .produces
            .iter()
            .filter(|k| k.faction().is_none_or(|f| f == faction))
            .enumerate()
        {
            let cost = kind.stats().cost;
            let (enabled, why) = if queue_full {
                (false, Some("queue is full".to_string()))
            } else if scrap < cost {
                (false, Some(format!("needs {cost} scrap")))
            } else {
                (true, None)
            };
            let mut desc = vec![unit_flavor(kind).to_string()];
            desc.extend(weapon_lines(kind));
            panel.cards.push(Card {
                icon: CardIcon::Unit(kind),
                title: kind.name().to_string(),
                cost: Some(cost),
                hotkey: format!("{}", i + 1),
                action: CardAction::Dispatch(Action::TrainSlot(i as u8)),
                enabled,
                why,
                desc,
            });
        }
        if building.rally.is_some() {
            panel.cards.push(Card {
                icon: CardIcon::Glyph("R"),
                title: "Clear rally".into(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::ClearRally(building.id),
                enabled: true,
                why: None,
                desc: vec!["Fresh units gather at the doorstep again.".into()],
            });
        }
        for (i, &kind) in building.queue.iter().enumerate() {
            panel.queue.push(Card {
                icon: CardIcon::Unit(kind),
                title: kind.name().to_string(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::CancelQueue(building.id, i as u8),
                enabled: true,
                why: None,
                desc: vec!["Click to cancel; full refund.".into()],
            });
        }
        return Some(panel);
    }
    if game.selection.units.is_empty() {
        return None;
    }
    let units: Vec<_> = game
        .selection
        .units
        .iter()
        .filter_map(|id| game.state.unit(*id))
        .collect();
    let first = units.first()?;
    let owner = first.player;
    let has_builder = units.iter().any(|u| u.kind == UnitKind::Harvester);
    let mut desc = vec![unit_flavor(first.kind).to_string()];
    desc.extend(weapon_lines(first.kind));
    let mut panel = Panel {
        title: if units.len() == 1 {
            first.kind.name().to_uppercase()
        } else {
            format!("{} UNITS", units.len())
        },
        sub: if units.len() == 1 {
            format!("{}/{} hp", first.hp, first.kind.stats().max_hp)
        } else {
            let kinds: Vec<&str> = {
                let mut ks: Vec<UnitKind> = units.iter().map(|u| u.kind).collect();
                // dedup only folds neighbors, and a mixed selection
                // arrives in id order where equal kinds need not be.
                ks.sort_by_key(|k| k.name());
                ks.dedup();
                ks.iter().map(|k| k.name()).take(4).collect()
            };
            kinds.join(", ")
        },
        portrait: CardIcon::Unit(first.kind),
        faction: game.state.player(owner).faction,
        cards: Vec::new(),
        queue: Vec::new(),
        queue_label: "orders",
    };
    if owner != game.human {
        // Foreign units inspect read-only. An ally shows its orders —
        // that was the ask: see what your teammate is doing — while a
        // hostile shows hp and kind only: its order state is intent the
        // fog never licensed (zero chips, a test pins it).
        let hostile = game.state.hostile(game.human, owner);
        panel.sub = format!(
            "{} · {}",
            if hostile { "hostile" } else { "ally" },
            panel.sub
        );
        if !hostile && units.len() == 1 {
            panel.queue.push(order_card(&first.order, true));
            for order in &first.queue {
                panel.queue.push(order_card(order, false));
            }
        }
        return Some(panel);
    }
    panel.cards.push(Card {
        icon: CardIcon::Glyph("■"),
        title: "Stop".into(),
        cost: None,
        hotkey: chord(bindings, Action::StopOrScrap),
        action: CardAction::Dispatch(Action::StopOrScrap),
        enabled: true,
        why: None,
        desc: vec!["Clear orders; stand and auto-engage.".into()],
    });
    panel.cards.push(Card {
        icon: CardIcon::Glyph("P"),
        title: "Patrol".into(),
        cost: None,
        hotkey: chord(bindings, Action::Patrol),
        action: CardAction::Dispatch(Action::Patrol),
        enabled: true,
        why: None,
        desc: vec![
            "Arm a looping route; press again to start it.".into(),
            "Machines engage whatever they meet along the way.".into(),
        ],
    });
    if has_builder {
        panel.cards.push(Card {
            icon: CardIcon::Glyph("V"),
            title: "Salvage".into(),
            cost: None,
            hotkey: chord(bindings, Action::Salvage),
            action: CardAction::Dispatch(Action::Salvage),
            enabled: true,
            why: None,
            desc: vec![
                "Arm, then click an own built building to strip it".into(),
                "for a partial refund. Foundries refuse.".into(),
            ],
        });
    }
    if has_builder {
        let scrap = game.state.player(game.human).scrap;
        let palette_key = chord(bindings, Action::ToggleBuildPalette);
        for (i, &kind) in crate::input::BUILD_PALETTE.iter().enumerate() {
            let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
            let (enabled, why) = if scrap < cost {
                (false, Some(format!("needs {cost} scrap")))
            } else {
                (true, None)
            };
            panel.cards.push(Card {
                icon: CardIcon::Building(kind),
                title: kind.name().to_string(),
                cost: Some(cost),
                hotkey: format!("{palette_key},{}", i + 1),
                action: CardAction::ArmBuild(kind),
                enabled,
                why,
                desc: vec![building_flavor(kind).to_string()],
            });
        }
    }
    // The first unit's program: what it is doing and what comes next.
    // An idle unit with nothing queued contributes no chips, so the
    // orders dock vanishes instead of showing a lone "Idle" cell.
    if !matches!(first.order, Order::Idle) || !first.queue.is_empty() {
        panel.queue.push(order_card(&first.order, true));
        for order in first.queue.iter().take(7) {
            panel.queue.push(order_card(order, false));
        }
    }
    Some(panel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;
    use oxide_sim::{Command, PlayerCommand, Scenario};

    fn game() -> Game {
        Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 800.0)).expect("skirmish builds")
    }

    fn human_foundry(game: &Game) -> oxide_sim::BuildingId {
        game.state
            .buildings()
            .iter()
            .find(|b| b.player == game.human)
            .expect("human foundry")
            .id
    }

    #[test]
    fn nothing_selected_builds_no_panel() {
        let game = game();
        assert!(build(&game, &BindingMap::classic()).is_none());
    }

    #[test]
    fn the_foundry_panel_speaks_its_roster() {
        let mut game = game();
        game.selection.building = Some(human_foundry(&game));
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert_eq!(panel.title, "FOUNDRY");
        assert_eq!(panel.cards.len(), 2, "harvester and sentinel");
        assert_eq!(panel.cards[0].hotkey, "1");
        assert_eq!(panel.cards[0].cost, Some(50));
        assert!(panel.cards[0].enabled, "150 scrap affords a harvester");
        assert_eq!(
            panel.cards[0].action,
            CardAction::Dispatch(Action::TrainSlot(0)),
            "the card IS its hotkey"
        );
        assert!(panel.queue.is_empty(), "nothing queued yet");
        // The harvester is unarmed — its card carries no weapon line;
        // the sentinel's carries both of its guns.
        assert!(!panel.cards[0].desc.iter().any(|l| l.contains("dmg")));
        assert!(panel.cards[1].desc.iter().any(|l| l.contains("dmg")));
    }

    #[test]
    fn poverty_and_capacity_disable_cards_with_reasons() {
        let mut scenario = Scenario::skirmish();
        // The bank must outlast the queue cap or poverty masks it.
        scenario.players[0].scrap = 500;
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("skirmish builds");
        let foundry = human_foundry(&game);
        game.selection.building = Some(foundry);
        // Queue harvesters until 50 scrap remains: the sentinel card
        // (75) must dim with the price named.
        for _ in 0..3 {
            game.state.tick(&[PlayerCommand {
                player: game.human,
                command: Command::Train {
                    building: foundry,
                    kind: oxide_sim::UnitKind::Harvester,
                },
            }]);
        }
        // 500 - 3x50 = 350: still rich, cards enabled, ghosts armed.
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert!(panel.cards[1].enabled);
        assert_eq!(panel.queue.len(), 3);
        assert_eq!(panel.queue[1].action, CardAction::CancelQueue(foundry, 1));
        // Fill to the sim's cap: every production card refuses.
        for _ in 0..oxide_sim::stats::QUEUE_CAP {
            game.state.tick(&[PlayerCommand {
                player: game.human,
                command: Command::Train {
                    building: foundry,
                    kind: oxide_sim::UnitKind::Harvester,
                },
            }]);
        }
        let queued = game.state.building(foundry).unwrap().queue.len();
        assert_eq!(queued, oxide_sim::stats::QUEUE_CAP, "the sim capped it");
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert!(panel.cards.iter().all(|c| !c.enabled));
        assert!(
            panel
                .cards
                .iter()
                .all(|c| c.why.as_deref() == Some("queue is full")),
            "the reason names the cap, not the bank"
        );
    }

    #[test]
    fn the_harvester_panel_is_the_same_grammar() {
        let mut game = game();
        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == oxide_sim::UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        game.selection.units = vec![harvester];
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert_eq!(panel.title, "HARVESTER");
        assert_eq!(panel.cards[0].title, "Stop");
        assert_eq!(panel.cards[1].title, "Patrol");
        let builds: Vec<_> = panel
            .cards
            .iter()
            .filter(|c| matches!(c.action, CardAction::ArmBuild(_)))
            .collect();
        assert_eq!(builds.len(), crate::input::BUILD_PALETTE.len());
        assert!(
            builds[0].hotkey.starts_with("B,"),
            "palette cards teach their chord"
        );
        // An idle unit with nothing queued shows no order chips at all —
        // the dock only exists when there is a program to show.
        assert!(panel.queue.is_empty(), "idle shows no dock");
        // Give it a program: the strip appears, and stays display-only.
        game.state.tick(&[oxide_sim::PlayerCommand {
            player: game.human,
            command: oxide_sim::Command::AttackMove {
                units: vec![harvester],
                goal: chassis::grid::TilePos::new(8, 8),
                queue: false,
            },
        }]);
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert_eq!(panel.queue.len(), 1);
        assert_eq!(
            panel.queue[0].action,
            CardAction::None,
            "orders are display-only"
        );
    }

    #[test]
    fn weapon_lines_read_from_the_stats_table() {
        let sentinel = weapon_lines(oxide_sim::UnitKind::Sentinel);
        assert_eq!(sentinel.len(), 2, "main gun and the anti-air poke");
        assert!(sentinel[0].contains("vs ground"));
        assert!(sentinel[1].contains("vs air"));
        let bombard = weapon_lines(oxide_sim::UnitKind::Bombard);
        assert!(bombard[0].contains("live shell"));
        assert!(bombard[0].contains("splash"));
    }
}
