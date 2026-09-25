use super::*;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc,
};

#[derive(Clone, Default)]
pub(super) struct Cancellation(Arc<AtomicU8>);
impl Cancellation {
    fn cancel(&self) -> bool {
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }
    fn commit(&self) -> Result<()> {
        self.0
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("load cancelled"))
    }
}

type Task = Box<dyn FnOnce(&Cancellation) -> Result<Output> + Send>;
struct Work {
    id: u64,
    cancel: Cancellation,
    task: Task,
}
pub(super) enum Output {
    Catalog {
        entries: Vec<crate::saves::ReplayEntry>,
        recovery: Option<PathBuf>,
    },
    Loaded(Box<crate::game::checkpoint::RestoredGame>),
    Saved,
    Retired,
}

pub(super) struct Worker {
    sender: mpsc::SyncSender<Work>,
    receiver: mpsc::Receiver<(u64, Result<Output>)>,
    active: Option<(u64, Cancellation)>,
    next: u64,
}
impl Worker {
    pub(super) fn new() -> Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<Work>(1);
        let (results, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("save-loader".into())
            .spawn(move || {
                while let Ok(work) = jobs.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (work.task)(&work.cancel)
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("persistence worker failed")));
                    if results.send((work.id, result)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender,
            receiver,
            active: None,
            next: 0,
        })
    }
    fn start(&mut self, task: Task) -> Result<u64> {
        anyhow::ensure!(self.active.is_none(), "persistence worker is busy");
        self.next += 1;
        let cancel = Cancellation::default();
        self.sender
            .try_send(Work {
                id: self.next,
                cancel: cancel.clone(),
                task,
            })
            .map_err(|_| anyhow::anyhow!("persistence worker is unavailable"))?;
        self.active = Some((self.next, cancel));
        Ok(self.next)
    }
    fn cancel(&self) -> bool {
        self.active
            .as_ref()
            .is_none_or(|(_, cancel)| cancel.cancel())
    }
    pub(super) fn busy(&self) -> bool {
        self.active.is_some()
    }
    fn poll(&mut self) -> Option<(u64, Result<Output>)> {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.active = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Disconnected) => self
                .active
                .take()
                .map(|(id, _)| (id, Err(anyhow::anyhow!("persistence worker stopped")))),
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}

pub(super) enum Intent {
    Load(PathBuf),
    Continue,
    Recover(PathBuf),
    Named(String),
    Leave(screens::pause::LeaveVerb, bool),
    Rematch,
}
impl Intent {
    fn loading(&self) -> bool {
        matches!(self, Self::Load(_) | Self::Continue | Self::Recover(_))
    }
}
pub(super) struct Busy {
    intent: Intent,
    back: Box<Screen>,
    id: Option<u64>,
    presented: bool,
    pub(super) menu: crate::menu::Menu,
    quit_after: bool,
}
impl Busy {
    pub(super) fn mode(&self) -> &'static str {
        if self.intent.loading() {
            "loading"
        } else {
            "saving"
        }
    }
    pub(super) fn request_quit(&mut self) {
        self.quit_after = true;
    }
}

impl App {
    pub(super) fn persistence_screen(&mut self, intent: Intent, back: Screen) -> Screen {
        self.game.presentation.paused = true;
        if self.catalog_id.take().is_some() {
            self.persistence.cancel();
        }
        let loading = intent.loading();
        Screen::Busy(Box::new(Busy {
            intent,
            back: Box::new(back),
            id: None,
            presented: false,
            quit_after: false,
            menu: crate::menu::Menu::new(
                if loading {
                    "LOADING GAME"
                } else {
                    "SAVING GAME"
                },
                if loading {
                    vec!["Cancel".into()]
                } else {
                    Vec::new()
                },
            ),
        }))
    }

    pub(super) fn poll_persistence(&mut self, screen: &mut Screen) {
        if self.catalog_delete.is_some() && !self.persistence.busy() {
            self.request_catalog();
        }
        let Some((id, result)) = self.persistence.poll() else {
            return;
        };
        if self.catalog_id == Some(id) {
            self.catalog_id = None;
            if let Err(error) = &result {
                self.menu_notice = Some((
                    format!("Could not refresh saves: {error:#}"),
                    get_time() + 8.0,
                ));
            }
            if let Ok(Output::Catalog { entries, recovery }) = result {
                match screen {
                    Screen::Home(home) => home.set_catalog(
                        entries
                            .iter()
                            .any(|e| e.compatible && e.kind == crate::saves::RecordKind::Autosave),
                        recovery,
                    ),
                    Screen::Replays(shelf) => shelf.set_catalog(entries),
                    _ => {}
                }
            }
        } else if matches!(screen, Screen::Busy(busy) if busy.id == Some(id)) {
            self.persistence_result = Some(result);
        }
    }

    pub(super) fn request_catalog(&mut self) {
        if self.persistence.busy() || self.catalog_id.is_some() {
            return;
        }
        let delete = self.catalog_delete.take();
        let task: Task = Box::new(move |cancel| {
            if let Some(path) = delete {
                std::fs::remove_file(path)?;
            }
            let entries = crate::saves::discover(|| cancel.cancelled());
            anyhow::ensure!(!cancel.cancelled(), "catalog cancelled");
            let recovery = crate::paths::recovery_dir().and_then(|root| {
                oxide_kit::recovery::interrupted_candidates(&root)
                    .into_iter()
                    .next()
            });
            Ok(Output::Catalog { entries, recovery })
        });
        match self.persistence.start(task) {
            Ok(id) => self.catalog_id = Some(id),
            Err(error) => self.menu_notice = Some((error.to_string(), get_time() + 5.0)),
        }
    }
}

fn load_candidates(
    paths: Vec<PathBuf>,
    recovery: bool,
    root: Option<PathBuf>,
    cancel: &Cancellation,
) -> Result<Output> {
    let mut last_error = anyhow::anyhow!("no loadable save is available");
    for path in paths {
        anyhow::ensure!(!cancel.cancelled(), "load cancelled");
        let result = if recovery {
            oxide_kit::recovery::inspect(&path).and_then(|record| {
                crate::game::checkpoint::RestoredGame::recover(record, || cancel.cancelled())
            })
        } else {
            crate::saved_game::prepare_load(&path)
        };
        match result {
            Ok(mut loaded) => {
                cancel.commit()?;
                loaded.start_recovery(root, recovery.then_some(path))?;
                return Ok(Output::Loaded(Box::new(loaded)));
            }
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

pub(super) fn frame(app: &mut App, mut busy: Box<Busy>, events: &[RawEvent]) -> Result<Screen> {
    render::draw(&app.game.view(), &app.sprites, &app.input);
    veil();
    let loading = busy.intent.loading();
    let dots = match (get_time() * 3.0) as u64 % 3 {
        0 => ".",
        1 => "..",
        _ => "...",
    };
    busy.menu.draw(&format!(
        "{}{}",
        if loading {
            "Loading game"
        } else {
            "Saving game"
        },
        dots
    ));
    if loading
        && app.persistence_result.is_none()
        && (busy.menu.handle(events, &mut app.input.mouse).is_some()
            || events.iter().any(|event| {
                matches!(
                    event,
                    RawEvent::KeyDown {
                        key: oxide_protocol::Key::Escape
                    }
                )
            }))
        && (busy.id.is_none() || app.persistence.cancel())
    {
        app.persistence_result = None;
        return Ok(*busy.back);
    }
    if let Some(result) = app.persistence_result.take() {
        match result {
            Ok(Output::Loaded(loaded)) => {
                let fresh = keep_flags(loaded.install(), &app.game);
                let retired = std::mem::replace(&mut app.game, fresh).retire();
                app.game.presentation.paused = true;
                app.tutorial = None;
                app.performance.reset();
                app.input.reset_session();
                app.persistence.start(Box::new(move |_| {
                    retired();
                    Ok(Output::Retired)
                }))?;
                if busy.quit_after {
                    return Ok(app.persistence_screen(
                        Intent::Leave(screens::pause::LeaveVerb::Quit, false),
                        Screen::Playing,
                    ));
                }
                return Ok(Screen::Playing);
            }
            Ok(Output::Saved) => {
                if !matches!(busy.intent, Intent::Named(_)) {
                    app.game.autosave_done = true;
                }
                if busy.quit_after {
                    return Ok(app.persistence_screen(
                        Intent::Leave(screens::pause::LeaveVerb::Quit, false),
                        *busy.back,
                    ));
                }
                return Ok(match busy.intent {
                    Intent::Named(name) => {
                        if let Screen::Pause(ref mut pause) = *busy.back {
                            pause.end_naming(format!("saved: {name}"));
                        }
                        *busy.back
                    }
                    Intent::Leave(screens::pause::LeaveVerb::Quit, _) => std::process::exit(0),
                    Intent::Leave(_, _) => Screen::Home(HomeScreen::open()),
                    Intent::Rematch => {
                        app.install_session(
                            Game::new(app.game.scenario.clone())?,
                            app.args.paused,
                            None,
                        );
                        Screen::Playing
                    }
                    _ => unreachable!(),
                });
            }
            Err(error) => {
                eprintln!(
                    "{} failed: {error:#}",
                    if loading { "Load" } else { "Save" }
                );
                if busy.quit_after {
                    return Ok(Screen::Pause(PauseScreen::open_save_failed(
                        error.to_string(),
                        screens::pause::LeaveVerb::Quit,
                        app.game.state.result().is_some(),
                        can_surrender(&app.game),
                        false,
                    )));
                }
                if let Intent::Leave(verb, home) = busy.intent {
                    return Ok(Screen::Pause(PauseScreen::open_save_failed(
                        error.to_string(),
                        verb,
                        app.game.state.result().is_some(),
                        can_surrender(&app.game),
                        home,
                    )));
                }
                if matches!(busy.intent, Intent::Recover(_))
                    && let Screen::Home(home) = &mut *busy.back
                {
                    home.clear_recovery();
                }
                app.menu_notice = Some((
                    if loading {
                        format!("Load failed: {error}")
                    } else {
                        error.to_string()
                    },
                    get_time() + 8.0,
                ));
                if let Screen::Pause(ref mut pause) = *busy.back {
                    pause.end_naming(error.to_string());
                }
                return Ok(*busy.back);
            }
            Ok(Output::Catalog { .. } | Output::Retired) => unreachable!(),
        }
    }
    if !busy.presented {
        busy.presented = true;
        return Ok(Screen::Busy(busy));
    }
    if busy.id.is_none() && !app.persistence.busy() {
        let task: Task = match &busy.intent {
            Intent::Named(_) | Intent::Leave(_, _) | Intent::Rematch => {
                let name = if let Intent::Named(name) = &busy.intent {
                    Some(name.as_str())
                } else {
                    None
                };
                let job = autosave::SaveJob::capture(&app.game, name);
                Box::new(move |_| {
                    job.run().map_err(|error| {
                        let line = error.player_line();
                        anyhow::Error::new(error).context(line)
                    })?;
                    Ok(Output::Saved)
                })
            }
            Intent::Load(_) | Intent::Continue | Intent::Recover(_) => {
                let paths = match &busy.intent {
                    Intent::Load(path) | Intent::Recover(path) => Some(vec![path.clone()]),
                    _ => None,
                };
                let recovery = matches!(busy.intent, Intent::Recover(_));
                let root = app.game.recovery_root.clone();
                Box::new(move |cancel| {
                    let mut paths = paths.unwrap_or_else(autosave::candidates);
                    if recovery && let Some(root) = &root {
                        for candidate in oxide_kit::recovery::interrupted_candidates(root) {
                            if !paths.contains(&candidate) {
                                paths.push(candidate);
                            }
                        }
                    }
                    load_candidates(paths, recovery, root, cancel)
                })
            }
        };
        match app.persistence.start(task) {
            Ok(id) => busy.id = Some(id),
            Err(error) => app.persistence_result = Some(Err(error)),
        }
    }
    Ok(Screen::Busy(busy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn completion(worker: &mut Worker) -> (u64, Result<Output>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = worker.poll() {
                return result;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn cancellation_keeps_admission_until_the_worker_releases_its_capture() {
        let mut worker = Worker::new().unwrap();
        let (started, observed) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let id = worker
            .start(Box::new(move |cancel| {
                started.send(()).unwrap();
                blocked.recv().unwrap();
                anyhow::ensure!(!cancel.cancelled(), "cancelled");
                Ok(Output::Saved)
            }))
            .unwrap();
        observed.recv().unwrap();
        assert!(worker.cancel());
        assert!(worker.start(Box::new(|_| Ok(Output::Saved))).is_err());
        release.send(()).unwrap();
        let (finished, result) = completion(&mut worker);
        assert_eq!(id, finished);
        assert!(result.is_err());
        assert!(!worker.busy());
        let next = worker
            .start(Box::new(|cancel| {
                cancel.commit()?;
                assert!(!cancel.cancel());
                Ok(Output::Saved)
            }))
            .unwrap();
        assert_ne!(id, next);
        assert!(completion(&mut worker).1.is_ok());
    }

    #[test]
    fn a_failed_job_does_not_poison_the_worker() {
        let mut worker = Worker::new().unwrap();
        worker
            .start(Box::new(|_| panic!("injected worker failure")))
            .unwrap();
        assert!(completion(&mut worker).1.is_err());
        worker.start(Box::new(|_| Ok(Output::Saved))).unwrap();
        assert!(completion(&mut worker).1.is_ok());
    }

    #[test]
    fn continue_skips_a_corrupt_newest_payload_and_reuses_the_restored_older_save() {
        let dir =
            std::env::temp_dir().join(format!("oxide-continue-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut game = Game::new(Scenario::skirmish()).unwrap();
        game.advance_ticks(120);
        let mut meta = game.recorder.meta.clone();
        meta.kind = Some("autosave".into());
        meta.ticks = Some(game.state.current_tick());
        let older = dir.join("older.oxsave");
        crate::saved_game::write_capture(game.capture_save(), meta, &older).unwrap();
        let newer = dir.join("newer.oxsave");
        let mut corrupt = std::fs::read(&older).unwrap();
        *corrupt.last_mut().unwrap() ^= 1;
        std::fs::write(&newer, corrupt).unwrap();
        assert!(
            crate::saved_game::inspect(&newer)
                .unwrap()
                .problem
                .is_none()
        );
        let Output::Loaded(restored) = load_candidates(
            vec![newer, older.clone()],
            false,
            None,
            &Cancellation::default(),
        )
        .unwrap() else {
            panic!()
        };
        let mut restored = restored.install();
        for _ in 0..120 {
            assert_eq!(game.do_tick().events, restored.do_tick().events);
            assert_eq!(game.hash_hex(), restored.hash_hex());
        }
        let cancelled = Cancellation::default();
        assert!(cancelled.cancel());
        assert!(
            load_candidates(vec![older], false, Some(dir.join("recovery")), &cancelled).is_err()
        );
        assert!(!dir.join("recovery").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
