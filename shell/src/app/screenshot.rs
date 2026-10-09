//! Debug screenshots: requests park until the frame renders, then one
//! readback serves them all.

use super::App;
use anyhow::{Context, Result};
use macroquad::prelude::{Image, get_screen_data};
use oxide_protocol::{Reply, ResponseEnvelope, ScreenshotView};
use std::sync::mpsc::Sender;

/// A screenshot request parked until after this frame renders.
pub(super) struct PendingScreenshot {
    pub(super) id: u64,
    pub(super) path: String,
    pub(super) reply: Sender<ResponseEnvelope>,
}

/// Answers every screenshot requested this frame from one readback.
pub(super) fn serve(app: &mut App) {
    if app.pending_shots.is_empty() {
        return;
    }
    let image = get_screen_data();
    for shot in app.pending_shots.drain(..) {
        let response = match write_png(&image, &shot.path) {
            Ok((width, height)) => ResponseEnvelope::ok(
                shot.id,
                Reply::Screenshot(ScreenshotView {
                    path: shot.path,
                    width,
                    height,
                    renderer: "gpu".to_string(),
                }),
            ),
            Err(err) => ResponseEnvelope::err(shot.id, format!("screenshot: {err:#}")),
        };
        shot.reply.send(response).ok();
    }
}

/// Writes a captured frame as PNG, returning errors: macroquad's own
/// `export_png` unwraps on failure, so one malformed debug-socket path
/// would abort the session.
pub(super) fn write_png(image: &Image, path: &str) -> Result<(u32, u32)> {
    if let Some(parent) = std::path::Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path).with_context(|| format!("creating {path}"))?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        u32::from(image.width),
        u32::from(image.height),
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().context("writing png header")?;
    // The GL framebuffer is bottom-up; PNG rows are top-down.
    let stride = usize::from(image.width) * 4;
    let mut flipped = Vec::with_capacity(image.bytes.len());
    for row in image.bytes.chunks_exact(stride).rev() {
        flipped.extend_from_slice(row);
    }
    writer
        .write_image_data(&flipped)
        .context("writing png data")?;
    Ok((u32::from(image.width), u32::from(image.height)))
}

#[cfg(test)]
mod tests;
