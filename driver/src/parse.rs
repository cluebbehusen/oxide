//! Argument value-parsers for the live-client CLI: tiles, points,
//! buttons, keys, and the kind-name enums clap surfaces as choices.

use anyhow::{Context, Result, bail};
use oxide_protocol::{Key, MouseButton};
use oxide_sim::UnitKind;

pub(crate) fn parse_tile(s: &str) -> Result<chassis::grid::TilePos> {
    let (x, y) = s
        .split_once(',')
        .with_context(|| format!("expected \"x,y\", got {s:?}"))?;
    Ok(chassis::grid::TilePos::new(
        x.trim().parse()?,
        y.trim().parse()?,
    ))
}

pub(crate) fn parse_point(s: &str) -> Result<(f32, f32)> {
    let (x, y) = s
        .split_once(',')
        .with_context(|| format!("expected \"x,y\", got {s:?}"))?;
    let point = (x.trim().parse::<f32>()?, y.trim().parse::<f32>()?);
    if point.0.is_finite() && point.1.is_finite() {
        Ok(point)
    } else {
        bail!("point coordinates must be finite")
    }
}

pub(crate) fn parse_mouse_button(s: &str) -> Result<MouseButton> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        other => bail!("unknown button {other:?}"),
    })
}

/// Clap-native unit kinds — typos die in argument parsing with the full
/// list of choices, before anything touches the socket.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum UnitKindArg {
    Harvester,
    Sentinel,
    Scuttler,
    Lancer,
    Bombard,
    Flakhound,
    Stinger,
    Buzzard,
    Darter,
    Talon,
    Wisp,
}

impl From<UnitKindArg> for UnitKind {
    fn from(k: UnitKindArg) -> Self {
        match k {
            UnitKindArg::Harvester => UnitKind::Harvester,
            UnitKindArg::Sentinel => UnitKind::Sentinel,
            UnitKindArg::Scuttler => UnitKind::Scuttler,
            UnitKindArg::Lancer => UnitKind::Lancer,
            UnitKindArg::Bombard => UnitKind::Bombard,
            UnitKindArg::Flakhound => UnitKind::Flakhound,
            UnitKindArg::Stinger => UnitKind::Stinger,
            UnitKindArg::Buzzard => UnitKind::Buzzard,
            UnitKindArg::Darter => UnitKind::Darter,
            UnitKindArg::Talon => UnitKind::Talon,
            UnitKindArg::Wisp => UnitKind::Wisp,
        }
    }
}

/// Buildable kinds only — the Foundry is scenario-authored and rejecting
/// it at the parser teaches that faster than a sim rejection would.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum BuildingKindArg {
    Turret,
    Fabricator,
    FlakTurret,
    Bastion,
    Array,
    Reclaimer,
}

impl From<BuildingKindArg> for oxide_sim::BuildingKind {
    fn from(k: BuildingKindArg) -> Self {
        match k {
            BuildingKindArg::Turret => oxide_sim::BuildingKind::Turret,
            BuildingKindArg::Fabricator => oxide_sim::BuildingKind::Fabricator,
            BuildingKindArg::FlakTurret => oxide_sim::BuildingKind::FlakTurret,
            BuildingKindArg::Bastion => oxide_sim::BuildingKind::Bastion,
            BuildingKindArg::Array => oxide_sim::BuildingKind::Array,
            BuildingKindArg::Reclaimer => oxide_sim::BuildingKind::Reclaimer,
        }
    }
}

pub(crate) fn parse_key(s: &str) -> Result<Key> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "h" => Key::H,
        "s" => Key::S,
        "a" => Key::A,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "o" => Key::O,
        "q" => Key::Q,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "y" => Key::Y,
        "z" => Key::Z,
        "p" => Key::P,
        "r" => Key::R,
        "b" => Key::B,
        "n" => Key::N,
        "x" => Key::X,
        "enter" | "return" => Key::Enter,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "space" => Key::Space,
        "f1" => Key::F1,
        "shift" => Key::Shift,
        "ctrl" => Key::Ctrl,
        "1" => Key::Num1,
        "2" => Key::Num2,
        "3" => Key::Num3,
        "4" => Key::Num4,
        "5" => Key::Num5,
        "6" => Key::Num6,
        "7" => Key::Num7,
        "8" => Key::Num8,
        "9" => Key::Num9,
        other => bail!("unknown key {other:?}"),
    })
}
