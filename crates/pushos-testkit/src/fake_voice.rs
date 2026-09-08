//! Fakes for the voice ports.
//!
//! Let push to talk be exercised with no microphone, no permission and no
//! speech model, which is what the test suite has and what a machine building
//! PushOS in a container has.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use pushos_domain::error::ErrorClass;
use pushos_domain::ports::{Microphone, Recording, SAMPLE_RATE, Transcriber, VoiceError};
use pushos_domain::voice::Utterance;

/// A microphone that hears whatever a test puts in front of it.
#[derive(Debug, Clone, Default)]
pub struct FakeMicrophone {
    recording: Arc<Mutex<bool>>,
    /// What the next release will hand back.
    next: Arc<Mutex<Option<Recording>>>,
    starts: Arc<Mutex<usize>>,
    discards: Arc<Mutex<usize>>,
    fail_with: Arc<Mutex<Option<VoiceError2>>>,
}

/// The failure a fake was told to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceError2 {
    NotPermitted,
    Broken,
}

impl FakeMicrophone {
    /// Builds a microphone that hears a second of speech-like sound.
    pub fn new() -> Self {
        let fake = Self::default();
        fake.will_hear(speech(Duration::from_secs(1)));
        fake
    }

    /// Sets what the next release hands back.
    pub fn will_hear(&self, recording: Recording) {
        *lock(&self.next) = Some(recording);
    }

    /// Sets the microphone to hear nothing but room noise.
    pub fn will_hear_nothing(&self) {
        *lock(&self.next) = Some(Recording::new(vec![0.0; 1600]));
    }

    /// How many times recording began.
    pub fn starts(&self) -> usize {
        *lock(&self.starts)
    }

    /// How many times a recording was thrown away.
    pub fn discards(&self) -> usize {
        *lock(&self.discards)
    }

    /// Makes every subsequent call fail as a refused permission.
    pub fn refuse(&self) {
        *lock(&self.fail_with) = Some(VoiceError2::NotPermitted);
    }

    /// Makes every subsequent call fail as a fault.
    pub fn break_it(&self) {
        *lock(&self.fail_with) = Some(VoiceError2::Broken);
    }

    fn check(&self) -> Result<(), VoiceError> {
        match *lock(&self.fail_with) {
            None => Ok(()),
            Some(VoiceError2::NotPermitted) => Err(VoiceError::NotPermitted {
                what: "the microphone".to_owned(),
            }),
            Some(VoiceError2::Broken) => Err(VoiceError::backend(
                "the fake was told to fail",
                ErrorClass::ComponentFailure,
                std::io::Error::other("injected failure"),
            )),
        }
    }
}

#[async_trait]
impl Microphone for FakeMicrophone {
    async fn start(&self) -> Result<(), VoiceError> {
        self.check()?;
        *lock(&self.recording) = true;
        *lock(&self.starts) += 1;
        Ok(())
    }

    async fn stop(&self) -> Result<Option<Recording>, VoiceError> {
        self.check()?;
        if !std::mem::replace(&mut *lock(&self.recording), false) {
            return Ok(None);
        }
        Ok(lock(&self.next).clone())
    }

    async fn discard(&self) {
        *lock(&self.recording) = false;
        *lock(&self.discards) += 1;
    }

    async fn is_recording(&self) -> bool {
        *lock(&self.recording)
    }
}

/// A transcriber that says whatever a test told it to say.
#[derive(Debug, Clone, Default)]
pub struct FakeTranscriber {
    words: Arc<Mutex<String>>,
    heard: Arc<Mutex<Vec<Recording>>>,
    fail: Arc<Mutex<bool>>,
}

impl FakeTranscriber {
    /// Builds a transcriber that hears nothing until told otherwise.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets what it will say next.
    pub fn will_say(&self, words: &str) {
        lock(&self.words).replace_range(.., words);
    }

    /// Everything it was asked to work out, in order.
    pub fn heard(&self) -> Vec<Recording> {
        lock(&self.heard).clone()
    }

    /// Makes every subsequent call fail.
    pub fn fail(&self) {
        *lock(&self.fail) = true;
    }
}

#[async_trait]
impl Transcriber for FakeTranscriber {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn transcribe(&self, recording: &Recording) -> Result<Utterance, VoiceError> {
        if *lock(&self.fail) {
            return Err(VoiceError::backend(
                "the fake was told to fail",
                ErrorClass::ComponentFailure,
                std::io::Error::other("injected failure"),
            ));
        }

        lock(&self.heard).push(recording.clone());
        let words = lock(&self.words).clone();
        Ok(Utterance::new(words, recording.duration))
    }
}

/// Sound loud enough not to be mistaken for an empty room.
pub fn speech(length: Duration) -> Recording {
    let millis = u32::try_from(length.as_millis()).unwrap_or(u32::MAX);
    let samples = millis / 1000 * SAMPLE_RATE + length.subsec_millis() * SAMPLE_RATE / 1000;
    Recording::new((0..samples).map(wave).collect())
}

/// One sample of something loud enough to be taken for speech.
#[allow(clippy::cast_precision_loss)]
fn wave(index: u32) -> f32 {
    0.3 * f32::sin(index as f32 * 0.05)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_release_with_no_press_before_it_hands_back_nothing() {
        let microphone = FakeMicrophone::new();
        assert!(microphone.stop().await.expect("no fault").is_none());
    }

    #[tokio::test]
    async fn what_was_heard_comes_back_on_the_release() {
        let microphone = FakeMicrophone::new();
        microphone.start().await.expect("no fault");
        assert!(microphone.is_recording().await);

        let heard = microphone.stop().await.expect("no fault");
        assert!(heard.is_some_and(|recording| !recording.is_silent()));
        assert!(!microphone.is_recording().await);
    }

    #[tokio::test]
    async fn a_refused_microphone_says_so_rather_than_failing_vaguely() {
        let microphone = FakeMicrophone::new();
        microphone.refuse();

        let error = microphone.start().await.expect_err("it was refused");
        assert_eq!(error.class(), ErrorClass::Permission);
    }

    #[tokio::test]
    async fn a_transcriber_says_what_it_was_told_and_records_what_it_heard() {
        let transcriber = FakeTranscriber::new();
        transcriber.will_say("stop");

        let said = transcriber
            .transcribe(&speech(Duration::from_secs(1)))
            .await
            .expect("no fault");

        assert_eq!(said.text, "stop");
        assert_eq!(transcriber.heard().len(), 1);
    }
}
