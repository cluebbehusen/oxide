use super::*;
use oxide_sim::Command;
use std::{
    io::BufWriter,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const QUEUE_BYTES: usize = 1024 * 1024;
const FLUSH_BYTES: usize = 64 * 1024;
const SESSION_RESERVATION: u64 = 96 * 1024 * 1024;

/// Nonblocking recording health; durable progress can lag live progress.
#[derive(Debug, Clone, Serialize)]
pub struct WriterStatus {
    /// Baseline and any previous-session evidence have been durably published.
    pub ready: bool,
    /// Last completed tick acknowledged after fsync.
    pub durable_tick: u64,
    /// Pending command memory reservation.
    pub pending_bytes: usize,
    /// A durable clean-close record has been published.
    pub clean: bool,
    /// First capture failure. The intact older prefix remains available.
    pub error: Option<String>,
}
#[derive(Default)]
struct Shared {
    ready: AtomicBool,
    durable: AtomicU64,
    pending: AtomicUsize,
    stopped: AtomicBool,
    clean: AtomicBool,
    error: Mutex<Option<String>>,
}
impl Shared {
    fn fail(&self, error: impl ToString) {
        self.stopped.store(true, Ordering::Release);
        if let Ok(mut slot) = self.error.lock()
            && slot.is_none()
        {
            *slot = Some(error.to_string());
        }
    }
}
struct Queued {
    event: Event,
    bytes: usize,
}

/// One session's bounded command sink. Dropping it preserves an interrupted record.
/// Disk work belongs to its worker; gameplay uses only `try_send` and atomic reservations.
pub struct RecoveryWriter {
    build: BuildIdentity,
    directory: PathBuf,
    sender: SyncSender<Queued>,
    shared: Arc<Shared>,
}
impl RecoveryWriter {
    /// Start a fresh recording from an already resolved replay prefix.
    /// Directory initialization and baseline serialization run on the worker.
    pub fn start(root: PathBuf, base: GameReplay, tick: u64, build: BuildIdentity) -> Result<Self> {
        Self::start_recovered(root, base, tick, None, build)
    }

    /// Retire a recovered source only after its replacement baseline is durable.
    pub fn start_recovered(
        root: PathBuf,
        base: GameReplay,
        tick: u64,
        source: Option<PathBuf>,
        build: BuildIdentity,
    ) -> Result<Self> {
        Self::start_recording(
            root,
            base,
            tick,
            source,
            RecordingKind::LiveMatch,
            None,
            build,
        )
    }

    /// Starts a live journal at a session checkpoint, without retaining older commands.
    pub fn start_checkpoint(
        root: PathBuf,
        checkpoint: crate::checkpoint::SessionCheckpoint,
        build: BuildIdentity,
    ) -> Result<Self> {
        let base = checkpoint.recording()?;
        Self::start_recovered_checkpoint(
            root,
            base.clone(),
            base.start_tick(),
            checkpoint,
            None,
            build,
        )
    }

    /// Retains a recovered segment and its controller origin until its replacement is durable.
    pub fn start_recovered_checkpoint(
        root: PathBuf,
        base: GameReplay,
        tick: u64,
        checkpoint: crate::checkpoint::SessionCheckpoint,
        source: Option<PathBuf>,
        build: BuildIdentity,
    ) -> Result<Self> {
        Self::start_recording(
            root,
            base,
            tick,
            source,
            RecordingKind::LiveMatch,
            Some(checkpoint),
            build,
        )
    }

    /// Retain a watched replay for diagnostics without offering it as a resumable match.
    pub fn start_playback(
        root: PathBuf,
        base: GameReplay,
        ticks: u64,
        build: BuildIdentity,
    ) -> Result<Self> {
        Self::start_recording(
            root,
            base,
            ticks,
            None,
            RecordingKind::Playback,
            None,
            build,
        )
    }

    fn start_recording(
        root: PathBuf,
        mut base: GameReplay,
        tick: u64,
        source: Option<PathBuf>,
        kind: RecordingKind,
        checkpoint: Option<crate::checkpoint::SessionCheckpoint>,
        build: BuildIdentity,
    ) -> Result<Self> {
        ensure!(
            source
                .as_ref()
                .is_none_or(|source| source.parent() == Some(root.as_path())),
            "recovered source is outside the recording root"
        );
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let session = format!(
            "session-{:020}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        base.meta.ticks = Some(tick);
        let directory = root.join(&session);
        let header = Header {
            kind,
            session,
            build: build.clone(),
            base,
            checkpoint,
        };
        let shared = Arc::new(Shared::default());
        let worker_shared = shared.clone();
        let worker_directory = directory.clone();
        let (sender, receiver) = mpsc::sync_channel::<Queued>(4096);
        std::thread::Builder::new()
            .name("oxide-recovery".into())
            .spawn(move || {
                let mut lease = None;
                if let Err(error) = run(
                    &root,
                    &worker_directory,
                    header,
                    receiver,
                    &worker_shared,
                    &mut lease,
                    source,
                ) {
                    worker_shared.fail(format!("{error:#}"));
                }
                let status = snapshot(&worker_shared);
                if lease.is_some() {
                    let _ = chassis::fsx::write_atomic(
                        worker_directory.join("status.json"),
                        |writer| {
                            serde_json::to_writer(writer, &status).map_err(std::io::Error::other)
                        },
                    );
                }
            })?;
        Ok(Self {
            build,
            directory,
            sender,
            shared,
        })
    }
    /// Host identity shared by the recording header and diagnostic sidecars.
    pub fn build(&self) -> &BuildIdentity {
        &self.build
    }
    /// Directory containing this recording and its diagnostic sidecars.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Atomically sampled writer progress. Never waits for disk I/O.
    pub fn status(&self) -> WriterStatus {
        snapshot(&self.shared)
    }
    /// Freeze the command batch before invoking the authoritative tick.
    pub fn prepared(&self, tick: u64, commands: &[PlayerCommand]) {
        let bytes = commands.iter().fold(256usize, |total, command| {
            total.saturating_add(command_bytes(&command.command))
        });
        if self.reserve(bytes) {
            self.send(Queued {
                event: Event::Prepared {
                    tick,
                    commands: commands.to_vec(),
                },
                bytes,
            });
        }
    }
    /// Mark successful tick completion; presentation failures cannot undo this boundary.
    pub fn completed(&self, tick: u64) {
        if self.reserve(256) {
            self.send(Queued {
                event: Event::Completed { tick },
                bytes: 256,
            });
        }
    }
    /// Request a durable clean close after the ordinary save succeeded.
    /// Callers may poll status during shutdown; this method never waits.
    pub fn finish(&self, tick: u64) {
        if self.reserve(256) {
            self.send(Queued {
                event: Event::Clean { tick },
                bytes: 256,
            });
            self.shared.stopped.store(true, Ordering::Release);
        }
    }
    fn reserve(&self, bytes: usize) -> bool {
        if self.shared.stopped.load(Ordering::Acquire) {
            return false;
        }
        if self
            .shared
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(bytes).filter(|n| *n <= QUEUE_BYTES)
            })
            .is_err()
        {
            self.shared
                .fail("recovery queue is full; recording stopped at its intact prefix");
            return false;
        }
        true
    }
    fn send(&self, queued: Queued) {
        if let Err(error) = self.sender.try_send(queued) {
            self.shared.pending.fetch_sub(
                match error {
                    mpsc::TrySendError::Full(q) | mpsc::TrySendError::Disconnected(q) => q.bytes,
                },
                Ordering::AcqRel,
            );
            self.shared
                .fail("recovery writer unavailable; recording stopped");
        }
    }
}
fn snapshot(shared: &Shared) -> WriterStatus {
    WriterStatus {
        ready: shared.ready.load(Ordering::Acquire),
        durable_tick: shared.durable.load(Ordering::Acquire),
        pending_bytes: shared.pending.load(Ordering::Acquire),
        clean: shared.clean.load(Ordering::Acquire),
        error: shared.error.try_lock().ok().and_then(|error| error.clone()),
    }
}
fn command_bytes(command: &Command) -> usize {
    // Conservative accounting covers cloned vectors, enum storage and queue overhead.
    let (ids, points) = match command {
        Command::Move { units, .. }
        | Command::Attack { units, .. }
        | Command::AttackMove { units, .. }
        | Command::Harvest { units, .. }
        | Command::ReturnCargo { units, .. }
        | Command::Stop { units }
        | Command::Build { units, .. }
        | Command::Repair { units, .. }
        | Command::Salvage { units, .. }
        | Command::RepairUnit { units, .. }
        | Command::Advance { units, .. }
        | Command::Load { units, .. } => (units.len(), 0),
        Command::Patrol { units, waypoints } => (units.len(), waypoints.len()),
        Command::FocusFire { buildings, .. } | Command::ClearFocus { buildings } => {
            (buildings.len(), 0)
        }
        Command::Train { .. }
        | Command::Cancel { .. }
        | Command::CancelTrain { .. }
        | Command::SetRally { .. }
        | Command::Surrender
        | Command::CancelFound { .. }
        | Command::UpgradeBuilding { .. }
        | Command::Unload { .. } => (0, 0),
    };
    1024usize
        .saturating_add(ids.saturating_mul(32))
        .saturating_add(points.saturating_mul(128))
}

fn run(
    root: &Path,
    directory: &Path,
    header: Header,
    receiver: mpsc::Receiver<Queued>,
    shared: &Shared,
    lease_guard: &mut Option<File>,
    source: Option<PathBuf>,
) -> Result<()> {
    let source_lease = source
        .as_ref()
        .map(|source| {
            let claim =
                inactive(source).context("recovered source is active or already claimed")?;
            ensure!(
                !source.join("superseded.json").try_exists()?,
                "recovered source has already been superseded"
            );
            ensure!(
                inspect(source)?.kind == RecordingKind::LiveMatch,
                "playback diagnostics cannot become a recovered live match"
            );
            Ok::<_, anyhow::Error>(claim)
        })
        .transpose()?;
    std::fs::create_dir_all(root)?;
    let budget = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("budget.lock"))?;
    budget.lock()?;
    admit(root)?;
    std::fs::create_dir(directory)?;
    let lease = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.join("lease"))?;
    lease.lock()?;
    File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.join("readers"))?;
    *lease_guard = Some(lease);
    budget.unlock()?;
    validate_origin(header.kind, &header.base, header.checkpoint.as_ref())?;
    ensure!(
        header
            .base
            .meta
            .ticks
            .is_some_and(|tick| tick <= MAX_REPLAY_TICKS),
        "recovery base tick limit"
    );
    let mut buffer = Vec::from(MAGIC.as_slice());
    write_frame(&mut buffer, &header)?;
    ensure!(
        buffer.len() as u64 <= MAX_BYTES,
        "recovery baseline too large"
    );
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(directory.join("recovery.bin"))?;
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    let mut file = BufWriter::new(file);
    let mut total = 0;
    let mut tick = header.base.meta.ticks.unwrap_or(0);
    let mut prepared = false;
    let mut count = header.base.commands.len();
    let mut replay_bytes = super::pretty_size(&header.base)?.saturating_add(256);
    ensure!(
        replay_bytes <= MAX_BYTES as usize,
        "recovery replay size limit"
    );
    let mut sequence = 0;
    let mut last_flush = Instant::now();
    flush(
        root,
        &budget,
        &mut file,
        &mut buffer,
        &mut total,
        tick,
        shared,
    )?;
    if let Some(source) = source {
        let previous = inspect(&source)?;
        ensure!(
            chassis::hash::state_hash(&previous.replay) == chassis::hash::state_hash(&header.base),
            "replacement does not contain the recovered prefix"
        );
        ensure!(
            chassis::hash::state_hash(&previous.checkpoint)
                == chassis::hash::state_hash(&header.checkpoint),
            "replacement does not retain the recovered controller origin"
        );
        let provenance = serde_json::json!({
            "session": previous.session, "build": previous.build, "ticks": tick,
            "issue": previous.issue, "prepared_commands": previous.prepared
        });
        let bytes = serde_json::to_vec(&provenance)?;
        ensure!(
            bytes.len() <= 1024 * 1024,
            "recovered provenance size limit"
        );
        chassis::fsx::write_atomic(directory.join("previous-manifest.json"), |writer| {
            writer.write_all(&bytes)
        })?;
        for name in [
            "timings.json",
            "watchdog.json",
            "context.json",
            "status.json",
        ] {
            let file = source.join(name);
            if let Ok(metadata) = std::fs::symlink_metadata(&file) {
                ensure!(
                    metadata.is_file() && metadata.len() <= 8 * 1024 * 1024,
                    "invalid recovered diagnostic sidecar"
                );
                let bytes = std::fs::read(file)?;
                chassis::fsx::write_atomic(directory.join(format!("previous-{name}")), |writer| {
                    writer.write_all(&bytes)
                })?;
            }
        }
        let marker = serde_json::json!({"by": header.session, "ticks": tick});
        chassis::fsx::write_atomic(source.join("superseded.json"), |writer| {
            serde_json::to_writer(writer, &marker).map_err(std::io::Error::other)
        })?;
    }
    drop(source_lease);
    shared.ready.store(true, Ordering::Release);
    loop {
        let message = receiver.recv_timeout(Duration::from_millis(250));
        match message {
            Ok(queued) => {
                shared.pending.fetch_sub(queued.bytes, Ordering::AcqRel);
                ensure!(
                    header.kind == RecordingKind::LiveMatch
                        || matches!(queued.event, Event::Clean { .. }),
                    "playback source replay is immutable"
                );
                match &queued.event {
                    Event::Prepared { tick: at, commands } => {
                        ensure!(
                            *at == tick && !prepared && tick < MAX_REPLAY_TICKS,
                            "invalid recovery preparation"
                        );
                        if *at == header.base.start_tick()
                            && let Some(checkpoint) = &header.checkpoint
                        {
                            checkpoint.validate_first_batch(commands)?;
                        }
                        count = count.saturating_add(commands.len());
                        replay_bytes = commands.iter().fold(replay_bytes, |bytes, command| {
                            bytes.saturating_add(command_bytes(&command.command))
                        });
                        ensure!(
                            replay_bytes <= MAX_BYTES as usize,
                            "recovery replay size limit"
                        );
                        ensure!(
                            count <= chassis::replay::MAX_REPLAY_COMMANDS,
                            "recovery command limit"
                        );
                        prepared = true;
                    }
                    Event::Completed { tick: at } => {
                        ensure!(*at == tick + 1 && prepared, "invalid recovery completion");
                        tick = *at;
                        prepared = false;
                    }
                    Event::Clean { tick: at } => {
                        ensure!(*at == tick && !prepared, "invalid recovery close")
                    }
                }
                let clean = matches!(queued.event, Event::Clean { .. });
                write_frame(
                    &mut buffer,
                    &Record {
                        session: header.session.clone(),
                        sequence,
                        event: queued.event,
                    },
                )?;
                sequence += 1;
                if clean {
                    #[cfg(test)]
                    super::tests::fault("clean");
                    flush(
                        root,
                        &budget,
                        &mut file,
                        &mut buffer,
                        &mut total,
                        tick,
                        shared,
                    )?;
                    shared.clean.store(true, Ordering::Release);
                    while receiver.recv().is_ok() {}
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush(
                    root,
                    &budget,
                    &mut file,
                    &mut buffer,
                    &mut total,
                    tick,
                    shared,
                )?;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if !buffer.is_empty()
            && (buffer.len() >= FLUSH_BYTES || last_flush.elapsed() >= Duration::from_secs(1))
        {
            flush(
                root,
                &budget,
                &mut file,
                &mut buffer,
                &mut total,
                tick,
                shared,
            )?;
            last_flush = Instant::now();
        }
    }
    Ok(())
}
fn flush(
    root: &Path,
    budget: &File,
    file: &mut BufWriter<File>,
    buffer: &mut Vec<u8>,
    total: &mut u64,
    tick: u64,
    shared: &Shared,
) -> Result<()> {
    if buffer.is_empty() {
        return Ok(());
    }
    ensure!(
        total.saturating_add(buffer.len() as u64) <= MAX_BYTES,
        "recovery journal reached its size limit"
    );
    budget.lock()?;
    let result = (|| -> Result<()> {
        ensure!(
            managed_size(root) <= MANAGED_BYTES,
            "managed recording storage limit"
        );
        file.write_all(buffer)?;
        #[cfg(test)]
        super::tests::fault("journal-write");
        file.flush()?;
        file.get_ref().sync_all()?;
        #[cfg(unix)]
        File::open(root)?.sync_all()?;
        *total += buffer.len() as u64;
        buffer.clear();
        shared.durable.store(tick, Ordering::Release);
        Ok(())
    })();
    budget.unlock()?;
    result
}
fn managed_size(root: &Path) -> u64 {
    session_directories(root)
        .iter()
        .map(|directory| {
            let size = std::fs::read_dir(directory)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|entry| entry.metadata().ok())
                .filter(|metadata| metadata.is_file())
                .map(|metadata| metadata.len())
                .sum::<u64>();
            if read_lease(directory).is_none() {
                size.max(SESSION_RESERVATION)
            } else {
                size
            }
        })
        .sum()
}
// The caller holds the budget lock, so journal flushes cannot change this
// inspection while retention and admission use it.
fn admit(root: &Path) -> Result<()> {
    let mut directories: Vec<_> = session_directories(root)
        .into_iter()
        .map(|directory| {
            let clean = inspect(&directory).ok().map(|record| record.clean);
            (directory, clean)
        })
        .collect();
    directories.sort_by(|(a, clean_a), (b, clean_b)| {
        (clean_a != &Some(true), a).cmp(&(clean_b != &Some(true), b))
    });
    let mut remaining = directories.len();
    let mut interrupted = directories
        .iter()
        .filter(|(_, clean)| *clean == Some(false))
        .count();
    for (directory, clean) in directories {
        if remaining < 5
            && interrupted < 3
            && managed_size(root).saturating_add(SESSION_RESERVATION) <= MANAGED_BYTES
        {
            break;
        }
        let Some(clean) = clean else {
            continue;
        };
        let Some(_lease) = inactive(&directory) else {
            continue;
        };
        let Ok(readers) = File::options()
            .read(true)
            .write(true)
            .open(directory.join("readers"))
        else {
            continue;
        };
        if readers.try_lock().is_err() {
            continue;
        }
        if std::fs::remove_dir_all(&directory).is_ok() {
            remaining -= 1;
            if !clean {
                interrupted = interrupted.saturating_sub(1);
            }
        }
    }
    ensure!(
        remaining < 5,
        "recovery session limit: existing records are protected"
    );
    ensure!(
        interrupted < 3,
        "interrupted recovery limit: existing records are protected"
    );
    ensure!(
        managed_size(root).saturating_add(SESSION_RESERVATION) <= MANAGED_BYTES,
        "recovery storage budget exhausted"
    );
    Ok(())
}
