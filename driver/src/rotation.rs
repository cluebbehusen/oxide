//! Seat-swapping geometry for controlled bot comparisons.

use anyhow::Result;
use chassis::grid::as_index;
use oxide_sim::BuildingKind;
use oxide_sim::scenario::Scenario;

/// Rotates a scenario 180 degrees: terrain, Foundry anchors, starting
/// units and pre-built structures all turn together, so each seat keeps
/// its player index while playing the geometry its mirror held. This
/// separates player index from map position.
///
/// Refuses a map whose terrain is not exactly 180-symmetric with Foundry and
/// Extractor footprint markers read as ground. Rotating an asymmetric map would
/// hand the seats different terrain, so a comparison would measure the map
/// rather than the seat.
pub fn rotate_180(base: &Scenario) -> Result<Scenario> {
    let height = base.map.len();
    anyhow::ensure!(height > 0, "{} has an empty map", base.name);
    let rows: Vec<Vec<char>> = base.map.iter().map(|r| r.chars().collect()).collect();
    let width = rows[0].len();
    anyhow::ensure!(
        rows.iter().all(|r| r.len() == width),
        "{}'s map is ragged; the rotation needs a rectangle",
        base.name
    );
    let is_anchor = |c: char| c.is_ascii_digit() && c != '0' && c != '9';
    let is_frame = |c: char| c == 'E';
    let ground = |c: char| {
        if is_anchor(c) || is_frame(c) { '.' } else { c }
    };
    for (y, row) in rows.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            anyhow::ensure!(
                ground(c) == ground(rows[height - 1 - y][width - 1 - x]),
                "{} is not 180-symmetric at ({x}, {y}) — the probe will not rotate it",
                base.name
            );
        }
    }

    let mut map: Vec<Vec<char>> = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| ground(rows[height - 1 - y][width - 1 - x]))
                .collect()
        })
        .collect();
    // An anchor digit names the TOP-LEFT of the Foundry's footprint, so
    // its rotation lands a footprint in from the rotated corner. That
    // target sits inside the original footprint and is therefore open
    // ground the symmetry check already cleared.
    let (fw, fh) = BuildingKind::Foundry.size();
    let (fw, fh) = (as_index(fw), as_index(fh));
    for (y, row) in rows.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if !is_anchor(c) {
                continue;
            }
            let (Some(ty), Some(tx)) = (
                height.checked_sub(fh).and_then(|h| h.checked_sub(y)),
                width.checked_sub(fw).and_then(|w| w.checked_sub(x)),
            ) else {
                anyhow::bail!(
                    "{}'s anchor at ({x}, {y}) has no room for a Foundry",
                    base.name
                );
            };
            map[ty][tx] = c;
        }
    }
    // `E` likewise names the top-left of a 2x2 Extractor frame. Rotating
    // the marker as a point would shift the gameplay footprint by one tile.
    let (ew, eh) = BuildingKind::Extractor.size();
    let (ew, eh) = (as_index(ew), as_index(eh));
    for (y, row) in rows.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if !is_frame(c) {
                continue;
            }
            let (Some(ty), Some(tx)) = (
                height.checked_sub(eh).and_then(|h| h.checked_sub(y)),
                width.checked_sub(ew).and_then(|w| w.checked_sub(x)),
            ) else {
                anyhow::bail!(
                    "{}'s Extractor frame at ({x}, {y}) has no room for its footprint",
                    base.name
                );
            };
            map[ty][tx] = c;
        }
    }

    let mut out = base.clone();
    out.map = map.into_iter().map(|r| r.into_iter().collect()).collect();
    let (w, h) = (
        i32::try_from(width).expect("map extents fit in i32"),
        i32::try_from(height).expect("map extents fit in i32"),
    );
    for unit in &mut out.units {
        unit.x = w - 1 - unit.x;
        unit.y = h - 1 - unit.y;
    }
    for building in &mut out.buildings {
        let (bw, bh) = building.kind.size();
        building.x = w - building.x - bw;
        building.y = h - building.y - bh;
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
