//! Pure camera-listener weighting for one frame of queued sound events.

use crate::game::SoundKind;
use crate::mixer::Weight;
use crate::numeric;
use macroquad::prelude::Vec2;

const WIDE_ZOOM: f32 = 8.0;
const DETAIL_ZOOM: f32 = 32.0;
const WIDE_POSITIONAL_VOICES: usize = 5;
const CLOSE_POSITIONAL_VOICES: usize = 12;
const EXPLOSION_FALLOFF_TILES: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameSound {
    pub kind: SoundKind,
    pub gain: f32,
    positioned: bool,
}

fn zoom_detail(zoom: f32) -> f32 {
    ((zoom - WIDE_ZOOM) / (DETAIL_ZOOM - WIDE_ZOOM)).clamp(0.0, 1.0)
}

fn zoom_gain(kind: SoundKind, zoom: f32) -> f32 {
    let detail = zoom_detail(zoom);
    match crate::mixer::spec(kind).weight {
        Weight::Detail => 0.25 + 0.75 * detail,
        Weight::Standard => 0.45 + 0.55 * detail,
        Weight::Heavy => 0.72 + 0.28 * detail,
        Weight::Protected => 1.0,
    }
}

fn distance_gain(kind: SoundKind, world: Vec2, center: Vec2, half_extents: Vec2) -> f32 {
    let half_extents = Vec2::new(half_extents.x.max(1.0), half_extents.y.max(1.0));
    let delta = (world - center).abs() - half_extents;
    let outside = Vec2::new(delta.x.max(0.0), delta.y.max(0.0));
    let distance = outside.length();
    if crate::mixer::spec(kind).explosion {
        (1.0 - distance / EXPLOSION_FALLOFF_TILES).clamp(0.0, 1.0)
    } else if distance == 0.0 {
        1.0
    } else {
        (1.0 - distance / (2.0 * half_extents.length())).clamp(0.25, 1.0)
    }
}

fn positional_voice_limit(zoom: f32) -> usize {
    let detail = zoom_detail(zoom);
    WIDE_POSITIONAL_VOICES
        + numeric::to_usize(
            ((CLOSE_POSITIONAL_VOICES - WIDE_POSITIONAL_VOICES) as f32 * detail).round(),
        )
}

/// Coalesces one frame of emitters into the mix heard at the current camera.
///
/// Unpositioned UI and alert sounds always survive. Positional duplicates keep
/// the loudest emitter of their kind, and a wide camera admits fewer minor
/// voices while reserving room for heavy threats.
pub(crate) fn frame_mix(
    queued: impl IntoIterator<Item = (SoundKind, Option<Vec2>)>,
    center: Vec2,
    half_extents: Vec2,
    zoom: f32,
) -> Vec<FrameSound> {
    let mut mixed: Vec<FrameSound> = Vec::new();
    for (kind, world) in queued {
        let protected = matches!(kind, SoundKind::Alert);
        let positioned = world.is_some() && !protected;
        let gain = if protected {
            1.0
        } else if let Some(world) = world {
            distance_gain(kind, world, center, half_extents) * zoom_gain(kind, zoom)
        } else {
            1.0
        };

        if gain <= 0.0 {
            continue;
        }

        if let Some(existing) = mixed.iter_mut().find(|event| event.kind == kind) {
            if gain > existing.gain {
                existing.gain = gain;
                existing.positioned = positioned;
            }
        } else {
            mixed.push(FrameSound {
                kind,
                gain,
                positioned,
            });
        }
    }

    let limit = positional_voice_limit(zoom);
    let mut ranked: Vec<usize> = mixed
        .iter()
        .enumerate()
        .filter_map(|(index, event)| event.positioned.then_some(index))
        .collect();
    ranked.sort_by(|&left, &right| {
        let heavy = |event: FrameSound| crate::mixer::spec(event.kind).weight == Weight::Heavy;
        heavy(mixed[right])
            .cmp(&heavy(mixed[left]))
            .then_with(|| mixed[right].gain.total_cmp(&mixed[left].gain))
            .then_with(|| left.cmp(&right))
    });
    if ranked.len() > limit {
        let mut keep = vec![true; mixed.len()];
        for index in ranked.into_iter().skip(limit) {
            keep[index] = false;
        }
        let mut index = 0;
        mixed.retain(|_| {
            let retain = keep[index];
            index += 1;
            retain
        });
    }

    mixed
}

#[cfg(test)]
mod tests;
