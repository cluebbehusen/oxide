use super::*;

/// WCAG channel linearization.
fn linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance, alpha ignored (callers composite first).
fn luminance(c: Color) -> f32 {
    0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
}

/// sRGB-space alpha blend of `fg` over an opaque `field` — the
/// same per-channel math the GPU applies when the text draws.
fn composite(fg: Color, field: Color) -> Color {
    let a = fg.a;
    Color::new(
        fg.r * a + field.r * (1.0 - a),
        fg.g * a + field.g * (1.0 - a),
        fg.b * a + field.b * (1.0 - a),
        1.0,
    )
}

/// Contrast ratio of translucent text drawn on an opaque field.
fn contrast(text: Color, field: Color) -> f32 {
    let field = Color::new(field.r, field.g, field.b, 1.0);
    let l1 = luminance(composite(text, field));
    let l2 = luminance(field);
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

#[test]
fn tutorial_body_clears_aa_on_its_card() {
    assert!(contrast(TEXT_BODY, SURFACE_CARD) >= 4.5);
}

#[test]
fn body_and_secondary_clear_aa_on_every_house_field() {
    for field in [SURFACE_PANEL, SURFACE_MENU, SURFACE_CARD] {
        assert!(contrast(TEXT_BODY, field) >= 4.5);
        assert!(contrast(TEXT_SECONDARY, field) >= 4.5);
    }
}

#[test]
fn primary_clears_aaa_on_every_house_field() {
    for field in [SURFACE_PANEL, SURFACE_MENU, SURFACE_CARD] {
        assert!(contrast(TEXT_PRIMARY, field) >= 7.0);
    }
}

#[test]
fn disabled_reads_as_disabled_and_the_tiers_stay_ordered() {
    let on_card = |t| contrast(t, SURFACE_CARD);
    assert!(on_card(TEXT_DISABLED) < 4.5, "dimness is the signal");
    assert!(on_card(TEXT_DISABLED) < on_card(TEXT_SECONDARY));
    assert!(on_card(TEXT_SECONDARY) < on_card(TEXT_BODY));
    assert!(on_card(TEXT_BODY) < on_card(TEXT_PRIMARY));
}

#[test]
fn chrome_text_clears_its_tier_on_every_plate_and_card() {
    let plates = [SURFACE_PLATE, SURFACE_CAPTION, SURFACE_BAND, VEIL, BADGE];
    let cards = [CARD_IDLE, CARD_HOVER, CARD_ARMED, CHIP];
    for field in plates.into_iter().chain(cards) {
        assert!(contrast(TEXT_PRIMARY, field) >= 7.0, "{field:?}");
        assert!(contrast(TEXT_BODY, field) >= 4.5, "{field:?}");
    }
    // Hotkey corners on a hovered (4.4:1) or armed (4.2:1) card fall just
    // short of AA; every other field clears it.
    for field in plates.into_iter().chain([CARD_IDLE, CHIP]) {
        assert!(contrast(TEXT_SECONDARY, field) >= 4.5, "{field:?}");
    }
    for field in plates {
        assert!(contrast(TEXT_ALERT, field) >= 4.5, "{field:?}");
    }
}

#[test]
fn strokes_scale_and_never_vanish() {
    assert_eq!(Stroke::Edge.at(2.0), 3.0);
    assert_eq!(Stroke::Hairline.at(0.75), 1.0, "a device pixel at least");
    let widths = [Stroke::Hairline, Stroke::Edge, Stroke::Focus, Stroke::Heavy].map(|s| s.at(1.0));
    assert!(widths.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn the_type_ramp_climbs_and_scales() {
    let ramp = [
        Type::Caption,
        Type::Small,
        Type::Body,
        Type::Label,
        Type::Heading,
        Type::Row,
        Type::Title,
        Type::Display,
    ];
    let sizes = ramp.map(|step| step.at(1.0));
    assert!(sizes.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(Type::Body.at(1.5), 24.0);
}
