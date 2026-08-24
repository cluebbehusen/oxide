//! The codex: every machine and works in the game, with its sprite,
//! its description, and its figures — the roster a player can read
//! without a match on the line. One screen object over the shared menu
//! widget: the list is the cursor, the selected row is the page.
//! Windowless update; the coordinator holds the displaced screen and
//! restores it wholesale on leave, exactly like Settings.

use crate::assets::Sprites;
use crate::game::SoundKind;
use crate::menu::Menu;
use crate::panel::{
    building_flavor, building_stat_line, building_weapon_lines, unit_flavor, unit_stat_line,
    weapon_lines,
};
use crate::render;
use crate::theme::{SURFACE_MENU, TEXT_ACCENT, TEXT_BODY, TEXT_PRIMARY, TEXT_SECONDARY};
use macroquad::prelude::*;
use oxide_protocol::{Key, RawEvent};
use oxide_sim::Faction;
use oxide_sim::stats::{BuildingKind, Domain, UnitKind};

/// What a codex frame decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Still reading.
    Stay,
    /// Back to wherever the screen was opened from.
    Leave,
}

/// One page of the codex.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entry {
    /// A machine.
    Unit(UnitKind),
    /// A works.
    Building(BuildingKind),
}

/// The codex in reading order: the factories as the player unlocks
/// them, each followed by what it trains, then every works. The
/// codex's section order is the tech order, so a new player reads the
/// game the way a match unfolds.
fn sections() -> Vec<(&'static str, Vec<Entry>)> {
    let trained_at = |factory: BuildingKind| -> Vec<Entry> {
        factory
            .base_stats()
            .produces
            .iter()
            .map(|kind| Entry::Unit(*kind))
            .collect()
    };
    vec![
        ("FOUNDRY", trained_at(BuildingKind::Foundry)),
        ("FABRICATOR", trained_at(BuildingKind::Fabricator)),
        ("AIRWORKS", trained_at(BuildingKind::Airworks)),
        ("CRUCIBLE", trained_at(BuildingKind::Crucible)),
        (
            "BUILDINGS",
            [
                BuildingKind::Foundry,
                BuildingKind::Fabricator,
                BuildingKind::Airworks,
                BuildingKind::Crucible,
                BuildingKind::Turret,
                BuildingKind::FlakTurret,
                BuildingKind::Bastion,
                BuildingKind::Barricade,
                BuildingKind::ScuttleCharge,
                BuildingKind::Extractor,
                BuildingKind::Reclaimer,
                BuildingKind::Array,
                BuildingKind::RepairBay,
            ]
            .into_iter()
            .map(Entry::Building)
            .collect(),
        ),
    ]
}

/// Capitalizes a lowercase display name for a heading.
fn title_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut start = true;
    for c in name.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start = c == ' ';
    }
    out
}

/// The codex screen: the list and what each row opens.
pub struct CodexScreen {
    /// The live list: section headers, one row per kind, and Back.
    pub menu: Menu,
    /// The page behind each row (`None` for headers and Back).
    entries: Vec<Option<Entry>>,
}

impl CodexScreen {
    /// Opens on the first machine. Where leaving lands is the
    /// coordinator's business.
    pub fn open() -> Self {
        let mut items = Vec::new();
        let mut entries = Vec::new();
        let mut headers = Vec::new();
        for (title, section) in sections() {
            headers.push(items.len());
            items.push(title.to_string());
            entries.push(None);
            for entry in section {
                let name = match entry {
                    Entry::Unit(kind) => kind.name(),
                    Entry::Building(kind) => kind.name(),
                };
                items.push(title_case(name));
                entries.push(Some(entry));
            }
        }
        items.push("Back".to_string());
        entries.push(None);
        // The list shifts left to make room for the page beside it.
        let mut menu = Menu::with_headers("ROSTER", items, headers);
        menu.shift = -0.24;
        Self { menu, entries }
    }

    /// The debug protocol's stable mode name.
    pub fn mode_name(&self) -> &'static str {
        "codex"
    }

    /// The page under the cursor, if the cursor is on a kind.
    pub fn selected_entry(&self) -> Option<Entry> {
        self.entries.get(self.menu.selected).copied().flatten()
    }

    /// Applies a frame's events. Rows are pages, not verbs: moving the
    /// cursor turns the page, and only Back (or Escape) acts.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Out {
        if events
            .iter()
            .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }))
        {
            return Out::Leave;
        }
        if let Some(row) = self.menu.handle(events, mouse) {
            sounds.push((SoundKind::Click, None));
            if self.entries[row].is_none() {
                return Out::Leave;
            }
        }
        Out::Stay
    }

    /// Draws the list and the selected page — the caller draws the
    /// veil first, so both land above it. `viewer` is the faction
    /// whose paint shared kinds wear; faction kinds wear their own.
    pub fn draw(&self, sprites: &Sprites, viewer: Faction) {
        self.menu
            .draw("every machine and works, in the order the factories unlock them");
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let s = render::ui_scale();
        let view = render::viewport();
        // The page sits right of the shifted list, between the title
        // zone and the hint line, and fits its own content.
        let x = view.x * 0.53;
        let w = (view.x * 0.44).min(560.0 * s);
        let top = (view.y * 0.36).max(view.y * 0.28 + 64.0 * s);
        let bottom = view.y - 64.0 * s;
        let pad = 14.0 * s;
        let plate = 84.0 * s;
        let plate_x = x + pad;
        let plate_y = top + pad;

        let (name, faction_line, sprite_factions): (String, String, Vec<Faction>) = match entry {
            Entry::Unit(kind) => {
                let owner = kind.faction();
                (
                    title_case(kind.name()),
                    match owner {
                        Some(Faction::Ferrous) => "Ferrous only".to_string(),
                        Some(Faction::Cupric) => "Cupric only".to_string(),
                        None => "shared roster".to_string(),
                    },
                    match owner {
                        Some(f) => vec![f],
                        None => vec![viewer, other(viewer)],
                    },
                )
            }
            Entry::Building(kind) => (
                title_case(kind.name()),
                "shared roster".to_string(),
                vec![viewer, other(viewer)],
            ),
        };
        let plates = sprite_factions.len() as f32;
        let plates_w = plate * plates + 6.0 * s * (plates - 1.0);
        let role = match entry {
            Entry::Unit(kind) => {
                let stats = kind.stats();
                let domain = match stats.domain {
                    Domain::Ground => "ground",
                    Domain::Air => "air",
                };
                format!("{faction_line} | {domain} | {} scrap", stats.cost)
            }
            Entry::Building(kind) => match kind.base_stats().construction {
                Some(c) => format!("{faction_line} | {} scrap", c.cost),
                None => faction_line,
            },
        };

        // The page: description, figures, weapons, and what else the
        // kind does — the same lines the training tooltip shows.
        let body_size = 16.0 * s;
        let line_h = 20.0 * s;
        let body_w = w - pad * 2.0;
        let measure = |t: &str| measure_text(t, None, body_size as u16, 1.0).width;
        let mut lines: Vec<(String, Color)> = Vec::new();
        let para = |text: &str, color: Color, lines: &mut Vec<(String, Color)>| {
            for line in render::wrap_words(text, measure, body_w) {
                lines.push((line, color));
            }
        };
        match entry {
            Entry::Unit(kind) => {
                para(unit_flavor(kind), TEXT_BODY, &mut lines);
                lines.push((String::new(), TEXT_BODY));
                lines.push((unit_stat_line(kind), TEXT_PRIMARY));
                for line in weapon_lines(kind) {
                    lines.push((line, TEXT_SECONDARY));
                }
                for line in unit_notes(kind) {
                    para(&line, TEXT_SECONDARY, &mut lines);
                }
            }
            Entry::Building(kind) => {
                para(building_flavor(kind), TEXT_BODY, &mut lines);
                lines.push((String::new(), TEXT_BODY));
                lines.push((building_stat_line(kind), TEXT_PRIMARY));
                for line in building_weapon_lines(kind, 0) {
                    lines.push((line, TEXT_SECONDARY));
                }
                for line in building_notes(kind) {
                    para(&line, TEXT_SECONDARY, &mut lines);
                }
            }
        }

        // The box fits its page: never shorter than the plate row,
        // never past the hint line.
        let text_top = plate_y + plate + pad + line_h;
        let box_bottom = (text_top + lines.len() as f32 * line_h + pad * 0.5).min(bottom);
        draw_rectangle(x, top, w, box_bottom - top, SURFACE_MENU);
        draw_rectangle_lines(
            x,
            top,
            w,
            box_bottom - top,
            1.5,
            Color::new(0.6, 0.6, 0.65, 0.4),
        );
        draw_rectangle(
            plate_x,
            plate_y,
            plates_w,
            plate,
            Color::from_rgba(35, 35, 41, 255),
        );
        for (i, faction) in sprite_factions.iter().enumerate() {
            let dest = Rect::new(
                plate_x + i as f32 * (plate + 6.0 * s),
                plate_y,
                plate,
                plate,
            );
            let blit = |source: Rect| {
                draw_texture_ex(
                    sprites.texture(),
                    dest.x,
                    dest.y,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(dest.w, dest.h)),
                        source: Some(source),
                        ..Default::default()
                    },
                );
            };
            match entry {
                Entry::Unit(kind) => blit(sprites.unit(kind, *faction)),
                Entry::Building(kind) => {
                    blit(sprites.building(kind, *faction));
                    if let Some(mount) = sprites.defense_mount(kind, *faction) {
                        blit(mount);
                    }
                }
            }
        }
        let text_x = plate_x + plates_w + pad;
        draw_text(&name, text_x, plate_y + 30.0 * s, 34.0 * s, TEXT_PRIMARY);
        draw_text(&role, text_x, plate_y + 54.0 * s, 16.0 * s, TEXT_ACCENT);
        let mut y = text_top;
        for (line, color) in lines {
            if y > box_bottom - pad * 0.5 {
                break;
            }
            draw_text(&line, x + pad, y, body_size, color);
            y += line_h;
        }
    }
}

fn other(faction: Faction) -> Faction {
    match faction {
        Faction::Ferrous => Faction::Cupric,
        Faction::Cupric => Faction::Ferrous,
    }
}

/// What a machine does besides fight, as page lines.
fn unit_notes(kind: UnitKind) -> Vec<String> {
    let stats = kind.stats();
    let mut notes = Vec::new();
    if let Some(harvest) = stats.harvest {
        notes.push(format!(
            "Hauls {} scrap a trip; digs {:.0} scrap/s.",
            harvest.capacity,
            oxide_sim::TICKS_PER_SECOND as f32 / harvest.ticks_per_scrap as f32
        ));
    }
    if stats.welder {
        notes.push(if stats.build_rate > 1 {
            format!(
                "Welds: repairs machines and raises buildings at {}x pace.",
                stats.build_rate
            )
        } else {
            "Welds: repairs machines and raises buildings.".to_string()
        });
    }
    if stats.transport_capacity > 0 {
        notes.push(format!(
            "Lifts {} sling points of ground machines.",
            stats.transport_capacity
        ));
    }
    if stats.transport_size > 1 {
        notes.push(format!(
            "Takes {} sling points to lift.",
            stats.transport_size
        ));
    }
    if stats.demolition {
        notes.push("Detonates on its target; always fatal to itself.".to_string());
    }
    if !stats.requires.is_empty() {
        let names: Vec<&str> = stats.requires.iter().map(|b| b.name()).collect();
        notes.push(format!("Needs a standing {}.", names.join(" and ")));
    }
    let factory = [
        BuildingKind::Foundry,
        BuildingKind::Fabricator,
        BuildingKind::Airworks,
        BuildingKind::Crucible,
    ]
    .into_iter()
    .find(|b| b.base_stats().produces.contains(&kind));
    if let Some(factory) = factory {
        notes.push(format!("Trained at the {}.", factory.name()));
    }
    notes
}

/// What a works does besides stand, as page lines: its requirements,
/// what it trains, and its upgrade rungs.
fn building_notes(kind: BuildingKind) -> Vec<String> {
    let base = kind.base_stats();
    let mut notes = Vec::new();
    if let Some(c) = base.construction
        && !c.requires.is_empty()
    {
        let names: Vec<&str> = c.requires.iter().map(|b| b.name()).collect();
        notes.push(format!("Needs a standing {}.", names.join(" and ")));
    }
    if !base.produces.is_empty() {
        let names: Vec<String> = base.produces.iter().map(|u| title_case(u.name())).collect();
        notes.push(format!("Trains {}.", names.join(", ")));
    }
    for (tier, stats) in kind.tiers().iter().enumerate().skip(1) {
        let Some(c) = stats.construction else {
            continue;
        };
        let mut line = format!(
            "Upgrade: {} for {} scrap ({:.0} s){} - {} hp",
            kind.tier_name(tier as u8),
            c.cost,
            c.build_ticks as f32 / oxide_sim::TICKS_PER_SECOND as f32,
            if c.requires.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = c.requires.iter().map(|b| b.name()).collect();
                format!(", needs a {}", names.join(" and "))
            },
            stats.max_hp
        );
        for weapon in building_weapon_lines(kind, tier as u8) {
            line.push_str(&format!("; {weapon}"));
        }
        notes.push(line);
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn drive(screen: &mut CodexScreen, key: Key) -> Out {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        screen.update(
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
            &mut mouse,
            &mut sounds,
        )
    }

    #[test]
    fn every_kind_has_exactly_one_page() {
        let screen = CodexScreen::open();
        let mut units = 0;
        let mut buildings = 0;
        for entry in screen.entries.iter().flatten() {
            match entry {
                Entry::Unit(_) => units += 1,
                Entry::Building(_) => buildings += 1,
            }
        }
        assert_eq!(units, oxide_sim::stats::UnitKind::ALL.len());
        assert_eq!(buildings, oxide_sim::stats::BuildingKind::ALL.len());
        // No machine is listed under two factories.
        let mut seen = std::collections::BTreeSet::new();
        for entry in screen.entries.iter().flatten() {
            assert!(seen.insert(format!("{entry:?}")), "{entry:?} listed twice");
        }
    }

    #[test]
    fn opens_on_a_page_and_escape_leaves() {
        let mut screen = CodexScreen::open();
        assert_eq!(screen.mode_name(), "codex");
        assert_eq!(
            screen.selected_entry(),
            Some(Entry::Unit(UnitKind::Harvester)),
            "the first real row is the first Foundry machine"
        );
        assert_eq!(drive(&mut screen, Key::Down), Out::Stay);
        assert_eq!(
            screen.selected_entry(),
            Some(Entry::Unit(UnitKind::Sentinel))
        );
        assert_eq!(drive(&mut screen, Key::Escape), Out::Leave);
    }

    #[test]
    fn back_is_the_last_row_and_leaves() {
        let mut screen = CodexScreen::open();
        let last = screen.menu.items.len() - 1;
        assert_eq!(screen.menu.items[last], "Back");
        screen.menu.select(last);
        assert_eq!(screen.selected_entry(), None);
        assert_eq!(drive(&mut screen, Key::Enter), Out::Leave);
    }

    #[test]
    fn activating_a_page_row_stays() {
        let mut screen = CodexScreen::open();
        assert_eq!(drive(&mut screen, Key::Enter), Out::Stay);
    }

    #[test]
    fn notes_read_from_the_stats_table() {
        let harvester = unit_notes(UnitKind::Harvester);
        assert!(harvester.iter().any(|l| l.starts_with("Hauls 10 scrap")));
        assert!(
            harvester
                .iter()
                .any(|l| l.contains("Trained at the foundry"))
        );
        let skyhook = unit_notes(UnitKind::Skyhook);
        assert!(skyhook.iter().any(|l| l.starts_with("Lifts 4 sling")));
        let turret = building_notes(BuildingKind::Turret);
        assert_eq!(turret.len(), 2, "two upgrade rungs: {turret:?}");
        assert!(turret[0].contains("heavy turret"));
        assert!(turret[1].contains("bulwark") && turret[1].contains("crucible"));
    }
}
