//! The New Match flow: the map browser grid, then either the 1v1
//! quick questions (difficulty, personality, faction) or — on team
//! maps — the match setup screen: seat cards grouped by team beside a
//! large who-is-where preview.
//!
//! Two front-ends, one back: whatever the flow, every answer lands in
//! the draft's PER-SEAT vector, and `launch()` reads only that.

use crate::game::SoundKind;
use crate::menu::{Menu, PreviewCache, ScenarioEntry, discover_scenarios};
use crate::screens::browser::{Browser, Out as BrowserOut};
use anyhow::{Context, Result};
use macroquad::prelude::{
    Color, DrawTextureParams, Rect, Vec2, color_u8, draw_circle, draw_circle_lines, draw_rectangle,
    draw_rectangle_lines, draw_text, draw_texture_ex, measure_text, vec2,
};
use oxide_protocol::{Key, MouseButton, RawEvent};
use oxide_sim::Scenario;
use std::path::PathBuf;

const TITLE_COLOR: Color = color_u8!(196, 87, 59, 255);
const ITEM_COLOR: Color = color_u8!(214, 210, 196, 255);
const DIM: Color = color_u8!(214, 210, 196, 120);
const PANEL: Color = color_u8!(20, 20, 24, 230);

/// One seat's dials in the draft.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeatPlan {
    /// Difficulty row (indexes `Level::LADDER`).
    pub level_choice: usize,
    /// Personality row (feeds [`personality_knob`]).
    pub personality_choice: usize,
    /// Faction chip (feeds [`faction_override`]): 0 keeps the map's
    /// authored roster. The one dial the human's own card carries too.
    pub faction_choice: usize,
}

impl Default for SeatPlan {
    fn default() -> Self {
        Self {
            level_choice: 1, // Medium is the fair default
            personality_choice: 0,
            faction_choice: 0, // the authored roster
        }
    }
}

/// The faction a chip value forces onto its seat; `None` keeps the
/// map's authored faction.
pub fn faction_override(choice: usize) -> Option<oxide_sim::Faction> {
    match choice {
        1 => Some(oxide_sim::Faction::Ferrous),
        2 => Some(oxide_sim::Faction::Cupric),
        _ => None,
    }
}

/// The faction a seat will actually run: its chip override, or the
/// map's authored roster.
pub fn effective_faction(
    scenario: &Scenario,
    draft: &NewMatchDraft,
    seat: usize,
) -> oxide_sim::Faction {
    draft
        .seats
        .get(seat)
        .and_then(|p| faction_override(p.faction_choice))
        .unwrap_or(scenario.players[seat].faction)
}

/// Everything New Match has chosen so far. The draft outlives every
/// screen transition: backing from any step to the map list and
/// forward again re-offers each earlier answer instead of forgetting
/// it.
#[derive(Default)]
pub struct NewMatchDraft {
    /// The loaded map, once picked.
    pub scenario: Option<Box<Scenario>>,
    /// The picked map's path (`None` = the embedded skirmish) — keyed
    /// by PATH so the browser's section sort can never move the
    /// remembered highlight onto a different map.
    pub scenario_path: Option<PathBuf>,
    /// Which chair the human takes (index into the scenario's players).
    pub seat_choice: usize,
    /// One plan per seat, aligned with the scenario's player list and
    /// re-derived (and clamped) whenever the scenario is assigned —
    /// Back from an 8-seat map to a 2-seat map must not leave a stale
    /// seat 7. The human's own row is inert at launch.
    pub seats: Vec<SeatPlan>,
    /// Faction row for the 1v1 quick flow (Ferrous / Cupric /
    /// surprise). Team maps have no faction question: picking a seat
    /// IS picking its authored faction.
    pub faction_choice: usize,
}

impl NewMatchDraft {
    /// Installs a picked map: the per-seat vector re-derives at the new
    /// map's width (existing dials survive where seats overlap) and the
    /// seat choice clamps onto the board.
    pub fn set_scenario(&mut self, scenario: Scenario, path: Option<PathBuf>) {
        let count = scenario.players.len();
        self.seats.resize(count, SeatPlan::default());
        self.seats.truncate(count);
        self.seat_choice = self.seat_choice.min(count.saturating_sub(1));
        self.scenario = Some(Box::new(scenario));
        self.scenario_path = path;
    }

    /// Whether the picked map runs the per-seat setup screen (anything
    /// beyond a duel) instead of the 1v1 quick flow.
    pub fn team_map(&self) -> bool {
        self.scenario.as_ref().is_some_and(|s| s.players.len() > 2)
    }
}

const DIFFICULTY_ITEMS: [&str; 4] = ["Easy", "Medium", "Hard", "Expert"];
const PERSONALITY_ITEMS: [&str; 4] = ["Surprise me", "Turtle", "Balanced", "Aggressive"];
const FACTION_ITEMS: [&str; 3] = ["Ferrous", "Cupric", "Surprise me"];
/// The setup cards' faction chip values, aligned with
/// [`faction_override`].
const FACTION_CHIP_ITEMS: [&str; 3] = ["Auto", "Ferrous", "Cupric"];

/// The personality dial a wizard row means; `None` lets the scenario
/// seed deal one.
pub fn personality_knob(choice: usize) -> Option<u32> {
    match choice {
        1 => Some(100), // Turtle
        2 => Some(500), // Balanced
        3 => Some(900), // Aggressive
        _ => None,      // Surprise me
    }
}

/// Which wizard question is on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// The map browser grid.
    Map,
    /// Difficulty picker (1v1 quick flow).
    Difficulty,
    /// Personality picker (1v1 quick flow).
    Personality,
    /// Faction picker — the quick flow's last question; answering
    /// launches.
    Faction,
    /// Team-map match setup: seat cards by team with INLINE dials,
    /// Start, a live map.
    Setup,
}

/// What a wizard frame decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Still asking.
    Stay,
    /// Backed all the way out to the front door.
    Home,
    /// Every question answered: the caller launches from the draft.
    Launch,
}

/// The wizard: current step, the row menu the quick-flow steps use,
/// the browser grid, and the setup cursor.
pub struct Wizard {
    /// Which question is up.
    pub step: Step,
    /// The quick-flow steps' menu (and the seat-detail submenu).
    pub menu: Menu,
    /// Discovered scenario entries, section-sorted.
    pub entries: Vec<ScenarioEntry>,
    /// The map grid's state.
    pub browser: Browser,
    /// Setup cursor over the DISPLAY order: seats grouped by team,
    /// then the Start button.
    pub setup_sel: usize,
    /// Which cell of the selected seat card the cursor is on: 0 the
    /// seat itself, 1 its difficulty dial, 2 its personality dial.
    /// Sticky across rows — walking the roster on a dial column edits
    /// in bulk. Rows without dials clamp to 0.
    pub setup_cell: usize,
    /// Setup zone armed by a press: (row, cell); activation on
    /// release inside the same zone.
    setup_pressed: Option<(usize, usize)>,
}

/// The plan the quick flow's preselects mirror: the first AI seat's
/// (the human's own row is inert and never written).
fn ai_plan(draft: &NewMatchDraft) -> SeatPlan {
    draft
        .seats
        .iter()
        .enumerate()
        .find(|(i, _)| *i != draft.seat_choice)
        .map(|(_, p)| *p)
        .unwrap_or_default()
}

fn rows_menu(title: &str, items: &[&str], selected: usize) -> Menu {
    let mut rows: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    rows.push("Back".to_string());
    let mut menu = Menu::new(title, rows);
    menu.select(selected.min(items.len() - 1));
    menu
}

/// One collision-free team key per seat: an authored id stays
/// itself; an omitted seat surrogates as its own index lifted above
/// the whole u8 range, so no authored id can alias it and every pass
/// that groups seats derives the identical key. (The old scheme keyed
/// omitted seats two different ways — `teams.len()` in one pass, the
/// seat index in the next — and a mixed-team map could drop a seat
/// from the setup order entirely.)
fn seat_team_keys(scenario: &Scenario) -> Vec<u16> {
    scenario
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| p.team.map(u16::from).unwrap_or(256 + i as u16))
        .collect()
}

/// Seats in DISPLAY order: grouped by team (first appearance), seat
/// order within — the setup screen's visual order and its cursor's
/// walking order are the same list.
pub fn seat_display_order(scenario: &Scenario) -> Vec<usize> {
    let keys = seat_team_keys(scenario);
    let mut teams: Vec<u16> = Vec::new();
    for &k in &keys {
        if !teams.contains(&k) {
            teams.push(k);
        }
    }
    let mut order: Vec<usize> = Vec::new();
    for team in &teams {
        for (i, &k) in keys.iter().enumerate() {
            if k == *team {
                order.push(i);
            }
        }
    }
    order
}

/// The setup screen's frame geometry, a pure function of the map and
/// window: seat cards on the left grouped under team headings, the
/// Start button beneath them, the preview panel filling the right.
pub struct SetupLayout {
    /// Team headings and their text rects.
    pub headings: Vec<(String, Rect)>,
    /// One card rect per DISPLAY position (see [`seat_display_order`]).
    pub seats: Vec<Rect>,
    /// Per card, the four interactive zones: the seat itself, its
    /// difficulty chip, its personality chip, its faction chip. The
    /// human's own card keeps only seat and faction — the AI dials'
    /// rects are zero-sized there.
    pub cells: Vec<[Rect; 4]>,
    /// The Start button.
    pub start: Rect,
    /// Where the map preview draws.
    pub preview: Rect,
}

/// Computes [`SetupLayout`]. `seat_choice` marks the card without
/// dials (the human edits opponents, not itself).
pub fn setup_layout(scenario: &Scenario, seat_choice: usize, view: Vec2, ui: f32) -> SetupLayout {
    let order = seat_display_order(scenario);
    let n = order.len();
    let keys = seat_team_keys(scenario);
    let teams: Vec<u16> = {
        let mut seen = Vec::new();
        for &s in &order {
            let t = keys[s];
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen
    };
    let left_x = 56.0 * ui;
    let left_w = (view.x * 0.42).min(520.0 * ui);
    // Margins yield before content: small windows compress the title
    // zone first, then chrome, then the cards — the full roster and
    // the Start button stay on screen by construction (the old fixed
    // 34ui card floor pushed Start past the bottom of a 640x400
    // window on eight-seat maps, with no scrolling to reach it).
    let top = (132.0 * ui).min(view.y * 0.22);
    let bottom = view.y - (44.0 * ui).min(view.y * 0.08);
    let mut heading_h = 26.0 * ui;
    let mut start_h = 46.0 * ui;
    let mut gap = 6.0 * ui;
    let mut avail = bottom - top - teams.len() as f32 * heading_h - start_h - 24.0 * ui;
    let mut card_h = ((avail / n.max(1) as f32) - gap).clamp(34.0 * ui, 56.0 * ui);
    if n as f32 * (card_h + gap) > avail {
        heading_h *= 0.7;
        start_h *= 0.75;
        gap *= 0.5;
        avail = bottom - top - teams.len() as f32 * heading_h - start_h - 24.0 * ui;
        card_h = ((avail / n.max(1) as f32) - gap).max(18.0 * ui);
    }

    let mut headings = Vec::new();
    let mut seats = vec![Rect::new(0.0, 0.0, 0.0, 0.0); n];
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cells = vec![[zero; 4]; n];
    let mut y = top;
    let mut last_team: Option<u16> = None;
    for (pos, &seat) in order.iter().enumerate() {
        let team = keys[seat];
        if last_team != Some(team) {
            let label = format!(
                "TEAM {}",
                teams.iter().position(|t| *t == team).unwrap() + 1
            );
            headings.push((label, Rect::new(left_x, y, left_w, heading_h)));
            last_team = Some(team);
            y += heading_h;
        }
        let card = Rect::new(left_x, y, left_w, card_h);
        seats[pos] = card;
        // The inline dial chips, right-aligned; the seat zone is the
        // rest of the card. Every card carries the faction chip —
        // the human's own card carries ONLY that.
        let fac_w = 82.0 * ui;
        let pers_w = 118.0 * ui;
        let diff_w = 78.0 * ui;
        let pad = 8.0 * ui;
        // Proportional, so a squeezed card keeps its chips inside.
        let chip_h = (card_h * 0.72).clamp(14.0 * ui, 40.0 * ui);
        let cy = y + (card_h - chip_h) * 0.5;
        let fac = Rect::new(card.x + card.w - fac_w - pad, cy, fac_w, chip_h);
        if seat != seat_choice {
            let pers = Rect::new(fac.x - pers_w - pad, cy, pers_w, chip_h);
            let diff = Rect::new(pers.x - diff_w - pad, cy, diff_w, chip_h);
            let seat_zone = Rect::new(card.x, y, diff.x - card.x, card_h);
            cells[pos] = [seat_zone, diff, pers, fac];
        } else {
            let seat_zone = Rect::new(card.x, y, fac.x - card.x, card_h);
            cells[pos] = [seat_zone, zero, zero, fac];
        }
        y += card_h + gap;
    }
    let start = Rect::new(left_x, y + 12.0 * ui, 240.0 * ui, start_h);
    let px = left_x + left_w + 28.0 * ui;
    let preview = Rect::new(px, top, view.x - px - 40.0 * ui, bottom - top - 20.0 * ui);
    SetupLayout {
        headings,
        seats,
        cells,
        start,
        preview,
    }
}

/// Every Foundry anchor authored on an ASCII map: `(seat, (x, y))` for
/// each digit `1`..=`8`, in row-major order.
pub fn seat_anchors(map: &[String]) -> Vec<(usize, (i32, i32))> {
    let mut anchors = Vec::new();
    for (y, row) in map.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            if let Some(digit) = ch.to_digit(10)
                && (1..=8).contains(&digit)
            {
                anchors.push((digit as usize - 1, (x as i32, y as i32)));
            }
        }
    }
    anchors
}

/// Marks every seat's foundry on a drawn preview rect: numbered discs
/// in the seat's EFFECTIVE faction color (chip overrides included), a
/// white ring for the human's chair, an accent ring for the focused
/// seat — the list and the map read as one thing.
pub fn draw_seat_markers(
    scenario: &Scenario,
    draft: &NewMatchDraft,
    rect: Rect,
    seat_choice: usize,
    focus_seat: Option<usize>,
    ui: f32,
) {
    let map_w = scenario.map.first().map_or(1, |r| r.chars().count()) as f32;
    let map_h = scenario.map.len() as f32;
    for (seat, (ax, ay)) in seat_anchors(&scenario.map) {
        if scenario.players.get(seat).is_none() {
            continue;
        }
        // Foundry anchors are the 2x2's top-left; mark its center.
        let px = rect.x + (ax as f32 + 1.0) / map_w * rect.w;
        let py = rect.y + (ay as f32 + 1.0) / map_h * rect.h;
        let accent = crate::render::faction_accent(effective_faction(scenario, draft, seat));
        if seat == seat_choice {
            draw_circle_lines(px, py, 10.0 * ui, 2.5, macroquad::prelude::WHITE);
        } else if focus_seat == Some(seat) {
            draw_circle_lines(px, py, 10.0 * ui, 2.0, accent);
        }
        draw_circle(px, py, 7.5 * ui, accent);
        let label = format!("{}", seat + 1);
        let tw = measure_text(&label, None, (13.0 * ui) as u16, 1.0).width;
        draw_text(
            &label,
            px - tw * 0.5,
            py + 4.5 * ui,
            13.0 * ui,
            Color::from_rgba(20, 20, 24, 255),
        );
    }
}

impl Wizard {
    /// Opens at the map grid, the remembered map re-highlighted.
    pub fn open(draft: &NewMatchDraft) -> Self {
        let entries = discover_scenarios();
        let mut browser = Browser::new();
        if draft.scenario.is_some() {
            browser.select_path(&entries, &draft.scenario_path);
        }
        Self {
            step: Step::Map,
            menu: Menu::new("OXIDE", Vec::new()),
            entries,
            browser,
            setup_sel: 0,
            setup_cell: 0,
            setup_pressed: None,
        }
    }

    /// The entry for the draft's picked map, with its stable index —
    /// what the setup screen keys its preview by.
    pub fn picked_entry(&self, draft: &NewMatchDraft) -> Option<(usize, &ScenarioEntry)> {
        self.entries
            .iter()
            .position(|e| e.path == draft.scenario_path)
            .and_then(|i| self.entries.get(i).map(|e| (i, e)))
    }

    fn goto(&mut self, step: Step, draft: &NewMatchDraft) {
        self.step = step;
        self.menu = match step {
            Step::Map => {
                self.entries = discover_scenarios();
                self.browser
                    .select_path(&self.entries, &draft.scenario_path);
                Menu::new("OXIDE", Vec::new())
            }
            Step::Difficulty => {
                rows_menu("DIFFICULTY", &DIFFICULTY_ITEMS, ai_plan(draft).level_choice)
            }
            Step::Personality => rows_menu(
                "OPPONENT",
                &PERSONALITY_ITEMS,
                ai_plan(draft).personality_choice,
            ),
            Step::Faction => rows_menu("FACTION", &FACTION_ITEMS, draft.faction_choice),
            Step::Setup => {
                // Start preselected: Enter-Enter from the grid plays
                // the map as authored.
                self.setup_sel = draft.seats.len();
                self.setup_cell = 0;
                self.setup_pressed = None;
                Menu::new("MATCH SETUP", Vec::new())
            }
        };
    }

    /// Writes a 1v1 quick-flow answer through to every AI seat: the two
    /// front-ends share one back — `launch()` reads only the vector.
    fn write_all_seats(draft: &mut NewMatchDraft, write: impl Fn(&mut SeatPlan)) {
        for (i, plan) in draft.seats.iter_mut().enumerate() {
            if i != draft.seat_choice {
                write(plan);
            }
        }
    }

    /// Applies a frame's events. Windowless: the screens navigate, the
    /// draft records answers, and the return says whether the caller
    /// should stay, go Home, or launch the match.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        draft: &mut NewMatchDraft,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Result<Out> {
        match self.step {
            Step::Map => match self.browser.handle(&self.entries, events, mouse) {
                BrowserOut::Back => return Ok(Out::Home),
                BrowserOut::Pick(entry) => {
                    sounds.push((SoundKind::Click, None));
                    let scenario = match &self.entries[entry].path {
                        Some(path) => Scenario::load(path)
                            .with_context(|| format!("loading {}", path.display()))?,
                        None => Scenario::skirmish(),
                    };
                    draft.set_scenario(scenario, self.entries[entry].path.clone());
                    if draft.team_map() {
                        self.goto(Step::Setup, draft);
                    } else {
                        self.goto(Step::Difficulty, draft);
                    }
                }
                BrowserOut::Stay => {}
            },
            Step::Setup => {
                if let Some(out) = self.update_setup(events, mouse, draft, sounds) {
                    return Ok(out);
                }
            }
            Step::Difficulty | Step::Personality | Step::Faction => {
                let escaped = events
                    .iter()
                    .any(|e| matches!(e, RawEvent::KeyDown { key: Key::Escape }));
                let choice = self.menu.handle(events, mouse);
                if choice.is_some() {
                    sounds.push((SoundKind::Click, None));
                }
                match self.step {
                    Step::Difficulty => {
                        if escaped || choice.is_some_and(|c| c >= DIFFICULTY_ITEMS.len()) {
                            self.goto(Step::Map, draft);
                        } else if let Some(c) = choice {
                            Self::write_all_seats(draft, |p| p.level_choice = c);
                            self.goto(Step::Personality, draft);
                        }
                    }
                    Step::Personality => {
                        if escaped || choice.is_some_and(|c| c >= PERSONALITY_ITEMS.len()) {
                            self.goto(Step::Difficulty, draft);
                        } else if let Some(c) = choice {
                            Self::write_all_seats(draft, |p| p.personality_choice = c);
                            self.goto(Step::Faction, draft);
                        }
                    }
                    Step::Faction => {
                        if escaped || choice.is_some_and(|c| c >= FACTION_ITEMS.len()) {
                            self.goto(Step::Personality, draft);
                        } else if let Some(c) = choice {
                            draft.faction_choice = c;
                            return Ok(Out::Launch);
                        }
                    }
                    _ => unreachable!("outer match routed these"),
                }
            }
        }
        Ok(Out::Stay)
    }

    /// The setup screen's input: Up/Down walk the seat cards and the
    /// Start button; Left/Right walk a card's cells (seat, difficulty,
    /// personality — the cell column is sticky, so walking the roster
    /// on a dial edits in bulk); Enter takes the seat or cycles the
    /// dial under the cursor; clicks hit each zone directly.
    fn update_setup(
        &mut self,
        events: &[RawEvent],
        mouse: &mut Vec2,
        draft: &mut NewMatchDraft,
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Option<Out> {
        let Some(scenario) = draft.scenario.as_deref() else {
            self.goto(Step::Map, draft);
            return None;
        };
        let order = seat_display_order(scenario);
        let start_index = order.len();
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let layout = setup_layout(scenario, draft.seat_choice, view, ui);
        // A cell is walkable when its zone has width: the human's own
        // card keeps only the seat zone and the faction chip.
        let cell_live = |row: usize, cell: usize| -> bool {
            row < start_index && (cell == 0 || layout.cells[row][cell].w > 0.0)
        };
        let zone_at = |p: Vec2| -> Option<(usize, usize)> {
            for (row, cells) in layout.cells.iter().enumerate() {
                for (cell, r) in cells.iter().enumerate() {
                    if r.w > 0.0 && r.contains(p) {
                        return Some((row, cell));
                    }
                }
            }
            layout.start.contains(p).then_some((start_index, 0))
        };
        let mut activate: Option<(usize, usize)> = None;
        for event in events {
            match *event {
                RawEvent::KeyDown { key: Key::Escape } => {
                    self.goto(Step::Map, draft);
                    return None;
                }
                RawEvent::KeyDown { key: Key::Up } => {
                    self.setup_sel = self.setup_sel.checked_sub(1).unwrap_or(start_index);
                }
                RawEvent::KeyDown { key: Key::Down } => {
                    self.setup_sel = (self.setup_sel + 1) % (start_index + 1);
                }
                RawEvent::KeyDown { key: Key::Left } => {
                    let mut c = self.setup_cell;
                    while c > 0 {
                        c -= 1;
                        if cell_live(self.setup_sel, c) {
                            break;
                        }
                    }
                    self.setup_cell = c;
                }
                RawEvent::KeyDown { key: Key::Right } => {
                    let mut c = self.setup_cell + 1;
                    while c <= 3 && !cell_live(self.setup_sel, c) {
                        c += 1;
                    }
                    if c <= 3 && cell_live(self.setup_sel, c) {
                        self.setup_cell = c;
                    }
                }
                RawEvent::KeyDown { key: Key::Home } => self.setup_sel = 0,
                RawEvent::KeyDown { key: Key::End } => self.setup_sel = start_index,
                RawEvent::KeyDown { key: Key::Enter } => {
                    // The sticky column falls back to the seat zone on
                    // rows where its cell is dead.
                    let cell = if cell_live(self.setup_sel, self.setup_cell) {
                        self.setup_cell
                    } else {
                        0
                    };
                    activate = Some((self.setup_sel, cell));
                }
                RawEvent::MouseMove { x, y } => *mouse = vec2(x, y),
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    self.setup_pressed = zone_at(vec2(x, y));
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let released = zone_at(vec2(x, y));
                    let armed = self.setup_pressed.take();
                    if let (Some(a), Some(r)) = (armed, released)
                        && a == r
                    {
                        self.setup_sel = a.0;
                        if cell_live(a.0, a.1) {
                            self.setup_cell = a.1;
                        }
                        activate = Some(a);
                    }
                }
                _ => {}
            }
        }
        if let Some((row, cell)) = activate {
            sounds.push((SoundKind::Click, None));
            if row == start_index {
                return Some(Out::Launch);
            }
            let seat = order[row];
            match cell {
                // Seat choice never permutes seats — teams and dials
                // stay put — it moves the human's chair.
                0 => draft.seat_choice = seat,
                1 => {
                    let plan = &mut draft.seats[seat];
                    plan.level_choice = (plan.level_choice + 1) % DIFFICULTY_ITEMS.len();
                }
                2 => {
                    let plan = &mut draft.seats[seat];
                    plan.personality_choice =
                        (plan.personality_choice + 1) % PERSONALITY_ITEMS.len();
                }
                _ => {
                    let plan = &mut draft.seats[seat];
                    plan.faction_choice = (plan.faction_choice + 1) % FACTION_CHIP_ITEMS.len();
                }
            }
        }
        None
    }

    /// Draws the setup screen: team-grouped seat cards, the Start
    /// button, and the live map with every chair marked.
    pub fn draw_setup(&self, draft: &NewMatchDraft, previews: &mut PreviewCache) {
        let Some(scenario) = draft.scenario.as_deref() else {
            return;
        };
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let layout = setup_layout(scenario, draft.seat_choice, view, ui);
        let order = seat_display_order(scenario);

        let title = "MATCH SETUP";
        let tsize = 56.0 * ui;
        let tdims = measure_text(title, None, tsize as u16, 1.0);
        draw_text(
            title,
            (view.x - tdims.width) * 0.5,
            64.0 * ui,
            tsize,
            TITLE_COLOR,
        );
        let sub = format!("{} - pick your seat; tune each opponent", scenario.name);
        let sdims = measure_text(&sub, None, (18.0 * ui) as u16, 1.0);
        draw_text(
            &sub,
            (view.x - sdims.width) * 0.5,
            92.0 * ui,
            18.0 * ui,
            DIM,
        );

        for (label, rect) in &layout.headings {
            draw_text(label, rect.x, rect.y + rect.h * 0.7, 17.0 * ui, TITLE_COLOR);
            let dims = measure_text(label, None, (17.0 * ui) as u16, 1.0);
            draw_rectangle(
                rect.x + dims.width + 12.0 * ui,
                rect.y + rect.h * 0.55,
                rect.w - dims.width - 12.0 * ui,
                1.0,
                Color::new(0.6, 0.6, 0.65, 0.25),
            );
        }
        for (pos, rect) in layout.seats.iter().enumerate() {
            let seat = order[pos];
            let spec = &scenario.players[seat];
            let selected = self.setup_sel == pos;
            let is_you = seat == draft.seat_choice;
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, PANEL);
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if selected { 2.5 } else { 1.0 },
                if selected {
                    TITLE_COLOR
                } else {
                    Color::new(0.6, 0.6, 0.65, 0.3)
                },
            );
            let accent = crate::render::faction_accent(effective_faction(scenario, draft, seat));
            let cy = rect.y + rect.h * 0.5;
            let chip_x = rect.x + 22.0 * ui;
            if is_you {
                draw_circle_lines(chip_x, cy, 13.0 * ui, 2.0, macroquad::prelude::WHITE);
            }
            draw_circle(chip_x, cy, 10.0 * ui, accent);
            let num = format!("{}", seat + 1);
            let ndims = measure_text(&num, None, (14.0 * ui) as u16, 1.0);
            draw_text(
                &num,
                chip_x - ndims.width * 0.5,
                cy + 5.0 * ui,
                14.0 * ui,
                Color::from_rgba(20, 20, 24, 255),
            );
            draw_text(
                &spec.name,
                rect.x + 44.0 * ui,
                cy + 5.0 * ui,
                16.0 * ui,
                ITEM_COLOR,
            );
            if is_you {
                let tag = "your seat";
                let tdims = measure_text(tag, None, (14.0 * ui) as u16, 1.0);
                let fac = layout.cells[pos][3];
                draw_text(
                    tag,
                    fac.x - tdims.width - 14.0 * ui,
                    cy + 5.0 * ui,
                    14.0 * ui,
                    DIM,
                );
            }
            // The inline dials: boxed value chips; the cursor's cell
            // wears the accent. The human's own card shows only its
            // faction chip.
            let plan = draft.seats[seat];
            let labels = [
                DIFFICULTY_ITEMS[plan.level_choice],
                PERSONALITY_ITEMS[plan.personality_choice],
                FACTION_CHIP_ITEMS[plan.faction_choice],
            ];
            for (ci, label) in labels.iter().enumerate() {
                let chip = layout.cells[pos][ci + 1];
                if chip.w <= 0.0 {
                    continue;
                }
                let on_cell = selected && self.setup_cell == ci + 1;
                draw_rectangle(
                    chip.x,
                    chip.y,
                    chip.w,
                    chip.h,
                    Color::from_rgba(32, 32, 38, 255),
                );
                draw_rectangle_lines(
                    chip.x,
                    chip.y,
                    chip.w,
                    chip.h,
                    if on_cell { 2.0 } else { 1.0 },
                    if on_cell {
                        TITLE_COLOR
                    } else {
                        Color::new(0.6, 0.6, 0.65, 0.35)
                    },
                );
                let ldims = measure_text(label, None, (13.0 * ui) as u16, 1.0);
                draw_text(
                    label,
                    chip.x + (chip.w - ldims.width) * 0.5,
                    chip.y + chip.h * 0.5 + 4.5 * ui,
                    13.0 * ui,
                    if on_cell { ITEM_COLOR } else { DIM },
                );
            }
            // The seat-zone cell cursor: a soft inner line under
            // the name, so "Enter takes this chair" reads.
            if selected && self.setup_cell == 0 && !is_you {
                let zone = layout.cells[pos][0];
                draw_rectangle(
                    zone.x + 44.0 * ui,
                    cy + 9.0 * ui,
                    measure_text(&spec.name, None, (16.0 * ui) as u16, 1.0).width,
                    1.5,
                    TITLE_COLOR,
                );
            }
        }
        // Start button.
        let start_selected = self.setup_sel == layout.seats.len();
        draw_rectangle(
            layout.start.x,
            layout.start.y,
            layout.start.w,
            layout.start.h,
            PANEL,
        );
        draw_rectangle_lines(
            layout.start.x,
            layout.start.y,
            layout.start.w,
            layout.start.h,
            if start_selected { 3.0 } else { 1.5 },
            if start_selected { TITLE_COLOR } else { DIM },
        );
        let label = "Start match";
        let ldims = measure_text(label, None, (20.0 * ui) as u16, 1.0);
        draw_text(
            label,
            layout.start.x + (layout.start.w - ldims.width) * 0.5,
            layout.start.y + layout.start.h * 0.66,
            20.0 * ui,
            if start_selected { ITEM_COLOR } else { DIM },
        );

        // The map, large, with every chair marked.
        if let Some((idx, entry)) = self.picked_entry(draft)
            && let Some(tex) = previews.get(idx, entry)
        {
            let scale = (layout.preview.w / tex.width()).min(layout.preview.h / tex.height());
            let (pw, ph) = (tex.width() * scale, tex.height() * scale);
            let x = layout.preview.x + (layout.preview.w - pw) * 0.5;
            let y = layout.preview.y + (layout.preview.h - ph) * 0.5;
            draw_rectangle(
                x - 8.0 * ui,
                y - 8.0 * ui,
                pw + 16.0 * ui,
                ph + 16.0 * ui,
                PANEL,
            );
            draw_texture_ex(
                tex,
                x,
                y,
                crate::render::theme_tint(&entry.theme),
                DrawTextureParams {
                    dest_size: Some(vec2(pw, ph)),
                    ..Default::default()
                },
            );
            let focus = (self.setup_sel < order.len()).then(|| order[self.setup_sel]);
            draw_seat_markers(
                scenario,
                draft,
                Rect::new(x, y, pw, ph),
                draft.seat_choice,
                focus,
                ui,
            );
        }

        let on_dial = self.setup_sel < order.len() && self.setup_cell > 0;
        let hint = if self.setup_sel == order.len() {
            "Enter starts the match - Esc back"
        } else if on_dial {
            "Enter cycles the dial - Left/Right move - Esc back"
        } else {
            "Enter takes this seat - Left/Right reach the dials - Esc back"
        };
        let hdims = measure_text(hint, None, (16.0 * ui) as u16, 1.0);
        draw_text(
            hint,
            (view.x - hdims.width) * 0.5,
            view.y - 20.0 * ui,
            16.0 * ui,
            DIM,
        );
    }

    /// The debug protocol's stable mode name for the current step —
    /// unchanged across redesigns, so automation scripts keep their
    /// footing.
    pub fn mode_name(&self) -> &'static str {
        match self.step {
            Step::Map => "main_menu",
            Step::Difficulty => "difficulty_menu",
            Step::Personality => "personality_menu",
            Step::Faction => "faction_menu",
            Step::Setup => "match_setup",
        }
    }

    /// The (title, items, selected) surface QueryUi reports — the
    /// custom screens speak the same protocol the row menus do.
    pub fn ui_surface(&self, draft: &NewMatchDraft) -> (String, Vec<String>, usize) {
        match self.step {
            Step::Map => (
                "OXIDE".to_string(),
                self.entries.iter().map(|e| e.label.clone()).collect(),
                self.browser.selected,
            ),
            Step::Setup => {
                let mut items: Vec<String> = draft
                    .scenario
                    .as_deref()
                    .map(|sc| {
                        seat_display_order(sc)
                            .into_iter()
                            .map(|seat| {
                                let spec = &sc.players[seat];
                                let plan = draft.seats[seat];
                                if seat == draft.seat_choice {
                                    format!(
                                        "{}. {} (you) · {}",
                                        seat + 1,
                                        spec.name,
                                        FACTION_CHIP_ITEMS[plan.faction_choice]
                                    )
                                } else {
                                    format!(
                                        "{}. {} · {} · {} · {}",
                                        seat + 1,
                                        spec.name,
                                        DIFFICULTY_ITEMS[plan.level_choice],
                                        PERSONALITY_ITEMS[plan.personality_choice],
                                        FACTION_CHIP_ITEMS[plan.faction_choice]
                                    )
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                items.push("Start match".to_string());
                ("MATCH SETUP".to_string(), items, self.setup_sel)
            }
            _ => (
                self.menu.title.clone(),
                self.menu.items.clone(),
                self.menu.selected,
            ),
        }
    }

    /// The subtitle under the quick-flow menus.
    pub fn subtitle(&self, _draft: &NewMatchDraft) -> &'static str {
        match self.step {
            Step::Map => "machines eating a dead world",
            Step::Difficulty => "how hard should it think?",
            Step::Personality => "how should it fight?",
            Step::Faction => "which roster do your machines run?",
            Step::Setup => "pick your seat; tune each opponent",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn press(key: Key) -> Vec<RawEvent> {
        vec![RawEvent::KeyDown { key }, RawEvent::KeyUp { key }]
    }

    fn drive(w: &mut Wizard, draft: &mut NewMatchDraft, key: Key) -> Out {
        let mut mouse = vec2(0.0, 0.0);
        let mut sounds = Vec::new();
        w.update(&press(key), &mut mouse, draft, &mut sounds)
            .expect("update")
    }

    /// Activates the browser's first entry (sections put duels first,
    /// so entry 0 is always a 1v1 — its own test pins that).
    fn pick_first_map(w: &mut Wizard, draft: &mut NewMatchDraft) {
        w.browser.selected = 0;
        assert_eq!(drive(w, draft, Key::Enter), Out::Stay);
    }

    #[test]
    fn the_wizard_walks_forward_and_back_without_forgetting() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        assert_eq!(w.step, Step::Map);

        pick_first_map(&mut w, &mut draft);
        assert_eq!(w.step, Step::Difficulty, "entry 0 is a duel: quick flow");
        assert!(draft.scenario.is_some(), "the draft holds the map");
        assert_eq!(w.menu.selected, 1, "Medium preselected from the draft");

        // Choose Hard, then back out — the answer must survive (it
        // lives in the per-seat vector now).
        drive(&mut w, &mut draft, Key::Down);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Personality);
        assert!(
            draft
                .seats
                .iter()
                .enumerate()
                .all(|(i, p)| i == draft.seat_choice || p.level_choice == 2),
            "the quick flow writes every AI seat's plan"
        );
        assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Stay);
        assert_eq!(w.step, Step::Difficulty);
        assert_eq!(w.menu.selected, 2, "Back re-offers Hard, not the default");

        // Forward to the end: the last answer launches.
        drive(&mut w, &mut draft, Key::Enter);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(w.step, Step::Faction);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Launch);
        assert_eq!(draft.faction_choice, 0);
    }

    #[test]
    fn escape_unwinds_to_home_and_mid_flow_steps_back_one() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Home);

        let mut w = Wizard::open(&draft);
        pick_first_map(&mut w, &mut draft);
        assert_eq!(w.step, Step::Difficulty);
        // The Back row steps one screen, to the map grid.
        let last = w.menu.items.len() - 1;
        w.menu.select(last);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Map, "Back walks one step");
        assert_eq!(
            w.browser.selected,
            w.entries
                .iter()
                .position(|e| e.path == draft.scenario_path)
                .unwrap(),
            "the grid re-offers the earlier pick, found by PATH"
        );
    }

    #[test]
    fn the_browser_leads_with_duels() {
        let w = Wizard::open(&NewMatchDraft::default());
        assert!(
            w.entries.first().is_some_and(|e| e.seats == 2),
            "entry 0 must be a 1v1: a first Play+Enter never launches a team match"
        );
        let seat_counts: Vec<usize> = w.entries.iter().map(|e| e.seats).collect();
        let mut sorted = seat_counts.clone();
        sorted.sort_unstable();
        assert_eq!(seat_counts, sorted, "sections ascend by format");
    }

    #[test]
    fn a_team_map_runs_the_setup_screen_and_reseats_without_permuting() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        let Some(team_entry) = w.entries.iter().position(|e| e.seats > 2) else {
            return; // no team maps discovered (bare checkout)
        };
        let seats = w.entries[team_entry].seats;
        w.browser.selected = team_entry;
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(w.step, Step::Setup, "team maps skip the quick flow");
        assert_eq!(draft.seats.len(), seats, "the per-seat vector re-derived");
        assert_eq!(w.setup_sel, seats, "Start preselected under the seat cards");

        // Walk to the second DISPLAY seat; Enter takes the chair
        // inline — no sub-screen.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Down);
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seat_choice, order[1], "the chair moved");
        assert_eq!(w.step, Step::Setup, "and the screen never left");

        // The first display seat's difficulty dial cycles in place:
        // Right reaches the dial, Enter cycles it twice.
        drive(&mut w, &mut draft, Key::Home);
        let first = order[0];
        drive(&mut w, &mut draft, Key::Right);
        drive(&mut w, &mut draft, Key::Enter);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[first].level_choice, 3,
            "Medium cycled twice lands on Expert"
        );
        // The cell column is sticky: walking down keeps the dial.
        drive(&mut w, &mut draft, Key::Down);
        drive(&mut w, &mut draft, Key::Enter);
        let second_ai = order
            .iter()
            .copied()
            .find(|s| *s != draft.seat_choice && *s != first)
            .unwrap_or(order[1]);
        let _ = second_ai; // which row it lands on depends on the chair
        // Personality: one more Right from the difficulty dial.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Right);
        drive(&mut w, &mut draft, Key::Right);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[first].personality_choice, 1,
            "Surprise me cycles to Turtle"
        );

        // End sits on Start; Enter launches.
        drive(&mut w, &mut draft, Key::End);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Launch);

        draft.set_scenario(Scenario::skirmish(), None);
        assert_eq!(draft.seats.len(), 2, "re-derived at the new width");
        assert!(draft.seat_choice < 2, "the chair clamped onto the board");
    }

    #[test]
    fn omitted_singleton_teams_keep_their_setup_card() {
        let mut scenario = Scenario::load("../scenarios/trident-plateau.json").expect("shipped");
        // Two explicit teammates then an omitted singleton — the shape
        // that used to derive two different surrogate keys and drop
        // the seat from the display order (Enter on Start then indexed
        // past the order and panicked).
        scenario.players[2].team = None;
        let order = seat_display_order(&scenario);
        assert_eq!(
            order.len(),
            scenario.players.len(),
            "every seat keeps a card"
        );
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..scenario.players.len()).collect::<Vec<_>>());
        let layout = setup_layout(&scenario, 0, vec2(1280.0, 800.0), 1.0);
        assert_eq!(layout.seats.len(), scenario.players.len());

        // An authored id inside the old surrogate range must not
        // swallow an omitted seat either.
        scenario.players[2].team = Some(202);
        let order = seat_display_order(&scenario);
        assert_eq!(order.len(), scenario.players.len());
    }

    #[test]
    fn the_faction_chip_cycles_on_every_card_including_yours() {
        let mut draft = NewMatchDraft::default();
        let mut w = Wizard::open(&draft);
        let Some(team_entry) = w.entries.iter().position(|e| e.seats > 2) else {
            return; // no team maps discovered (bare checkout)
        };
        w.browser.selected = team_entry;
        drive(&mut w, &mut draft, Key::Enter);
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        assert_eq!(order[0], draft.seat_choice, "the human opens in seat 0");

        // Your own card: Right skips the dead AI dials straight to the
        // faction chip; Enter cycles Auto to Ferrous.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Right);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[draft.seat_choice].faction_choice, 1,
            "your own chip cycled to Ferrous"
        );
        assert_eq!(
            w.step,
            Step::Setup,
            "cycling a chip never leaves the screen"
        );

        // The sticky column carries the faction cell onto an AI card.
        drive(&mut w, &mut draft, Key::Down);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seats[order[1]].faction_choice, 1);
        // Left from the faction chip reaches the AI-only personality
        // dial — the full chip row exists on an opponent's card.
        drive(&mut w, &mut draft, Key::Left);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[order[1]].personality_choice, 1,
            "cell 2 is the personality dial"
        );
    }

    #[test]
    fn the_setup_layout_fits_the_smallest_supported_window() {
        // Eight seats at 640x400: the old fixed card floor pushed the
        // Start button past the window bottom with no way to scroll.
        let scenario = Scenario::load("../scenarios/compass-grand.json").expect("shipped");
        let layout = setup_layout(&scenario, 0, vec2(640.0, 400.0), 1.0);
        assert_eq!(layout.seats.len(), 8);
        for card in &layout.seats {
            assert!(card.h >= 16.0, "cards stay clickable, not vestigial");
            assert!(
                card.y + card.h <= 400.0,
                "every card stays on screen (card at y={})",
                card.y
            );
        }
        assert!(
            layout.start.y + layout.start.h <= 400.0,
            "Start stays reachable (ends at {})",
            layout.start.y + layout.start.h
        );
        for cells in &layout.cells {
            for (card, chip) in layout.seats.iter().zip(cells.iter().skip(1)) {
                if chip.w > 0.0 {
                    assert!(chip.h <= card.h, "chips never overflow their card");
                }
            }
        }
    }

    #[test]
    fn the_setup_layout_never_overlaps_its_own_parts() {
        let scenario = Scenario::load("../scenarios/compass-grand.json").expect("shipped");
        let layout = setup_layout(&scenario, 0, vec2(1280.0, 800.0), 1.0);
        assert_eq!(layout.seats.len(), 8);
        for pair in layout.seats.windows(2) {
            assert!(
                pair[0].y + pair[0].h <= pair[1].y + 0.01,
                "seat cards stack without overlap"
            );
        }
        let last = layout.seats.last().unwrap();
        assert!(
            layout.start.y >= last.y + last.h,
            "Start sits under the cards"
        );
        assert!(
            layout.preview.x >= last.x + last.w,
            "the preview never crosses the cards"
        );
        assert!(
            layout.start.y + layout.start.h <= 800.0,
            "everything fits an 800px window"
        );
        let order = seat_display_order(&scenario);
        for (pos, cells) in layout.cells.iter().enumerate() {
            let card = layout.seats[pos];
            for r in cells.iter().filter(|r| r.w > 0.0) {
                assert!(
                    r.x >= card.x - 0.01
                        && r.y >= card.y - 0.01
                        && r.x + r.w <= card.x + card.w + 0.01
                        && r.y + r.h <= card.y + card.h + 0.01,
                    "cell rects nest inside their card"
                );
            }
            if order[pos] == 0 {
                assert!(cells[1].w == 0.0, "the human's card has no dials");
            } else {
                assert!(cells[1].w > 0.0 && cells[2].w > 0.0, "AI cards carry dials");
            }
        }
    }

    #[test]
    fn seat_anchors_reads_the_authored_digits() {
        let map: Vec<String> = vec!["####".into(), "#1.#".into(), "#.2#".into()];
        assert_eq!(seat_anchors(&map), vec![(0, (1, 1)), (1, (2, 2))]);
    }
}
