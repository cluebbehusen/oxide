//! Shared interface lettering for short labels and body copy.

use macroquad::prelude::*;
use std::cell::RefCell;

thread_local! {
    static DISPLAY: RefCell<Option<Font>> = const { RefCell::new(None) };
}

pub(crate) fn install(font: Font) {
    DISPLAY.with(|current| *current.borrow_mut() = Some(font));
}

pub(crate) fn measure(text: &str, size: f32) -> TextDimensions {
    DISPLAY.with(|font| measure_text(text, font.borrow().as_ref(), size as u16, 1.0))
}

pub(crate) fn draw(text: &str, x: f32, y: f32, size: f32, color: Color) {
    DISPLAY.with(|font| {
        draw_text_ex(
            text,
            x,
            y,
            TextParams {
                font: font.borrow().as_ref(),
                font_size: size as u16,
                color,
                ..Default::default()
            },
        );
    });
}
