//! Player-requested local exports; all serialization and copying happens off-frame.
use anyhow::{Context, Result, ensure};
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub(crate) struct ReportJob {
    receiver: Option<Receiver<Result<String, String>>>,
}
impl ReportJob {
    fn begin(
        &mut self,
        name: &str,
        work: impl FnOnce() -> Result<String> + Send + 'static,
    ) -> Result<()> {
        ensure!(
            self.receiver.is_none(),
            "a diagnostic operation is already in progress"
        );
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = sender.send(work().map_err(|error| format!("{error:#}")));
            })?;
        self.receiver = Some(receiver);
        Ok(())
    }
    pub(crate) fn start(&mut self, game: &crate::game::Game) -> Result<()> {
        let root = game
            .recovery_root
            .clone()
            .or_else(crate::paths::recovery_dir)
            .context("diagnostics folder unavailable")?;
        let active = (game.state.current_tick() > 0)
            .then(|| game.recovery.clone())
            .flatten();
        self.begin("oxide-report-export", move || {
            let source = active
                .as_ref()
                .map(|writer| writer.directory().to_owned())
                .or_else(|| {
                    oxide_kit::recovery::latest_interrupted(&root).map(|record| record.directory)
                })
                .context("no recorded match is available")?;
            let reports = root.join("reports");
            std::fs::create_dir_all(&reports)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            oxide_kit::recovery::export(&source, &reports.join(format!("report-{stamp}")))?;
            Ok("Diagnostic report exported. Open diagnostics folder to find it.".into())
        })
    }
    pub(crate) fn poll(&mut self) -> Option<Result<String, String>> {
        match self.receiver.as_ref()?.try_recv() {
            Ok(result) => {
                self.receiver = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                Some(Err("diagnostic worker stopped".into()))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
    pub(crate) fn open_folder(&mut self) -> Result<()> {
        let root = crate::paths::recovery_dir().context("diagnostics folder unavailable")?;
        self.begin("oxide-open-diagnostics", move || {
            std::fs::create_dir_all(&root)?;
            #[cfg(target_os = "macos")]
            let mut command = std::process::Command::new("open");
            #[cfg(target_os = "windows")]
            let mut command = std::process::Command::new("explorer");
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let mut command = std::process::Command::new("xdg-open");
            ensure!(
                command.arg(root).status()?.success(),
                "file manager could not open the diagnostics folder"
            );
            Ok("Diagnostics folder opened.".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn home_exports_the_interrupted_match_instead_of_its_empty_active_backdrop() {
        use oxide_kit::recovery::{RecoveryWriter, inspect};
        let root = std::env::temp_dir().join(format!("oxide-report-home-{}", std::process::id()));
        let base =
            oxide_kit::GameReplay::new(oxide_sim::SIM_VERSION, oxide_sim::Scenario::skirmish());
        let interrupted = RecoveryWriter::start(root.clone(), base, 0).unwrap();
        interrupted.prepared(0, &[]);
        interrupted.completed(1);
        until(|| interrupted.status().durable_tick == 1);
        let source = interrupted.directory().to_owned();
        drop(interrupted);
        until(|| {
            std::fs::File::open(source.join("lease"))
                .unwrap()
                .try_lock()
                .is_ok()
        });

        let mut game = crate::game::Game::new(oxide_sim::Scenario::skirmish()).unwrap();
        game.recovery_root = Some(root.clone());
        game.start_recovery();
        until(|| game.recovery.as_ref().unwrap().status().ready);
        let mut job = ReportJob::default();
        job.start(&game).unwrap();
        assert!(
            job.start(&game).is_err(),
            "only one export can run at a time"
        );
        until(|| job.poll().is_some_and(|result| result.is_ok()));
        let report = std::fs::read_dir(root.join("reports"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(inspect(&report).unwrap().replay.meta.ticks, Some(1));
        let active = game.recovery.as_ref().unwrap().directory().to_owned();
        drop(game);
        until(|| {
            std::fs::File::open(active.join("lease"))
                .unwrap()
                .try_lock()
                .is_ok()
        });
        std::fs::remove_dir_all(root).unwrap();
    }
}
