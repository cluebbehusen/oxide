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
        let active = game.recovery.clone();
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
