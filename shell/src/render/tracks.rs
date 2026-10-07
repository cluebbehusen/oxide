//! Continuous exposed belt runs inside the existing track casings.

use macroquad::prelude::*;
use oxide_sim::UnitKind;

pub(crate) fn supported(kind: UnitKind) -> bool {
    !runs(kind).is_empty()
}

fn runs(kind: UnitKind) -> &'static [[f32; 4]] {
    match kind {
        UnitKind::Sentinel => &[[18., 52., 33., 111.], [95., 52., 110., 111.]],
        UnitKind::Warden => &[
            [11., 37., 30., 57.],
            [98., 37., 117., 57.],
            [11., 88., 30., 110.],
            [98., 88., 117., 110.],
        ],
        UnitKind::Lancer => &[[16., 74., 32., 111.], [96., 74., 112., 111.]],
        UnitKind::Harvester => &[[20., 48., 33., 107.], [95., 48., 108., 107.]],
        UnitKind::Excavator => &[[17., 50., 32., 109.], [96., 50., 111., 109.]],
        UnitKind::Tender => &[[20., 52., 33., 108.], [95., 52., 108., 108.]],
        UnitKind::Avalanche => &[[27., 38., 36., 102.], [93., 38., 102., 102.]],
        UnitKind::Breaker => &[[23., 42., 38., 107.], [90., 42., 105., 107.]],
        UnitKind::Bombard => &[[16., 58., 31., 107.], [97., 58., 112., 107.]],
        UnitKind::Flakhound => &[[14., 73., 31., 111.], [97., 73., 114., 111.]],
        _ => &[],
    }
}

pub(crate) fn gauge(kind: UnitKind, scale: f32) -> f32 {
    let belts = runs(kind);
    if belts.len() < 2 {
        return 0.0;
    }
    (belts[1][0] + belts[1][2] - belts[0][0] - belts[0][2]) * 0.5 / 128.0 * scale
}

pub(super) fn draw(
    kind: UnitKind,
    center: Vec2,
    size: f32,
    rotation: f32,
    travel: [f32; 2],
    scale: f32,
) {
    let factor = size / 128.0;
    let (sin, cos) = rotation.sin_cos();
    let point = |x: f32, y: f32| {
        let x = (x - 64.0) * factor;
        let y = (y - 64.0) * factor;
        center + vec2(x * cos - y * sin, x * sin + y * cos)
    };
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32, color: Color| {
        if y1 <= y0 {
            return;
        }
        let (a, b, c, d) = (point(x0, y0), point(x1, y0), point(x1, y1), point(x0, y1));
        draw_triangle(a, b, c, color);
        draw_triangle(a, c, d, color);
    };
    for &[x0, y0, x1, y1] in runs(kind) {
        let side = usize::from(x0 > 64.0);
        let pitch = 13.0;
        // The visible upper run travels toward the nose; the hidden contact
        // run travels backward. One world unit of belt travel stays one unit.
        let offset = (-travel[side] * 128.0 / scale).rem_euclid(pitch);
        rect(x0, y0, x1, y1, color_u8!(23, 24, 29, 255));
        let mut y = y0 - pitch + offset;
        while y < y1 {
            let top = y.max(y0);
            let bottom = (y + 6.0).min(y1);
            rect(x0, top, x1, bottom, color_u8!(67, 68, 76, 255));
            rect(
                x0 + 1.0,
                top,
                x1 - 1.0,
                (y + 1.6).min(y1),
                color_u8!(107, 105, 107, 255),
            );
            rect(
                x0 + 2.0,
                (y + 4.5).max(y0),
                x1 - 2.0,
                bottom,
                color_u8!(44, 45, 52, 255),
            );
            y += pitch;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::numeric;

    fn idle_body(kind: UnitKind) -> Image {
        let sprites = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/sprites");
        let name = kind.name();
        let path = [
            format!("rig_{name}_hull_ferrous"),
            format!("rig_{name}_body_ferrous"),
            format!("rig_{name}_body_ferrous_cargo0"),
            format!("{name}_ferrous"),
        ]
        .into_iter()
        .map(|stem| sprites.join(format!("{stem}.png")))
        .find(|path| path.exists())
        .unwrap_or_else(|| panic!("{kind:?} has no body art"));
        Image::from_file_with_format(&std::fs::read(&path).unwrap(), Some(ImageFormat::Png))
            .unwrap()
    }

    #[test]
    fn belts_stay_on_their_track_casings() {
        for kind in UnitKind::ALL.into_iter().filter(|&kind| supported(kind)) {
            let body = idle_body(kind);
            let width = usize::from(body.width);
            for &[x0, y0, x1, y1] in runs(kind) {
                for y in numeric::to_usize(y0)..numeric::to_usize(y1) {
                    for x in numeric::to_usize(x0)..numeric::to_usize(x1) {
                        assert_eq!(
                            body.bytes[(y * width + x) * 4 + 3],
                            u8::MAX,
                            "{kind:?} belt leaves its casing at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }
}
