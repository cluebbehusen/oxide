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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CardIcon {
    /// A unit sprite (drawn in the human's faction colors).
    Unit(UnitKind),
    /// A building sprite.
    Building(BuildingKind),
    /// A verb pictogram from the atlas's icon family.
    Verb(VerbIcon),
    /// An order chip that knows what it acts on: the subject's own
    /// sprite under a corner verb badge. Orders with no subject
    /// (Idle, Move, Attack-move, Harvest) stay plain [`CardIcon::Verb`].
    Order {
        /// The machine or works the verb acts on.
        subject: OrderSubject,
        /// The verb, worn as a badge rather than the whole face.
        verb: VerbIcon,
        /// An unfinished site: translucent hull plus scaffold, the
        /// same language the world draws it in.
        ghost: bool,
    },
}

/// What an order chip is ABOUT, with the colors that subject actually
/// wears — an attack victim is not the panel owner's faction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderSubject {
    /// A machine: an attack victim.
    Unit(UnitKind, oxide_sim::Faction),
    /// Works: a site being raised, a patient, a strip job, a victim.
    Building(BuildingKind, oxide_sim::Faction),
}

/// The atlas's verb pictograms, in `Sprites::verb_icons` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbIcon {
    /// Everything halts.
    Stop,
    /// The oblivious walk.
    Move,
    /// The fighting march.
    AttackMove,
    /// The strike burst.
    Attack,
    /// The loop.
    Patrol,
    /// The scrap pyramid.
    Harvest,
    /// The wrench.
    Build,
    /// The weld.
    Repair,
    /// Value coming back down.
    Salvage,
    /// The refusal cross.
    Cancel,
    /// The rally pennant.
    Rally,
    /// The three-beat wait.
    Idle,
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
    /// Narrow the selection to one kind (Ctrl-click removes it
    /// instead) — the mixed-army type strip.
    FilterKind(UnitKind),
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
    /// How far along the card's job is, 0-1, when it has one — the
    /// production head's bar and an order chip's own meter. The
    /// renderer draws this and never reaches back into the state for
    /// it; the panel model is the one description of the panel.
    pub progress: Option<f32>,
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
    /// Static weapon facts for a singly selected unit. These are drawn
    /// without a hover and deliberately contain no order, target, or
    /// cooldown state, so inspecting a visible enemy reveals capability
    /// without revealing intent.
    pub combat: Vec<String>,
    /// Command cards.
    pub cards: Vec<Card>,
    /// Queue thumbnails (production or orders).
    pub queue: Vec<Card>,
    /// What the queue strip is labeled — for order docks, WHOSE
    /// program it shows ("orders - harvester"), because the dock draws
    /// one unit's story while breadcrumbs draw many.
    pub queue_label: String,
}

/// The selection's SUBJECT: the unit whose program the dock, the
/// portrait, and the full-opacity breadcrumbs all describe — one rule,
/// so the surfaces can never disagree. Majority kind first (a mixed
/// army reads as its bulk, not its lowest id), lowest id inside it as
/// the deterministic tie-break.
pub fn subject_unit(game: &Game) -> Option<oxide_sim::UnitId> {
    let units: Vec<_> = game
        .selection
        .units
        .iter()
        .filter_map(|id| game.state.unit(*id))
        .collect();
    let mut counts: Vec<(UnitKind, usize)> = Vec::new();
    for u in &units {
        match counts.iter_mut().find(|(k, _)| *k == u.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((u.kind, 1)),
        }
    }
    let (majority, _) = counts
        .into_iter()
        .max_by_key(|&(k, n)| (n, std::cmp::Reverse(k.name())))?;
    units
        .iter()
        .filter(|u| u.kind == majority)
        .map(|u| u.id)
        .min()
}

/// One-line flavor per unit kind — tooltip and codex copy.
pub fn unit_flavor(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Harvester => "Collects scrap, constructs buildings, and repairs units.",
        UnitKind::Sentinel => "General-purpose unit with ground and anti-air weapons.",
        UnitKind::Scuttler => "Fast ground raider effective against exposed Harvesters.",
        UnitKind::Lancer => "Long-range ground sniper; vulnerable at close range.",
        UnitKind::Bombard => "Long-range artillery that needs allied vision to fire.",
        UnitKind::Flakhound => "Tracked anti-air unit.",
        UnitKind::Buzzard => "Heavy aircraft that attacks ground targets.",
        UnitKind::Talon => "Heavy air-superiority fighter.",
        UnitKind::Stinger => "Low-cost ground anti-air unit.",
        UnitKind::Darter => "Fast aircraft that attacks ground targets.",
        UnitKind::Wisp => "Fast interceptor that attacks air targets only.",
    }
}

/// One-line flavor per building kind.
pub fn building_flavor(kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::Foundry => {
            "Trains Harvesters and Sentinels. Losing all Foundries eliminates you."
        }
        BuildingKind::Fabricator => "Unlocks advanced ground units and aircraft.",
        BuildingKind::Turret => "Static defense that attacks ground units.",
        BuildingKind::FlakTurret => "Static defense that attacks aircraft.",
        BuildingKind::Bastion => "Long-range artillery emplacement that needs allied vision.",
        BuildingKind::Array => {
            "Reveals terrain within 9 tiles and detects hostile units within 16."
        }
        BuildingKind::Reclaimer => "Generates 1 scrap every 1.5 seconds.",
        BuildingKind::RepairBay => {
            "Automatically repairs friendly ground units within 4 tiles. Repairs consume scrap."
        }
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
                (true, true) => "ground and air",
                (true, false) => "ground",
                (false, true) => "air",
                (false, false) => "nothing",
            };
            let flavor = if w.projectile {
                " · projectile"
            } else if w.indirect {
                " · indirect"
            } else {
                ""
            };
            let splash = if w.splash.is_some() { " · splash" } else { "" };
            format!(
                "{} damage · {:.1} range · targets {targets}{flavor}{splash}",
                w.damage,
                w.range.to_num::<f32>(),
            )
        })
        .collect()
}

/// Always-visible combat facts for a selected unit.
pub fn combat_lines(kind: UnitKind) -> Vec<String> {
    let lines = weapon_lines(kind);
    if lines.is_empty() {
        vec!["unarmed".to_string()]
    } else {
        lines
    }
}

/// The subject an order chip may show, plus the lines that name it —
/// OWN programs only. An ally's chips stay bare pictograms rather than
/// resting the panel on a claim about what team sight shares, and an
/// attack victim resolves through the breadcrumbs' own fog gate, so
/// the chip and the trail can never tell different stories.
fn order_subject(game: &Game, order: &Order) -> Option<(OrderSubject, String, bool, Option<f32>)> {
    let faction_of = |p| game.state.player(p).faction;
    match order {
        Order::Build { site } => {
            let b = game.state.building(*site)?;
            let ticks = b
                .kind
                .stats()
                .construction
                .map(|c| c.build_ticks)
                .unwrap_or(1)
                .max(1);
            let frac = (b.progress as f32 / ticks as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Building(b.kind, faction_of(b.player)),
                b.kind.name().to_string(),
                !b.built,
                Some(frac),
            ))
        }
        Order::Repair { building } | Order::Salvage { building } => {
            let b = game.state.building(*building)?;
            let frac = (b.hp as f32 / b.kind.stats().max_hp.max(1) as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Building(b.kind, faction_of(b.player)),
                b.kind.name().to_string(),
                !b.built,
                Some(frac),
            ))
        }
        Order::Attack { target, .. } => {
            // The same gate the breadcrumb chase point uses: a victim
            // back in the fog is named by neither surface.
            let (subject, name, tile) = match target {
                oxide_sim::Target::Unit(uid) => {
                    let u = game.state.unit(*uid)?;
                    (
                        OrderSubject::Unit(u.kind, faction_of(u.player)),
                        u.kind.name().to_string(),
                        u.tile(),
                    )
                }
                oxide_sim::Target::Building(bid) => {
                    let b = game.state.building(*bid)?;
                    (
                        OrderSubject::Building(b.kind, faction_of(b.player)),
                        b.kind.name().to_string(),
                        b.anchor,
                    )
                }
            };
            (game.all_seeing() || game.my_vision().visible(tile))
                .then_some((subject, name, false, None))
        }
        // A weld patient is own by construction — no fog gate needed,
        // and its meter is the wound closing.
        Order::RepairUnit { unit } => {
            let u = game.state.unit(*unit)?;
            let frac = (u.hp as f32 / u.kind.stats().max_hp.max(1) as f32).clamp(0.0, 1.0);
            Some((
                OrderSubject::Unit(u.kind, faction_of(u.player)),
                u.kind.name().to_string(),
                false,
                Some(frac),
            ))
        }
        // A pending found's subject is the kind it will claim — drawn as
        // a ghost, since nothing stands yet.
        Order::Found { kind, .. } => Some((
            OrderSubject::Building(*kind, faction_of(game.human)),
            kind.name().to_string(),
            true,
            None,
        )),
        Order::Idle | Order::Move { .. } | Order::Harvest { .. } | Order::AttackMove { .. } => None,
    }
}

fn order_card(game: &Game, order: &Order, active: bool, own: bool) -> Card {
    let (icon, title, desc): (VerbIcon, &str, &str) = match order {
        Order::Idle => (
            VerbIcon::Idle,
            "Idle",
            "Idle; armed units attack nearby enemies automatically.",
        ),
        Order::Move { .. } => (VerbIcon::Move, "Move", "Moving without engaging enemies."),
        Order::Harvest { .. } => (
            VerbIcon::Harvest,
            "Harvest",
            "Collecting scrap and returning it to a Foundry.",
        ),
        Order::Attack { .. } => (
            VerbIcon::Attack,
            "Attack",
            "Chasing one target until it is gone.",
        ),
        Order::Build { .. } => (
            VerbIcon::Build,
            "Build",
            "Constructing the selected building.",
        ),
        Order::Repair { .. } => (
            VerbIcon::Repair,
            "Repair",
            "Repairing a damaged building; consumes scrap.",
        ),
        Order::AttackMove { .. } => (
            VerbIcon::AttackMove,
            "Attack-move",
            "Moving while engaging enemies along the route.",
        ),
        Order::Salvage { .. } => (
            VerbIcon::Salvage,
            "Salvage",
            "Stripping a building down for a partial refund.",
        ),
        Order::Found { .. } => (
            VerbIcon::Build,
            "Found",
            "Moving to the build site. Scrap is charged when construction begins.",
        ),
        Order::RepairUnit { .. } => (
            VerbIcon::Repair,
            "Weld",
            "Repairing a damaged unit; consumes scrap.",
        ),
    };
    let subject = if own {
        order_subject(game, order)
    } else {
        None
    };
    let mut desc = vec![desc.to_string()];
    let (face, head, progress) = match subject {
        Some((subject, name, ghost, progress)) => {
            if let Some(line) = subject_detail(game, order, progress) {
                desc.push(line);
            }
            (
                CardIcon::Order {
                    subject,
                    verb: icon,
                    ghost,
                },
                format!("{title} - {name}"),
                progress,
            )
        }
        None => (CardIcon::Verb(icon), title.to_string(), None),
    };
    Card {
        icon: face,
        title: if active {
            format!("{head} (now)")
        } else {
            head
        },
        cost: None,
        hotkey: String::new(),
        action: CardAction::None,
        enabled: true,
        why: None,
        desc,
        progress,
    }
}

/// The concrete second tooltip line for a subject-bearing order: how
/// far the job has come, in the units the verb is actually measured in.
fn subject_detail(game: &Game, order: &Order, progress: Option<f32>) -> Option<String> {
    let pct = |f: f32| (f * 100.0).round() as u32;
    match order {
        Order::Build { .. } => Some(format!("{}% raised", pct(progress?))),
        Order::Repair { building } => {
            let b = game.state.building(*building)?;
            Some(format!("{}/{} hp", b.hp, b.kind.stats().max_hp))
        }
        Order::Salvage { building } => {
            let b = game.state.building(*building)?;
            let cost = b.kind.stats().construction.map(|c| c.cost).unwrap_or(0);
            let left = u64::from(cost) * oxide_sim::stats::SALVAGE_REFUND_PERMILLE / 1000
                * u64::from(b.hp)
                / u64::from(b.kind.stats().max_hp.max(1));
            Some(format!(
                "{}/{} hp · ~{left} scrap left",
                b.hp,
                b.kind.stats().max_hp
            ))
        }
        Order::RepairUnit { unit } => {
            let u = game.state.unit(*unit)?;
            Some(format!("{}/{} hp", u.hp, u.kind.stats().max_hp))
        }
        _ => None,
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
            combat: Vec::new(),
            cards: Vec::new(),
            queue: Vec::new(),
            queue_label: "queue".to_string(),
        };
        if owner != game.human {
            // Foreign buildings inspect read-only: an allied building says
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
                    icon: CardIcon::Verb(VerbIcon::Idle),
                    title: "Ally building".into(),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::None,
                    enabled: true,
                    why: None,
                    desc: vec!["Read-only: allied buildings cannot be controlled.".into()],
                    progress: None,
                });
            }
            return Some(panel);
        }
        if !building.built {
            panel.sub = format!("under construction · {}", panel.sub);
            panel.cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Cancel),
                title: "Scrap site".into(),
                cost: None,
                hotkey: chord(bindings, Action::StopOrScrap),
                action: CardAction::Dispatch(Action::StopOrScrap),
                enabled: true,
                why: None,
                desc: vec!["Abandon the site for a partial refund.".into()],
                progress: None,
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
                progress: None,
            });
        }
        if building.rally.is_some() {
            panel.cards.push(Card {
                icon: CardIcon::Verb(VerbIcon::Rally),
                title: "Clear rally".into(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::ClearRally(building.id),
                enabled: true,
                why: None,
                desc: vec!["New units will remain near the producer.".into()],
                progress: None,
            });
        }
        for (i, &kind) in building.queue.iter().enumerate() {
            // Only the head is being worked; the rest are prepaid ghosts.
            let progress = (i == 0).then(|| {
                let total = kind.stats().train_ticks.max(1);
                (building.progress as f32 / total as f32).clamp(0.0, 1.0)
            });
            panel.queue.push(Card {
                icon: CardIcon::Unit(kind),
                title: kind.name().to_string(),
                cost: None,
                hotkey: String::new(),
                action: CardAction::CancelQueue(building.id, i as u8),
                enabled: true,
                why: None,
                desc: vec!["Click to cancel; full refund.".into()],
                progress,
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
    let subject_id = subject_unit(game)?;
    let first = units.iter().find(|u| u.id == subject_id)?;
    let owner = first.player;
    let has_builder = units.iter().any(|u| u.kind == UnitKind::Harvester);
    let mut panel = Panel {
        title: if units.len() == 1 {
            first.kind.name().to_uppercase()
        } else {
            format!("{} UNITS", units.len())
        },
        sub: if units.len() == 1 {
            format!("{}/{} hp", first.hp, first.kind.stats().max_hp)
        } else {
            let (kinds, extra) = {
                let mut ks: Vec<UnitKind> = units.iter().map(|u| u.kind).collect();
                // dedup only folds neighbors, and a mixed selection
                // arrives in id order where equal kinds need not be.
                ks.sort_by_key(|k| k.name());
                ks.dedup();
                let extra = ks.len().saturating_sub(4);
                let named: Vec<&str> = ks.iter().map(|k| k.name()).take(4).collect();
                (named, extra)
            };
            if extra > 0 {
                format!("{} +{extra} more", kinds.join(", "))
            } else {
                kinds.join(", ")
            }
        },
        portrait: CardIcon::Unit(first.kind),
        faction: game.state.player(owner).faction,
        combat: if units.len() == 1 {
            combat_lines(first.kind)
        } else {
            Vec::new()
        },
        cards: Vec::new(),
        queue: Vec::new(),
        queue_label: if units.len() == 1 {
            "orders".to_string()
        } else {
            // The dock shows ONE unit's program; say whose.
            format!("orders - {}", first.kind.name())
        },
    };
    if owner != game.human {
        // Foreign units inspect read-only. Static weapon facts are safe
        // for any visible unit. An ally also shows its orders, while a
        // hostile's order state remains hidden because it reveals intent.
        let hostile = game.state.hostile(game.human, owner);
        panel.sub = format!(
            "{} · {}",
            if hostile { "hostile" } else { "ally" },
            panel.sub
        );
        if !hostile && units.len() == 1 {
            panel
                .queue
                .push(order_card(game, &first.order, true, false));
            for order in &first.queue {
                panel.queue.push(order_card(game, order, false, false));
            }
        }
        return Some(panel);
    }
    // The type strip: a mixed army offers one card per kind, counted.
    // Click keeps only that kind; Ctrl-click drops it — the two cuts
    // every RTS hand knows. Capped at six cards so the band's card
    // budget holds; the sub line's "+N more" names the fold.
    if units.len() > 1 {
        let mut counts: Vec<(UnitKind, usize)> = Vec::new();
        for u in &units {
            match counts.iter_mut().find(|(k, _)| *k == u.kind) {
                Some((_, n)) => *n += 1,
                None => counts.push((u.kind, 1)),
            }
        }
        if counts.len() > 1 {
            counts.sort_by_key(|(k, _)| k.name());
            for (kind, n) in counts.into_iter().take(6) {
                panel.cards.push(Card {
                    icon: CardIcon::Unit(kind),
                    title: format!("{} x{n}", kind.name()),
                    cost: None,
                    hotkey: String::new(),
                    action: CardAction::FilterKind(kind),
                    enabled: true,
                    why: None,
                    desc: vec![
                        "Click: keep only this kind.".into(),
                        "Ctrl-click: drop this kind instead.".into(),
                    ],
                    progress: None,
                });
            }
        }
    }
    panel.cards.push(Card {
        icon: CardIcon::Verb(VerbIcon::Stop),
        title: "Stop".into(),
        cost: None,
        hotkey: chord(bindings, Action::StopOrScrap),
        action: CardAction::Dispatch(Action::StopOrScrap),
        enabled: true,
        why: None,
        desc: vec!["Clear orders; stand and auto-engage.".into()],
        progress: None,
    });
    panel.cards.push(Card {
        icon: CardIcon::Verb(VerbIcon::Move),
        title: "Run".into(),
        cost: None,
        hotkey: chord(bindings, Action::Run),
        action: CardAction::Dispatch(Action::Run),
        enabled: true,
        why: None,
        desc: vec![
            "Move to the selected ground without attacking".into(),
            "or acquiring targets.".into(),
        ],
        progress: None,
    });
    panel.cards.push(Card {
        icon: CardIcon::Verb(VerbIcon::Patrol),
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
        progress: None,
    });
    if has_builder {
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Salvage),
            title: "Salvage".into(),
            cost: None,
            hotkey: chord(bindings, Action::Salvage),
            action: CardAction::Dispatch(Action::Salvage),
            enabled: true,
            why: None,
            desc: vec![
                "Select a completed friendly building to dismantle".into(),
                "for a partial refund. Foundries cannot be salvaged.".into(),
            ],
            progress: None,
        });
    }
    if has_builder {
        panel.cards.push(Card {
            icon: CardIcon::Verb(VerbIcon::Repair),
            title: "Weld".into(),
            cost: None,
            hotkey: chord(bindings, Action::RepairUnit),
            action: CardAction::Dispatch(Action::RepairUnit),
            enabled: true,
            why: None,
            desc: vec![
                "Select a damaged friendly ground unit to repair it.".into(),
                "Each restored hit point consumes scrap.".into(),
            ],
            progress: None,
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
                progress: None,
            });
        }
    }
    // The first unit's program: what it is doing and what comes next.
    // An idle unit with nothing queued contributes no chips, so the
    // orders dock vanishes instead of showing a lone "Idle" cell.
    if !matches!(first.order, Order::Idle) || !first.queue.is_empty() {
        panel.queue.push(order_card(game, &first.order, true, true));
        for order in first.queue.iter().take(7) {
            panel.queue.push(order_card(game, order, false, true));
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
    fn verb_cards_wear_their_atlas_icons() {
        use chassis::grid::TilePos;
        use oxide_sim::UnitKind;
        let mut game = game();
        let sentinel = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == UnitKind::Sentinel)
            .expect("skirmish authors a sentinel")
            .id;
        game.state.tick(&[PlayerCommand {
            player: game.human,
            command: Command::AttackMove {
                units: vec![sentinel],
                goal: TilePos::new(20, 12),
                queue: false,
            },
        }]);
        game.selection.units = vec![sentinel];
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        let patrol = panel
            .cards
            .iter()
            .find(|c| c.title == "Patrol")
            .expect("patrol card");
        assert_eq!(patrol.icon, CardIcon::Verb(VerbIcon::Patrol));
        assert_eq!(patrol.hotkey, "R", "the tooltip chord stays live");
        let chip = &panel.queue[0];
        assert!(chip.title.starts_with("Attack-move"), "{}", chip.title);
        assert_eq!(
            chip.icon,
            CardIcon::Verb(VerbIcon::AttackMove),
            "chips wear pictograms, not letters that shadow chords"
        );
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
        assert!(!panel.cards[0].desc.iter().any(|l| l.contains("damage")));
        assert!(panel.cards[1].desc.iter().any(|l| l.contains("damage")));
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
        assert_eq!(panel.cards[1].title, "Run");
        assert_eq!(panel.cards[2].title, "Patrol");
        assert_eq!(
            panel.combat,
            vec!["unarmed"],
            "an unarmed unit still gives an explicit combat answer"
        );
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

    /// Places `kind` on the first tile the sim accepts near the
    /// harvester, returns the site.
    fn place(
        game: &mut Game,
        builder: oxide_sim::UnitId,
        kind: BuildingKind,
        queue: bool,
    ) -> BuildingId {
        use chassis::grid::TilePos;
        let here = game.state.unit(builder).expect("builder").tile();
        let before: Vec<BuildingId> = game.state.buildings().iter().map(|b| b.id).collect();
        for dy in -6i32..=6 {
            for dx in -6i32..=6 {
                let (x, y) = (here.x + dx, here.y + dy);
                if x < 0 || y < 0 {
                    continue;
                }
                let anchor = TilePos::new(x, y);
                if !game.state.can_place(game.human, kind, anchor) {
                    continue;
                }
                game.state.tick(&[PlayerCommand {
                    player: game.human,
                    command: Command::Build {
                        units: vec![builder],
                        kind,
                        anchor,
                        queue,
                        defer: false,
                    },
                }]);
                if let Some(b) = game
                    .state
                    .buildings()
                    .iter()
                    .find(|b| !before.contains(&b.id) && b.kind == kind)
                {
                    return b.id;
                }
            }
        }
        panic!("no ground accepted a {}", kind.name());
    }

    fn builder_game() -> (Game, oxide_sim::UnitId) {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 5000;
        let game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("skirmish builds");
        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        (game, harvester)
    }

    #[test]
    fn build_chips_wear_the_works_they_are_raising() {
        let (mut game, harvester) = builder_game();
        place(&mut game, harvester, BuildingKind::Turret, false);
        place(&mut game, harvester, BuildingKind::Array, true);
        game.selection.units = vec![harvester];
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert_eq!(panel.queue.len(), 2, "two legs of one program");
        // Two Build chips that no longer look the same: each carries
        // its own works, ghosted while the site is still rising.
        let faction = game.state.player(game.human).faction;
        assert_eq!(
            panel.queue[0].icon,
            CardIcon::Order {
                subject: OrderSubject::Building(BuildingKind::Turret, faction),
                verb: VerbIcon::Build,
                ghost: true,
            }
        );
        assert_eq!(
            panel.queue[1].icon,
            CardIcon::Order {
                subject: OrderSubject::Building(BuildingKind::Array, faction),
                verb: VerbIcon::Build,
                ghost: true,
            }
        );
        assert!(
            panel.queue[0].title.starts_with("Build - turret"),
            "{}",
            panel.queue[0].title
        );
        assert!(
            panel.queue[1].title.starts_with("Build - array"),
            "{}",
            panel.queue[1].title
        );
        assert!(panel.queue[0].desc.iter().any(|l| l.contains("% raised")));
        assert!(
            panel.queue[0].progress.is_some(),
            "a site chip meters its rise"
        );
        assert_eq!(
            panel.queue[0].action,
            CardAction::None,
            "an enriched chip is still display-only"
        );
    }

    #[test]
    fn a_chip_whose_subject_is_gone_falls_back_to_the_bare_verb() {
        // Orders outlive their subjects by a tick — the panel names
        // what it can find and never invents a silhouette.
        let (game, _) = builder_game();
        let dangling = Order::Repair {
            building: BuildingId(9999),
        };
        let card = order_card(&game, &dangling, true, true);
        assert_eq!(card.icon, CardIcon::Verb(VerbIcon::Repair));
        assert_eq!(card.title, "Repair (now)");
        assert!(card.progress.is_none());
        assert_eq!(card.desc.len(), 1, "no detail line it cannot back up");
    }

    #[test]
    fn a_foreign_program_is_never_enriched() {
        // Enriching an inspected ally's chips would rest the panel on a
        // claim about what team sight shares; a teammate's dock says
        // the verb and nothing about what it acts on.
        let (mut game, harvester) = builder_game();
        place(&mut game, harvester, BuildingKind::Turret, false);
        game.selection.units = vec![harvester];
        let own = build(&game, &BindingMap::classic()).expect("panel");
        assert!(matches!(own.queue[0].icon, CardIcon::Order { .. }));
        let order = game.state.unit(harvester).expect("builder").order;
        let bare = order_card(&game, &order, true, false);
        assert_eq!(bare.icon, CardIcon::Verb(VerbIcon::Build));
        assert_eq!(bare.title, "Build (now)");
        assert!(bare.progress.is_none());
    }

    #[test]
    fn weapon_lines_read_from_the_stats_table() {
        let sentinel = weapon_lines(oxide_sim::UnitKind::Sentinel);
        assert_eq!(sentinel.len(), 2, "main gun and the anti-air poke");
        assert!(sentinel[0].contains("damage"));
        assert!(sentinel[0].contains("range"));
        assert!(sentinel[0].contains("targets ground"));
        assert!(sentinel[1].contains("targets air"));
        let bombard = weapon_lines(oxide_sim::UnitKind::Bombard);
        assert!(bombard[0].contains("projectile"));
        assert!(bombard[0].contains("splash"));
    }

    #[test]
    fn a_single_unit_exposes_static_combat_facts_but_a_group_does_not() {
        let mut game = game();
        let sentinel = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == UnitKind::Sentinel)
            .expect("starting sentinel")
            .id;
        game.selection.units = vec![sentinel];
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert_eq!(panel.combat.len(), 2, "one line per weapon");
        assert!(panel.combat.iter().all(|line| line.contains("damage")));
        assert!(panel.combat.iter().all(|line| line.contains("range")));
        assert!(
            panel
                .combat
                .iter()
                .any(|line| line.contains("targets ground"))
        );
        assert!(panel.combat.iter().any(|line| line.contains("targets air")));

        let harvester = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.human && u.kind == UnitKind::Harvester)
            .expect("starting harvester")
            .id;
        game.selection.units = vec![sentinel, harvester];
        let panel = build(&game, &BindingMap::classic()).expect("panel");
        assert!(
            panel.combat.is_empty(),
            "mixed selections keep their compact type summary"
        );
    }
}
