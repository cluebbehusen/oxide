//! Keyboard navigation every menu, button row, grid and table shares.
//!
//! One rule everywhere: arrows wrap at the edges, Home and End jump to the
//! first and last live cell, and paging moves a screenful and stops at the
//! ends. Screens decode a frame's keys with [`Nav::decode`] and step their
//! cursor with [`step_line`] or [`step_grid`].

use oxide_protocol::{Key, RawEvent};

/// What a canonical menu key asks of a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Confirm,
    Back,
}

impl Nav {
    /// The navigation a key press means, after the binding map has turned
    /// the player's keys into the canonical menu keys.
    pub fn decode(event: &RawEvent) -> Option<Self> {
        let RawEvent::KeyDown { key } = event else {
            return None;
        };
        Some(match key {
            Key::Up => Self::Up,
            Key::Down => Self::Down,
            Key::Left => Self::Left,
            Key::Right => Self::Right,
            Key::PageUp => Self::PageUp,
            Key::PageDown => Self::PageDown,
            Key::Home => Self::Home,
            Key::End => Self::End,
            Key::Enter => Self::Confirm,
            Key::Escape => Self::Back,
            _ => return None,
        })
    }
}

/// Which arrows move a cursor along a line of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Up and Down, as in a list.
    Vertical,
    /// Left and Right, as in a button row.
    Horizontal,
    /// All four arrows, for a single row or column that reads either way.
    Both,
}

/// Steps a cursor at `at` along a line of `len` cells, skipping cells
/// `live` rejects. Arrows along `axis` wrap; Home and End land on the
/// first and last live cell; paging moves `page` cells and stops at the
/// ends, settling on the nearest live cell. `None` when `nav` does not move
/// along this line or no cell is live.
pub fn step_line(
    len: usize,
    at: usize,
    nav: Nav,
    axis: Axis,
    page: usize,
    live: impl Fn(usize) -> bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let at = at.min(len - 1);
    let back = match (nav, axis) {
        (Nav::Up, Axis::Vertical | Axis::Both) | (Nav::Left, Axis::Horizontal | Axis::Both) => true,
        (Nav::Down, Axis::Vertical | Axis::Both) | (Nav::Right, Axis::Horizontal | Axis::Both) => {
            false
        }
        (Nav::Home, _) => return nearest(len, 0, false, live),
        (Nav::End, _) => return nearest(len, len - 1, true, live),
        (Nav::PageUp, _) => return nearest(len, at.saturating_sub(page.max(1)), true, live),
        (Nav::PageDown, _) => {
            return nearest(len, (at + page.max(1)).min(len - 1), false, live);
        }
        _ => return None,
    };
    (1..=len)
        .map(|step| {
            if back {
                (at + len - step % len) % len
            } else {
                (at + step) % len
            }
        })
        .find(|&cell| live(cell))
}

/// The live cell nearest `from`, walking toward the start when `back` and
/// toward the end otherwise, then the other way, without wrapping.
pub fn nearest(len: usize, from: usize, back: bool, live: impl Fn(usize) -> bool) -> Option<usize> {
    let from = from.min(len.checked_sub(1)?);
    let before = (0..=from).rev();
    let after = from..len;
    if back {
        before.chain(after).find(|&cell| live(cell))
    } else {
        after.chain(before).find(|&cell| live(cell))
    }
}

/// Steps a cursor through a grid whose rows hold `rows[i]` cells, numbered
/// in reading order. Left and Right walk that order and wrap; Up and Down
/// move one row and wrap, keeping the column or the nearest one a short
/// row has; paging moves `page_rows` rows and stops at the ends; Home and
/// End land on the first and last cell.
pub fn step_grid(rows: &[usize], at: usize, nav: Nav, page_rows: usize) -> Option<usize> {
    let total: usize = rows.iter().sum();
    if total == 0 {
        return None;
    }
    let at = at.min(total - 1);
    let starts: Vec<usize> = rows
        .iter()
        .scan(0, |next, len| {
            let start = *next;
            *next += len;
            Some(start)
        })
        .collect();
    let live_rows: Vec<usize> = (0..rows.len()).filter(|&r| rows[r] > 0).collect();
    let row = (0..rows.len())
        .rev()
        .find(|&r| rows[r] > 0 && starts[r] <= at)?;
    let column = at - starts[row];
    let row_rank = live_rows.iter().position(|&r| r == row)?;
    let cell_in = |rank: usize| {
        let r = live_rows[rank];
        starts[r] + column.min(rows[r] - 1)
    };
    let n = live_rows.len();
    Some(match nav {
        Nav::Left => (at + total - 1) % total,
        Nav::Right => (at + 1) % total,
        Nav::Up => cell_in((row_rank + n - 1) % n),
        Nav::Down => cell_in((row_rank + 1) % n),
        Nav::PageUp => cell_in(row_rank.saturating_sub(page_rows.max(1))),
        Nav::PageDown => cell_in((row_rank + page_rows.max(1)).min(n - 1)),
        Nav::Home => 0,
        Nav::End => total - 1,
        Nav::Confirm | Nav::Back => return None,
    })
}

#[cfg(test)]
mod tests;
