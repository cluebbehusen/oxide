//! Weapon contact and recipient material at one cosmetic impact origin.

use crate::game::{HitSurface, ShotStyle};
use macroquad::prelude::*;
use oxide_sim::ProjectileKind;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    Rail,
    Orb,
    Shell,
    Mortar,
    Rocket,
    Bomb,
}

impl Family {
    pub(super) fn direct(style: ShotStyle) -> Option<Self> {
        match style {
            ShotStyle::Rail => Some(Self::Rail),
            ShotStyle::ForgeSpot => Some(Self::Orb),
            ShotStyle::Mortar => Some(Self::Mortar),
            _ => None,
        }
    }

    pub(super) fn payload(kind: ProjectileKind) -> Self {
        match kind {
            ProjectileKind::Shell => Self::Shell,
            ProjectileKind::Missile => Self::Rocket,
            ProjectileKind::Bomb => Self::Bomb,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Recipient {
    Ground,
    Machine,
    Structure,
    Air,
}

impl Recipient {
    pub(super) fn of(surface: Option<HitSurface>) -> Self {
        match surface {
            Some(HitSurface::Building(_)) => Self::Structure,
            Some(HitSurface::Unit(hit)) if hit.airborne => Self::Air,
            Some(HitSurface::Unit(_)) => Self::Machine,
            None => Self::Ground,
        }
    }
}

pub(super) struct Contact {
    pub family: Family,
    pub recipient: Recipient,
    pub at: Vec2,
    pub direction: Vec2,
    pub radius: f32,
    pub age: f32,
    pub seed: u32,
}

fn block(center: Vec2, size: Vec2, pixel: f32, color: Color) {
    let origin = ((center - size * 0.5) / pixel).round() * pixel;
    draw_rectangle(
        origin.x,
        origin.y,
        (size.x / pixel).round().max(1.) * pixel,
        (size.y / pixel).round().max(1.) * pixel,
        color,
    );
}

fn noise(seed: u32, i: u32) -> f32 {
    seed.wrapping_add(i.wrapping_mul(40_503))
        .wrapping_mul(2_654_435_761)
        .rotate_left(13) as f32
        / u32::MAX as f32
}

pub(super) fn seed(at: Vec2, tick: u64) -> u32 {
    at.x.to_bits() ^ at.y.to_bits().rotate_left(17) ^ tick as u32
}

pub(super) fn draw(contact: Contact, zoom: f32) {
    let Contact {
        family,
        recipient,
        at,
        direction,
        radius,
        age,
        seed,
    } = contact;
    if age < 0. {
        return;
    }
    let pixel = (zoom / 32.).max(1.);
    let explosive = matches!(
        family,
        Family::Shell | Family::Mortar | Family::Rocket | Family::Bomb
    );
    let radius = radius.clamp(0.35, 2.2);
    let scale = zoom * radius;
    let incoming = direction.normalize_or_zero();
    let outward = if incoming == Vec2::ZERO {
        Vec2::from_angle(noise(seed, 9) * std::f32::consts::TAU)
    } else {
        -incoming
    };
    let tangent = vec2(-outward.y, outward.x);
    let heat_life = match family {
        Family::Rail => 0.13,
        Family::Orb => 0.10,
        Family::Shell | Family::Mortar => 0.16,
        Family::Rocket => 0.28,
        Family::Bomb => 0.20,
    };
    let heat = (1. - age / heat_life).clamp(0., 1.);
    if heat > 0. {
        let size = scale
            * match family {
                Family::Rail => 0.17,
                Family::Orb => 0.12,
                Family::Rocket => 0.30,
                _ => 0.23,
            };
        if family == Family::Rail {
            let seam = at + incoming * zoom * 0.07;
            draw_line(
                at.x,
                at.y,
                seam.x,
                seam.y,
                (pixel * 2.).max(size * 0.18),
                Color::new(0.98, 0.81, 0.53, heat),
            );
        } else {
            for (offset, shape) in [
                (vec2(-0.20, 0.0), vec2(0.70, 0.22)),
                (vec2(0.06, -0.16), vec2(0.38, 0.65)),
                (vec2(0.25, -0.06), vec2(0.32, 0.30)),
            ] {
                block(
                    at + offset * size,
                    shape * size,
                    pixel,
                    Color::new(0.68, 0.29, 0.11, heat * 0.78),
                );
            }
        }
        block(
            at,
            vec2(size * 0.45, size * 0.30),
            pixel,
            Color::new(1., 0.93, 0.73, heat),
        );
        if family == Family::Rocket {
            for i in 0..3 {
                let offset = vec2(noise(seed, i) - 0.5, -noise(seed, i + 4) * 0.7);
                block(
                    at + offset * size,
                    Vec2::splat(size * 0.65),
                    pixel,
                    Color::new(0.89, 0.40, 0.12, heat * 0.7),
                );
            }
        }
    }

    let fragment_life = if explosive { 0.42 } else { 0.20 };
    if age < fragment_life && !super::reduced_motion() {
        let t = age / fragment_life;
        for i in 0..if explosive { 7 } else { 4 } {
            let spread = noise(seed, i) * 2. - 1.;
            let direction = if recipient == Recipient::Ground || family == Family::Bomb {
                Vec2::from_angle(noise(seed, i + 17) * std::f32::consts::TAU)
            } else {
                (outward + tangent * spread * 1.7).normalize_or_zero()
            };
            let distance = scale * (0.16 + noise(seed, i + 8) * 0.44) * (1. - (1. - t).powi(2));
            let lift = if recipient == Recipient::Air {
                0.
            } else {
                scale * 0.22 * 4. * t * (1. - t)
            };
            let position = at + direction * distance - vec2(0., lift);
            let color = match recipient {
                Recipient::Ground => Color::new(0.38, 0.31, 0.23, 1. - t),
                Recipient::Structure => Color::new(0.62, 0.62, 0.55, 1. - t),
                _ => Color::new(0.86, 0.77, 0.54, 1. - t),
            };
            let size = if recipient == Recipient::Structure && explosive {
                vec2(0.10, 0.04)
            } else {
                vec2(0.045, 0.03)
            } * scale;
            block(position, size, pixel, color);
        }
    }

    if !explosive || recipient == Recipient::Air {
        return;
    }
    let dust_age = age - 0.07;
    if (0.0..0.85).contains(&dust_age) {
        let t = dust_age / 0.85;
        for i in 0..7 {
            let angle = noise(seed, i + 31) * std::f32::consts::TAU;
            let travel = radius * zoom * (0.08 + t * 0.62);
            let offset = vec2(angle.cos(), angle.sin() * 0.50) * travel;
            let size = scale * (0.10 + t * 0.15);
            block(
                at + offset,
                vec2(size * 1.5, size * 0.65),
                pixel,
                Color::new(0.34, 0.30, 0.24, (1. - t) * 0.27),
            );
        }
    }
    if family == Family::Bomb && recipient == Recipient::Ground && age < 0.24 {
        let t = age / 0.24;
        for i in 0..3 {
            let offset = vec2(
                (noise(seed, i + 50) - 0.5) * scale * 0.20,
                -scale * t * (0.35 + noise(seed, i + 53) * 0.4),
            );
            block(
                at + offset,
                vec2(scale * 0.08, scale * 0.16),
                pixel,
                Color::new(0.46, 0.36, 0.24, (1. - t) * 0.8),
            );
        }
    }
    if family == Family::Rocket && (0.08..1.10).contains(&age) {
        let t = (age - 0.08) / 1.02;
        for i in 0..3 {
            let offset = vec2(
                (noise(seed, i + 61) - 0.5) * scale * 0.25,
                -scale * (t * 0.60 + i as f32 * 0.08),
            );
            block(
                at + offset,
                vec2(scale * (0.14 + t * 0.13), scale * 0.18),
                pixel,
                Color::new(0.13, 0.125, 0.12, (1. - t) * 0.40),
            );
        }
    }
}
