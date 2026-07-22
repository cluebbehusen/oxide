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
        UnitKind::Bombard => "Artillery. Shells past its own eyes — spot for it.",
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
        BuildingKind::Turret => "Static ground defense. Holds a line by standing on it.",
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
        let mut panel = Panel {
            title: building.kind.name().to_uppercase(),
            sub: format!("{}/{} hp", building.hp, stats.max_hp),
            portrait: CardIcon::Building(building.kind),
            cards: Vec::new(),
            queue: Vec::new(),
            queue_label: "queue",
        };
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
        let queue_full = building.queue.len() >= 2;
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
        for (i, &kind) in building.queue.iter().enumerate() {
            panel.queue.push(Card {
                icon: CardIcon::Unit(kind),
                title: kind.name().to_string(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::CancelQueue(building.id, i as u8),
                enabled: true,
                why: None,
                desc: vec!["Click to cancel — full refund.".into()],
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
                ks.dedup();
                ks.iter().map(|k| k.name()).take(4).collect()
            };
            kinds.join(", ")
        },
        portrait: CardIcon::Unit(first.kind),
        cards: Vec::new(),
        queue: Vec::new(),
        queue_label: "orders",
    };
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
            "Legs are attack-moves.".into(),
        ],
    });
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
                desc: vec![
                    building_flavor(kind).to_string(),
                    "Full price on placement; cancel refunds by hp.".into(),
                ],
            });
        }
    }
    // The first unit's program: what it is doing and what comes next.
    panel.queue.push(order_card(&first.order, true));
    for order in first.queue.iter().take(7) {
        panel.queue.push(order_card(order, false));
    }
    Some(panel)
}
