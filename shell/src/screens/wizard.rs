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

/// One AI seat's dials in the draft.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeatPlan {
    /// Difficulty row (indexes `Level::LADDER`).
    pub level_choice: usize,
    /// Personality row (feeds [`personality_knob`]).
    pub personality_choice: usize,
}

impl Default for SeatPlan {
    fn default() -> Self {
        Self {
            level_choice: 1, // Medium is the fair default
            personality_choice: 0,
        }
    }
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
    /// Team-map match setup: seat cards by team, Start, a live map.
    Setup,
    /// One seat's dials (take the chair, difficulty, personality).
    SeatDetail(usize),
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
    /// Setup card armed by a press (activation on release inside).
    setup_pressed: Option<usize>,
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

fn seat_detail_menu(draft: &NewMatchDraft, i: usize) -> Menu {
    let plan = draft.seats[i];
    let rows = vec![
        "Take this seat".to_string(),
        format!("Difficulty: {}", DIFFICULTY_ITEMS[plan.level_choice]),
        format!(
            "Personality: {}",
            PERSONALITY_ITEMS[plan.personality_choice]
        ),
        "Back".to_string(),
    ];
    let scenario = draft.scenario.as_ref().expect("setup wants a map");
    Menu::new(scenario.players[i].name.to_uppercase(), rows)
}

/// Seats in DISPLAY order: grouped by team (dense ids, first
/// appearance), seat order within — the setup screen's visual order
/// and its cursor's walking order are the same list.
pub fn seat_display_order(scenario: &Scenario) -> Vec<usize> {
    let mut teams: Vec<u8> = Vec::new();
    for p in &scenario.players {
        let t = p.team.unwrap_or(200 + teams.len() as u8);
        if !teams.contains(&t) {
            teams.push(t);
        }
    }
    let mut order: Vec<usize> = Vec::new();
    for team in &teams {
        for (i, p) in scenario.players.iter().enumerate() {
            let t = p.team.unwrap_or(200 + i as u8);
            if t == *team && !order.contains(&i) {
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
    /// The Start button.
    pub start: Rect,
    /// Where the map preview draws.
    pub preview: Rect,
}

/// Computes [`SetupLayout`].
pub fn setup_layout(scenario: &Scenario, view: Vec2, ui: f32) -> SetupLayout {
    let order = seat_display_order(scenario);
    let n = order.len();
    let teams: Vec<u8> = {
        let mut seen = Vec::new();
        for &s in &order {
            let t = scenario.players[s].team.unwrap_or(200 + s as u8);
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen
    };
    let left_x = 56.0 * ui;
    let left_w = (view.x * 0.42).min(520.0 * ui);
    let top = 132.0 * ui;
    let bottom = view.y - 44.0 * ui;
    let heading_h = 26.0 * ui;
    let start_h = 46.0 * ui;
    let gap = 6.0 * ui;
    let avail = bottom - top - teams.len() as f32 * heading_h - start_h - 24.0 * ui;
    let card_h = ((avail / n.max(1) as f32) - gap).clamp(34.0 * ui, 56.0 * ui);

    let mut headings = Vec::new();
    let mut seats = vec![Rect::new(0.0, 0.0, 0.0, 0.0); n];
    let mut y = top;
    let mut last_team: Option<u8> = None;
    for (pos, &seat) in order.iter().enumerate() {
        let team = scenario.players[seat].team.unwrap_or(200 + seat as u8);
        if last_team != Some(team) {
            let label = format!(
                "TEAM {}",
                teams.iter().position(|t| *t == team).unwrap() + 1
            );
            headings.push((label, Rect::new(left_x, y, left_w, heading_h)));
            last_team = Some(team);
            y += heading_h;
        }
        seats[pos] = Rect::new(left_x, y, left_w, card_h);
        y += card_h + gap;
    }
    let start = Rect::new(left_x, y + 12.0 * ui, 240.0 * ui, start_h);
    let px = left_x + left_w + 28.0 * ui;
    let preview = Rect::new(px, top, view.x - px - 40.0 * ui, bottom - top - 20.0 * ui);
    SetupLayout {
        headings,
        seats,
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
/// in faction color, a white ring for the human's chair, an accent
/// ring for the focused seat — the list and the map read as one thing.
pub fn draw_seat_markers(
    scenario: &Scenario,
    rect: Rect,
    seat_choice: usize,
    focus_seat: Option<usize>,
    ui: f32,
) {
    let map_w = scenario.map.first().map_or(1, |r| r.chars().count()) as f32;
    let map_h = scenario.map.len() as f32;
    for (seat, (ax, ay)) in seat_anchors(&scenario.map) {
        let Some(spec) = scenario.players.get(seat) else {
            continue;
        };
        // Foundry anchors are the 2x2's top-left; mark its center.
        let px = rect.x + (ax as f32 + 1.0) / map_w * rect.w;
        let py = rect.y + (ay as f32 + 1.0) / map_h * rect.h;
        let accent = crate::render::faction_accent(spec.faction);
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
                self.setup_pressed = None;
                Menu::new("MATCH SETUP", Vec::new())
            }
            Step::SeatDetail(i) => seat_detail_menu(draft, i),
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
            Step::Difficulty | Step::Personality | Step::Faction | Step::SeatDetail(_) => {
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
                    Step::SeatDetail(i) => {
                        if escaped || choice == Some(3) {
                            self.goto(Step::Setup, draft);
                            // Land the cursor back on the seat it came
                            // from, not on Start.
                            let order = draft
                                .scenario
                                .as_deref()
                                .map(seat_display_order)
                                .unwrap_or_default();
                            if let Some(pos) = order.iter().position(|s| *s == i) {
                                self.setup_sel = pos;
                            }
                        } else if let Some(c) = choice {
                            match c {
                                0 => {
                                    // Seat choice never permutes seats —
                                    // parity carries factions and teams —
                                    // it moves the human's chair.
                                    draft.seat_choice = i;
                                    self.goto(Step::Setup, draft);
                                }
                                1 => {
                                    let plan = &mut draft.seats[i];
                                    plan.level_choice =
                                        (plan.level_choice + 1) % DIFFICULTY_ITEMS.len();
                                    self.goto(Step::SeatDetail(i), draft);
                                    self.menu.select(1);
                                }
                                2 => {
                                    let plan = &mut draft.seats[i];
                                    plan.personality_choice =
                                        (plan.personality_choice + 1) % PERSONALITY_ITEMS.len();
                                    self.goto(Step::SeatDetail(i), draft);
                                    self.menu.select(2);
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => unreachable!("outer match routed these"),
                }
            }
        }
        Ok(Out::Stay)
    }

    /// The setup screen's input: a linear cursor over the seat cards
    /// (display order) and the Start button, plus card clicks.
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
        let layout = setup_layout(scenario, view, ui);
        let slot_at = |p: Vec2| {
            layout
                .seats
                .iter()
                .position(|r| r.contains(p))
                .or_else(|| layout.start.contains(p).then_some(start_index))
        };
        let mut activate: Option<usize> = None;
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
                RawEvent::KeyDown { key: Key::Home } => self.setup_sel = 0,
                RawEvent::KeyDown { key: Key::End } => self.setup_sel = start_index,
                RawEvent::KeyDown { key: Key::Enter } => activate = Some(self.setup_sel),
                RawEvent::MouseMove { x, y } => *mouse = vec2(x, y),
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    self.setup_pressed = slot_at(vec2(x, y));
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let released = slot_at(vec2(x, y));
                    let armed = self.setup_pressed.take();
                    if let (Some(a), Some(r)) = (armed, released)
                        && a == r
                    {
                        self.setup_sel = a;
                        activate = Some(a);
                    }
                }
                _ => {}
            }
        }
        if let Some(slot) = activate {
            sounds.push((SoundKind::Click, None));
            if slot == start_index {
                return Some(Out::Launch);
            }
            let seat = order[slot];
            self.goto(Step::SeatDetail(seat), draft);
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
        let layout = setup_layout(scenario, view, ui);
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
            let accent = crate::render::faction_accent(spec.faction);
            let cy = rect.y + rect.h * 0.5;
            let chip_x = rect.x + 22.0 * ui;
            if seat == draft.seat_choice {
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
            let name_y = if rect.h > 44.0 * ui {
                rect.y + rect.h * 0.44
            } else {
                cy + 5.0 * ui
            };
            draw_text(
                &spec.name,
                rect.x + 44.0 * ui,
                name_y,
                16.0 * ui,
                ITEM_COLOR,
            );
            let plan = draft.seats[seat];
            let sub = if seat == draft.seat_choice {
                "You".to_string()
            } else {
                format!(
                    "{} · {}",
                    DIFFICULTY_ITEMS[plan.level_choice], PERSONALITY_ITEMS[plan.personality_choice]
                )
            };
            if rect.h > 44.0 * ui {
                draw_text(
                    &sub,
                    rect.x + 44.0 * ui,
                    rect.y + rect.h * 0.82,
                    13.0 * ui,
                    DIM,
                );
            } else {
                let sdims = measure_text(&sub, None, (13.0 * ui) as u16, 1.0);
                draw_text(
                    &sub,
                    rect.x + rect.w - sdims.width - 12.0 * ui,
                    cy + 4.0 * ui,
                    13.0 * ui,
                    DIM,
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
                Rect::new(x, y, pw, ph),
                draft.seat_choice,
                focus,
                ui,
            );
        }

        let hint = "Arrows select - Enter open - Esc back - or click";
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
            Step::SeatDetail(_) => "seat_menu",
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
                                if seat == draft.seat_choice {
                                    format!("{}. {} (you)", seat + 1, spec.name)
                                } else {
                                    let plan = draft.seats[seat];
                                    format!(
                                        "{}. {} · {} · {}",
                                        seat + 1,
                                        spec.name,
                                        DIFFICULTY_ITEMS[plan.level_choice],
                                        PERSONALITY_ITEMS[plan.personality_choice]
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
            Step::SeatDetail(_) => "this seat's dials",
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

        // Walk to the second DISPLAY seat and take the chair.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Down);
        drive(&mut w, &mut draft, Key::Enter);
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        let opened = order[1];
        assert_eq!(w.step, Step::SeatDetail(opened));
        w.menu.select(0);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(draft.seat_choice, opened, "the chair moved");
        assert_eq!(w.step, Step::Setup);

        // Open the first display seat and cycle its difficulty twice.
        drive(&mut w, &mut draft, Key::Home);
        drive(&mut w, &mut draft, Key::Enter);
        let first = order[0];
        assert_eq!(w.step, Step::SeatDetail(first));
        w.menu.select(1);
        drive(&mut w, &mut draft, Key::Enter);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(
            draft.seats[first].level_choice, 3,
            "the dial cycles in place"
        );
        // Back from the detail returns the cursor to that seat.
        w.menu.select(3);
        drive(&mut w, &mut draft, Key::Enter);
        assert_eq!(w.step, Step::Setup);
        assert_eq!(w.setup_sel, 0, "Back lands on the seat it came from");

        // End sits on Start; Enter launches.
        drive(&mut w, &mut draft, Key::End);
        assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Launch);

        draft.set_scenario(Scenario::skirmish(), None);
        assert_eq!(draft.seats.len(), 2, "re-derived at the new width");
        assert!(draft.seat_choice < 2, "the chair clamped onto the board");
    }

    #[test]
    fn the_setup_layout_never_overlaps_its_own_parts() {
        let scenario = Scenario::load("../scenarios/compass-grand.json").expect("shipped");
        let layout = setup_layout(&scenario, vec2(1280.0, 800.0), 1.0);
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
    }

    #[test]
    fn seat_anchors_reads_the_authored_digits() {
        let map: Vec<String> = vec!["####".into(), "#1.#".into(), "#.2#".into()];
        assert_eq!(seat_anchors(&map), vec![(0, (1, 1)), (1, (2, 2))]);
    }
}
