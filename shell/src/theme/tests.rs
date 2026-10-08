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
