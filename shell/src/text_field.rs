//! A one-line text field for the menus: a title, the field, and Cancel
//! plus a confirm button.

use crate::game::SoundKind;
use crate::menu::Menu;
use crate::numeric;
use crate::press::{Fed, Press};
use macroquad::prelude::{
    Color, Rect, Vec2, draw_rectangle, draw_rectangle_lines, draw_text, measure_text,
};
use oxide_protocol::{Key, RawEvent};

/// The field's pointer targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Field,
    Confirm,
    Cancel,
}

/// Where the field's face sits. It hugs the top of the window: an
/// on-screen keyboard covers roughly the bottom half in landscape, and
/// the platform does not resize the canvas around it.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub title_y: f32,
    pub hint_y: f32,
    pub field: Rect,
    pub cancel: Rect,
    pub confirm: Rect,
}

impl Layout {
    fn zone_at(&self, p: Vec2) -> Option<Zone> {
        [
            (self.field, Zone::Field),
            (self.confirm, Zone::Confirm),
            (self.cancel, Zone::Cancel),
        ]
        .into_iter()
        .find(|(rect, _)| rect.contains(p))
        .map(|(_, zone)| zone)
    }
}

pub fn layout(view: Vec2, s: f32) -> Layout {
    let width = (420.0 * s).min(view.x - 32.0 * s);
    let x = (view.x - width) * 0.5;
    let field = Rect::new(x, 92.0 * s, width, crate::layout::MIN_TOUCH_TARGET * s);
    let gap = 12.0 * s;
    let button_w = (width - gap) * 0.5;
    let buttons_y = field.y + field.h + gap;
    Layout {
        title_y: 56.0 * s,
        hint_y: 80.0 * s,
        field,
        cancel: Rect::new(x, buttons_y, button_w, crate::layout::MIN_TOUCH_TARGET * s),
        confirm: Rect::new(
            x + button_w + gap,
            buttons_y,
            button_w,
            crate::layout::MIN_TOUCH_TARGET * s,
        ),
    }
}

/// What a frame did with the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Still editing.
    Stay,
    /// Enter or the confirm button, with the trimmed, non-empty value.
    Commit(String),
    /// Escape or Cancel.
    Cancel,
}

/// The field: its value and the presses on its buttons.
pub struct TextField {
    title: &'static str,
    confirm: &'static str,
    value: String,
    max: usize,
    press: Press<Zone>,
    /// A tap on the field asked for the on-screen keyboard again (the
    /// player may have dismissed it); the frame loop takes this.
    keyboard_request: bool,
}

impl TextField {
    /// A field titled `title` whose confirm button reads `confirm`,
    /// prefilled with `value` and holding at most `max` characters.
    pub fn new(title: &'static str, confirm: &'static str, value: &str, max: usize) -> Self {
        // Typing only ever adds printable ASCII; a prefill holds to the
        // same alphabet (plus the middle dot saves use) so a map name
        // cannot smuggle glyphs past the ingest filter.
        let mut value: String = value.chars().take(max).collect();
        value.retain(|c| c.is_ascii() || c == '\u{b7}');
        Self {
            title,
            confirm,
            value,
            max,
            press: Press::default(),
            keyboard_request: false,
        }
    }

    /// The field as a one-row menu, for the UI report. The caret is a
    /// static underscore, never a blink, so reduced motion holds and the
    /// shots suite stays deterministic.
    pub fn menu(&self) -> Menu {
        Menu::new(self.title, vec![format!("{}_", self.value)])
    }

    /// Consumes a request to raise the on-screen keyboard again.
    pub fn take_keyboard_request(&mut self) -> bool {
        std::mem::take(&mut self.keyboard_request)
    }

    /// Applies a frame's events. The field owns the frame: typed
    /// characters edit, Backspace deletes, Enter or the confirm button
    /// commits, Escape or Cancel abandons.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        sounds: &mut Vec<(SoundKind, Option<Vec2>)>,
    ) -> Edit {
        let layout = layout(crate::render::viewport(), crate::render::ui_scale());
        let mut pressed = None;
        for event in events {
            if let Fed::Activated(zone) = self.press.feed(event, |p, _| layout.zone_at(p)) {
                pressed = Some(zone);
            }
        }
        if pressed == Some(Zone::Field) {
            self.keyboard_request = true;
        }
        for event in events {
            match *event {
                RawEvent::Text { ch } if self.value.chars().count() < self.max => {
                    self.value.push(ch);
                }
                RawEvent::KeyDown {
                    key: Key::Backspace,
                } => {
                    self.value.pop();
                }
                _ => {}
            }
        }
        let key = |wanted: Key| {
            events
                .iter()
                .any(|e| matches!(e, RawEvent::KeyDown { key } if *key == wanted))
        };
        if pressed == Some(Zone::Confirm) || key(Key::Enter) {
            let value = self.value.trim();
            if value.is_empty() {
                sounds.push((SoundKind::Denied, None));
            } else {
                return Edit::Commit(value.to_owned());
            }
        }
        if pressed == Some(Zone::Cancel) || key(Key::Escape) {
            self.press.cancel();
            return Edit::Cancel;
        }
        Edit::Stay
    }

    /// Draws the title, `hint` under it in `hint_color`, the field, and
    /// its buttons. Coaching passes a faded color; a notice such as a
    /// failure passes a plain one, since warnings never wait.
    pub fn draw(&self, hint: &str, hint_color: Color, mouse: Vec2) {
        let s = crate::render::ui_scale();
        let view = crate::render::viewport();
        let layout = layout(view, s);
        let title_size = 48.0 * s;
        let dims = measure_text(self.title, None, numeric::font_size(title_size), 1.0);
        draw_text(
            self.title,
            (view.x - dims.width) * 0.5,
            layout.title_y,
            title_size,
            crate::theme::TEXT_TITLE,
        );
        let hint_size = 18.0 * s;
        let dims = measure_text(hint, None, numeric::font_size(hint_size), 1.0);
        draw_text(
            hint,
            (view.x - dims.width) * 0.5,
            layout.hint_y,
            hint_size,
            hint_color,
        );
        let field = layout.field;
        draw_rectangle(
            field.x,
            field.y,
            field.w,
            field.h,
            crate::theme::SURFACE_CARD,
        );
        draw_rectangle_lines(
            field.x,
            field.y,
            field.w,
            field.h,
            2.0 * s,
            crate::theme::TEXT_ACCENT,
        );
        let text = format!("{}_", self.value);
        let room = field.w - 24.0 * s;
        let mut size = 22.0 * s;
        let width = measure_text(&text, None, numeric::font_size(size), 1.0).width;
        if width > room {
            size = (size * room / width).max(10.0);
        }
        draw_text(
            &text,
            field.x + 12.0 * s,
            field.y + field.h * 0.66,
            size,
            crate::theme::TEXT_PRIMARY,
        );
        crate::button::draw(layout.cancel, "CANCEL", layout.cancel.contains(mouse), s);
        crate::button::draw(
            layout.confirm,
            self.confirm,
            layout.confirm.contains(mouse),
            s,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn feed(field: &mut TextField, events: &[RawEvent]) -> (Edit, Vec<SoundKind>) {
        let mut sounds = Vec::new();
        let edit = field.update(events, &mut sounds);
        (edit, sounds.into_iter().map(|(kind, _)| kind).collect())
    }

    fn text(value: &str) -> Vec<RawEvent> {
        value.chars().map(|ch| RawEvent::Text { ch }).collect()
    }

    fn key(key: Key) -> [RawEvent; 1] {
        [RawEvent::KeyDown { key }]
    }

    #[test]
    fn the_field_edits_caps_and_commits_a_trimmed_value() {
        let mut field = TextField::new("JOIN MATCH", "JOIN", "ab\u{e9}c", 5);
        assert_eq!(field.menu().items, vec!["abc_"], "the prefill is filtered");
        feed(&mut field, &text("def"));
        assert_eq!(field.menu().items, vec!["abcde_"], "the cap refuses more");
        feed(&mut field, &key(Key::Backspace));
        feed(&mut field, &text(" "));
        assert_eq!(
            feed(&mut field, &key(Key::Enter)).0,
            Edit::Commit("abcd".to_owned())
        );
    }

    #[test]
    fn a_blank_field_refuses_to_commit_and_escape_cancels() {
        let mut field = TextField::new("JOIN MATCH", "JOIN", "  ", 10);
        let (edit, sounds) = feed(&mut field, &key(Key::Enter));
        assert_eq!(edit, Edit::Stay);
        assert_eq!(sounds, vec![SoundKind::Denied]);
        assert_eq!(feed(&mut field, &key(Key::Escape)).0, Edit::Cancel);
    }

    #[test]
    fn the_face_stays_above_an_ipad_keyboard() {
        for view in [
            vec2(1133.0, 744.0),
            vec2(1180.0, 820.0),
            vec2(1194.0, 834.0),
            vec2(1366.0, 1024.0),
        ] {
            for s in [1.0, 1.25, 1.5] {
                let layout = layout(view, s);
                for rect in [layout.field, layout.cancel, layout.confirm] {
                    assert!(
                        rect.y + rect.h <= view.y * 0.45,
                        "{rect:?} reaches the keyboard at {view} ui {s}"
                    );
                }
            }
        }
        let small = vec2(640.0, 400.0);
        let layout = layout(small, 1.0);
        for rect in [layout.field, layout.cancel, layout.confirm] {
            assert!(rect.x >= 0.0 && rect.x + rect.w <= small.x);
            assert!(rect.y >= layout.hint_y && rect.y + rect.h <= small.y);
        }
        assert!(!layout.cancel.overlaps(&layout.confirm));
        assert!(!layout.field.overlaps(&layout.confirm));
    }
}
