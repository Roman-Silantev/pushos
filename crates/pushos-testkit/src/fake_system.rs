//! Fakes for the host operating system ports.
//!
//! Each fake records what it was asked to do and can be told to fail, so tests
//! can assert on both the happy path and the error classification without
//! touching the real machine.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ports::{
    ApplicationLauncher, ApplicationTarget, MediaController, MediaSnapshot, ProcessOutcome,
    ProcessRunner, ProcessSpec, ShortcutRunner,
};

/// Shared recording of calls made against a fake.
#[derive(Debug)]
struct Recorder<T> {
    calls: Mutex<Vec<T>>,
    fail_with: Mutex<Option<ErrorClass>>,
}

impl<T> Recorder<T> {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            fail_with: Mutex::new(None),
        })
    }

    fn record(&self, call: T) -> Result<(), ActionError> {
        self.check()?;
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
        Ok(())
    }

    fn check(&self) -> Result<(), ActionError> {
        let class = self.fail_with.lock().ok().and_then(|guard| *guard);
        match class {
            None => Ok(()),
            Some(class) => Err(ActionError::backend(
                "the fake was told to fail",
                class,
                std::io::Error::other("injected failure"),
            )),
        }
    }

    fn fail(&self, class: ErrorClass) {
        if let Ok(mut guard) = self.fail_with.lock() {
            *guard = Some(class);
        }
    }
}

impl<T: Clone> Recorder<T> {
    fn calls(&self) -> Vec<T> {
        self.calls.lock().map_or_default(|calls| calls.clone())
    }
}

/// What a fake media controller was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaCall {
    /// Playback was toggled.
    PlayPause,
    /// The next track was requested.
    Next,
    /// The previous track was requested.
    Previous,
    /// Volume was set to a percentage.
    Volume(u8),
}

/// A media controller that plays nothing.
#[derive(Debug, Clone)]
pub struct FakeMedia {
    recorder: Arc<Recorder<MediaCall>>,
    playing: Arc<Mutex<Option<MediaSnapshot>>>,
    volume: Arc<Mutex<Option<u8>>>,
}

impl FakeMedia {
    /// Builds a controller with nothing playing.
    pub fn new() -> Self {
        Self {
            recorder: Recorder::new(),
            playing: Arc::new(Mutex::new(None)),
            volume: Arc::new(Mutex::new(Some(50))),
        }
    }

    /// Sets what [`MediaController::volume`] will report.
    pub fn set_reported_volume(&self, percent: Option<u8>) {
        if let Ok(mut volume) = self.volume.lock() {
            *volume = percent;
        }
    }

    /// Sets what [`MediaController::now_playing`] will report.
    pub fn set_now_playing(&self, snapshot: MediaSnapshot) {
        if let Ok(mut playing) = self.playing.lock() {
            *playing = Some(snapshot);
        }
    }

    /// Makes every subsequent call fail with the given classification.
    pub fn fail_with(&self, class: ErrorClass) {
        self.recorder.fail(class);
    }

    /// Everything the controller was asked to do, in order.
    pub fn calls(&self) -> Vec<MediaCall> {
        self.recorder.calls()
    }
}

impl Default for FakeMedia {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MediaController for FakeMedia {
    async fn play_pause(&self) -> Result<(), ActionError> {
        self.recorder.record(MediaCall::PlayPause)
    }

    async fn next_track(&self) -> Result<(), ActionError> {
        self.recorder.record(MediaCall::Next)
    }

    async fn previous_track(&self) -> Result<(), ActionError> {
        self.recorder.record(MediaCall::Previous)
    }

    async fn set_volume(&self, percent: u8) -> Result<(), ActionError> {
        self.recorder.record(MediaCall::Volume(percent))?;
        if let Ok(mut volume) = self.volume.lock() {
            *volume = Some(percent);
        }
        Ok(())
    }

    async fn now_playing(&self) -> Result<Option<MediaSnapshot>, ActionError> {
        self.recorder.check()?;
        Ok(self.playing.lock().ok().and_then(|playing| playing.clone()))
    }

    async fn volume(&self) -> Result<Option<u8>, ActionError> {
        self.recorder.check()?;
        Ok(self.volume.lock().ok().and_then(|volume| *volume))
    }
}

/// An application launcher that opens nothing.
#[derive(Debug, Clone)]
pub struct FakeApplications {
    recorder: Arc<Recorder<ApplicationTarget>>,
}

impl FakeApplications {
    /// Builds a launcher.
    pub fn new() -> Self {
        Self {
            recorder: Recorder::new(),
        }
    }

    /// Makes every subsequent call fail with the given classification.
    pub fn fail_with(&self, class: ErrorClass) {
        self.recorder.fail(class);
    }

    /// Everything the launcher was asked to open, in order.
    pub fn opened(&self) -> Vec<ApplicationTarget> {
        self.recorder.calls()
    }
}

impl Default for FakeApplications {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApplicationLauncher for FakeApplications {
    async fn open(&self, target: &ApplicationTarget) -> Result<(), ActionError> {
        self.recorder.record(target.clone())
    }
}

/// A Shortcuts runner that runs nothing.
#[derive(Debug, Clone)]
pub struct FakeShortcuts {
    recorder: Arc<Recorder<(String, Option<String>)>>,
    available: Arc<Mutex<Vec<String>>>,
}

impl FakeShortcuts {
    /// Builds a runner offering the given Shortcut names.
    pub fn with_shortcuts(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            recorder: Recorder::new(),
            available: Arc::new(Mutex::new(names.into_iter().collect())),
        }
    }

    /// Makes every subsequent call fail with the given classification.
    pub fn fail_with(&self, class: ErrorClass) {
        self.recorder.fail(class);
    }

    /// Everything that was run, with any input passed to it.
    pub fn runs(&self) -> Vec<(String, Option<String>)> {
        self.recorder.calls()
    }
}

impl Default for FakeShortcuts {
    fn default() -> Self {
        Self::with_shortcuts([])
    }
}

#[async_trait]
impl ShortcutRunner for FakeShortcuts {
    async fn list(&self) -> Result<Vec<String>, ActionError> {
        self.recorder.check()?;
        Ok(self
            .available
            .lock()
            .map(|names| names.clone())
            .unwrap_or_default())
    }

    async fn run(&self, name: &str, input: Option<&str>) -> Result<(), ActionError> {
        self.recorder
            .record((name.to_owned(), input.map(ToOwned::to_owned)))
    }
}

/// A process runner that starts nothing.
#[derive(Debug, Clone)]
pub struct FakeProcesses {
    recorder: Arc<Recorder<ProcessSpec>>,
    exit_code: Arc<Mutex<i32>>,
}

impl FakeProcesses {
    /// Builds a runner whose processes all succeed.
    pub fn new() -> Self {
        Self {
            recorder: Recorder::new(),
            exit_code: Arc::new(Mutex::new(0)),
        }
    }

    /// Sets the exit status every subsequent process will report.
    pub fn set_exit_code(&self, code: i32) {
        if let Ok(mut exit_code) = self.exit_code.lock() {
            *exit_code = code;
        }
    }

    /// Makes every subsequent call fail with the given classification.
    pub fn fail_with(&self, class: ErrorClass) {
        self.recorder.fail(class);
    }

    /// Every process that was requested, in order.
    pub fn spawned(&self) -> Vec<ProcessSpec> {
        self.recorder.calls()
    }
}

impl Default for FakeProcesses {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProcessRunner for FakeProcesses {
    async fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutcome, ActionError> {
        self.recorder.record(spec.clone())?;
        Ok(ProcessOutcome {
            exit_code: Some(self.exit_code.lock().map_or(0, |code| *code)),
            stdout_tail: String::new(),
            stderr_tail: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_media_controller_records_what_it_was_asked_to_do() {
        let media = FakeMedia::new();
        media
            .play_pause()
            .await
            .expect("the fake succeeds by default");
        media
            .set_volume(40)
            .await
            .expect("the fake succeeds by default");

        assert_eq!(media.calls(), [MediaCall::PlayPause, MediaCall::Volume(40)]);
    }

    #[tokio::test]
    async fn an_injected_failure_keeps_its_classification() {
        let media = FakeMedia::new();
        media.fail_with(ErrorClass::Retryable);

        let error = media
            .play_pause()
            .await
            .expect_err("the fake was told to fail");
        assert_eq!(error.class(), ErrorClass::Retryable);
        assert!(
            media.calls().is_empty(),
            "a failed call is not recorded as done"
        );
    }

    #[tokio::test]
    async fn a_process_runner_reports_the_exit_code_it_was_given() {
        let processes = FakeProcesses::new();
        processes.set_exit_code(3);

        let outcome = processes
            .run(&ProcessSpec::new("cargo", ["test".to_owned()]))
            .await
            .expect("the fake succeeds by default");

        assert!(!outcome.succeeded());
        assert_eq!(processes.spawned().len(), 1);
        assert_eq!(processes.spawned()[0].program, "cargo");
    }

    #[tokio::test]
    async fn a_shortcut_runner_reports_the_names_it_was_configured_with() {
        let shortcuts = FakeShortcuts::with_shortcuts(["Office Lights".to_owned()]);
        assert_eq!(
            shortcuts.list().await.expect("listing succeeds"),
            ["Office Lights"]
        );

        shortcuts
            .run("Office Lights", Some("on"))
            .await
            .expect("running succeeds");
        assert_eq!(
            shortcuts.runs(),
            [("Office Lights".to_owned(), Some("on".to_owned()))]
        );
    }
}
