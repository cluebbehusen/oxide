//! Compact player checkpoints with independently readable metadata.

#[cfg(test)]
use crate::game::Game;
use crate::game::checkpoint::{GameCheckpoint, RestoredGame, SaveCapture};
use crate::numeric::Fit;
use anyhow::{Context, Result, ensure};
use chassis::replay::ReplayMeta;
use oxide_sim::SIM_VERSION;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"OXIDESAV";
const MAX_HEADER: usize = 64 * 1024;
const MAX_BYTES: usize = oxide_kit::checkpoint::MAX_BYTES;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    meta: ReplayMeta,
    map: String,
    seats: usize,
    compressed_bytes: u64,
    decoded_bytes: u64,
}

pub struct RecordInfo {
    pub meta: ReplayMeta,
    pub map: String,
    pub seats: usize,
    pub problem: Option<String>,
}

#[cfg(test)]
fn write(game: &Game, meta: ReplayMeta, path: &Path) -> Result<()> {
    write_capture(game.capture_save(), meta, path)
}

pub(crate) fn write_capture(capture: SaveCapture, meta: ReplayMeta, path: &Path) -> Result<()> {
    let checkpoint = capture.checkpoint()?;
    let mut payload = Vec::new();
    ciborium::into_writer(&checkpoint, &mut payload)?;
    ensure!(
        payload.len() <= MAX_BYTES,
        "save exceeds decoded byte limit"
    );
    // Metadata is obtained from the checkpoint itself, without restoring controllers.
    let (map, seats, tick) = checkpoint.metadata();
    ensure!(meta.ticks == Some(tick), "save metadata mismatch");
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3)?;
    encoder.include_checksum(true)?;
    encoder.write_all(&payload)?;
    let compressed = encoder.finish()?;
    let header = Header {
        version: VERSION,
        meta,
        map: map.to_owned(),
        seats,
        compressed_bytes: compressed.len() as u64,
        decoded_bytes: payload.len() as u64,
    };
    check_header(&header)?;
    let header = serde_json::to_vec(&header)?;
    ensure!(
        header.len() <= MAX_HEADER && 12 + header.len() + compressed.len() <= MAX_BYTES,
        "save exceeds byte limit"
    );
    chassis::fsx::write_atomic(path, |writer| {
        writer.write_all(MAGIC)?;
        writer.write_all(&header.len().fit::<u32>().to_le_bytes())?;
        writer.write_all(&header)?;
        writer.write_all(&compressed)?;
        Ok(())
    })
}

fn read_header(reader: &mut impl Read, file_bytes: u64) -> Result<Header> {
    ensure!(file_bytes <= MAX_BYTES as u64, "save exceeds byte limit");
    let mut magic = [0; 8];
    reader.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "unsupported player save format");
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= MAX_HEADER, "save header exceeds byte limit");
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    let header: Header = serde_json::from_slice(&bytes)?;
    ensure!(
        header.compressed_bytes <= MAX_BYTES as u64 && header.decoded_bytes <= MAX_BYTES as u64,
        "save payload exceeds byte limit"
    );
    ensure!(
        file_bytes == 12 + length as u64 + header.compressed_bytes,
        "save payload length mismatch"
    );
    Ok(header)
}

fn check_header(header: &Header) -> Result<()> {
    ensure!(
        header.version == VERSION,
        "unsupported save format {} (this build supports {VERSION})",
        header.version
    );
    ensure!(
        header.meta.sim_version == SIM_VERSION,
        "saved on sim v{}; this build runs v{SIM_VERSION}",
        header.meta.sim_version
    );
    ensure!(
        matches!(header.meta.kind.as_deref(), Some("save" | "autosave")),
        "invalid save kind"
    );
    Ok(())
}

pub(crate) fn prepare_load(path: &Path) -> Result<RestoredGame> {
    let (header, payload) = read_payload(path)?;
    let mut reader = payload.as_slice();
    let checkpoint: GameCheckpoint = ciborium::from_reader(&mut reader)?;
    ensure!(reader.is_empty(), "trailing checkpoint payload");
    let game = checkpoint.restore()?;
    ensure!(
        header.meta.ticks == Some(game.tick())
            && header.map == game.scenario().name
            && header.seats == game.scenario().players.len(),
        "save metadata does not match session"
    );
    Ok(game)
}

/// The checked header and decompressed checkpoint bytes of a player save.
fn read_payload(path: &Path) -> Result<(Header, Vec<u8>)> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("loading save {}", path.display()))?;
    let len = file.metadata()?.len();
    let header = read_header(&mut file, len).context("loading save header")?;
    check_header(&header)?;
    let mut compressed = Vec::new();
    file.take(header.compressed_bytes + 1)
        .read_to_end(&mut compressed)?;
    ensure!(
        compressed.len() as u64 == header.compressed_bytes,
        "save payload length mismatch"
    );
    ensure!(
        compressed
            .get(4)
            .is_some_and(|descriptor| descriptor & 4 != 0),
        "save frame has no checksum"
    );
    let frame_len = zstd::zstd_safe::find_frame_compressed_size(&compressed)
        .map_err(|code| anyhow::anyhow!("invalid compressed frame: {code}"))?;
    ensure!(frame_len == compressed.len(), "trailing compressed payload");
    let mut payload = Vec::new();
    zstd::stream::read::Decoder::new(compressed.as_slice())?
        .take(header.decoded_bytes + 1)
        .read_to_end(&mut payload)?;
    ensure!(
        payload.len() as u64 == header.decoded_bytes,
        "decoded payload length mismatch"
    );
    Ok((header, payload))
}

#[cfg(test)]
pub fn load(path: &Path) -> Result<Game> {
    prepare_load(path).map(RestoredGame::install)
}

pub fn inspect(path: &Path) -> Result<RecordInfo> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    inspect_reader(path, &mut file, len)
}

fn inspect_reader(path: &Path, file: &mut (impl Read + Seek), len: u64) -> Result<RecordInfo> {
    let mut magic = [0; 8];
    let compact = file.read_exact(&mut magic).is_ok() && &magic == MAGIC;
    if compact {
        file.seek(SeekFrom::Start(0))?;
        let header = read_header(file, len)?;
        let problem = check_header(&header)
            .err()
            .map(|error| format!("{error:#}"));
        Ok(RecordInfo {
            meta: header.meta,
            map: header.map,
            seats: header.seats,
            problem,
        })
    } else if path.extension().and_then(|extension| extension.to_str()) == Some("oxsave") {
        anyhow::bail!("invalid player save header");
    } else {
        // Replay inspection remains separate from player checkpoint restoration.
        let replay = oxide_kit::load_replay(path)?;
        let problem = replay
            .validate(Some(SIM_VERSION))
            .map_err(anyhow::Error::from)
            .and_then(|()| oxide_kit::bounded_replay_duration(&replay).map(|_| ()))
            .err()
            .map(|error| format!("{error:#}"));
        let ticks = oxide_kit::replay_duration(&replay);
        let mut meta = replay.meta;
        meta.ticks = Some(ticks);
        Ok(RecordInfo {
            meta,
            map: replay.setup.name,
            seats: replay.setup.players.len(),
            problem,
        })
    }
}

#[cfg(test)]
mod tests;
