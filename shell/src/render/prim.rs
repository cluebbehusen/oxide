//! macroquad's shape calls take loose scalars, so every call site explodes a
//! `Rect` or `Vec2` back into its fields. These take the value whole.

use macroquad::prelude::{
    Color, Rect, Vec2, draw_circle, draw_circle_lines, draw_line, draw_rectangle,
    draw_rectangle_lines,
};

pub(crate) fn fill_rect(rect: Rect, color: Color) {
    draw_rectangle(rect.x, rect.y, rect.w, rect.h, color);
}

pub(crate) fn stroke_rect(rect: Rect, thickness: f32, color: Color) {
    draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, thickness, color);
}

pub(crate) fn line_between(from: Vec2, to: Vec2, thickness: f32, color: Color) {
    draw_line(from.x, from.y, to.x, to.y, thickness, color);
}

pub(crate) fn fill_circle(center: Vec2, radius: f32, color: Color) {
    draw_circle(center.x, center.y, radius, color);
}

pub(crate) fn stroke_circle(center: Vec2, radius: f32, thickness: f32, color: Color) {
    draw_circle_lines(center.x, center.y, radius, thickness, color);
}
