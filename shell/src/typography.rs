//! Shared interface lettering for short labels and body copy.

use crate::numeric;
use macroquad::prelude::*;
use std::cell::RefCell;

pub(crate) fn entity_name(name: &str) -> String {
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

/// `text` with its first letter capitalized. Refusal reasons are
/// lowercase fragments so they also read inside longer messages; a
/// surface that shows one alone starts it as a sentence.
pub(crate) fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

thread_local! {
    static DISPLAY: RefCell<Option<Font>> = const { RefCell::new(None) };
}

pub(crate) fn install(font: Font) {
    DISPLAY.with(|current| *current.borrow_mut() = Some(font));
}

/// The size at which body-face `text` fits `width`: `size` when it
/// already fits, else shrunk to fit, but never below `floor`.
pub(crate) fn fit(text: &str, size: f32, width: f32, floor: f32) -> f32 {
    let measured = measure_text(text, None, numeric::font_size(size), 1.0).width;
    if measured <= width {
        size
    } else {
        (size * width / measured).max(floor)
    }
}

pub(crate) fn measure(text: &str, size: f32) -> TextDimensions {
    DISPLAY.with(|font| measure_text(text, font.borrow().as_ref(), numeric::font_size(size), 1.0))
}

pub(crate) fn draw(text: &str, x: f32, y: f32, size: f32, color: Color) {
    DISPLAY.with(|font| {
        draw_text_ex(
            text,
            x,
            y,
            TextParams {
                font: font.borrow().as_ref(),
                font_size: numeric::font_size(size),
                color,
                ..Default::default()
            },
        );
    });
}
