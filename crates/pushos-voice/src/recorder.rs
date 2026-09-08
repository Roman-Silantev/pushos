//! Keeping what the operator says while a control is held.
//!
//! The audio device is opened when listening starts and closed when it stops,
//! rather than held open for the life of the process. A microphone that is only
//! live while a finger is down is one an operator can reason about, and on a
//! Mac it is also the difference between the orange dot appearing when they
//! expect it and appearing all day.
//!
//! An input stream cannot be moved between threads on macOS, so one thread owns
//! it for its whole life and is spoken to through a channel.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use pushos_domain::error::ErrorClass;
use pushos_domain::ports::{Microphone, Recording, SAMPLE_RATE, VoiceError};
use tracing::{debug, warn};

/// The operator's microphone, through `CoreAudio`.
#[derive(Debug)]
pub struct CoreAudioMicrophone {
    commands: mpsc::Sender<Command>,
    recording: Arc<Mutex<bool>>,
}

/// What the thread owning the audio device is asked to do.
enum Command {
    Start(mpsc::Sender<Result<(), String>>),
    Stop(mpsc::Sender<Option<Vec<f32>>>),
    Discard,
}

impl CoreAudioMicrophone {
    /// Opens nothing yet, and starts the thread that will.
    ///
    /// Building this does not touch the microphone, so PushOS starts on a
    /// machine that has refused it, and says so only when asked to listen.
    pub fn new() -> Self {
        let (commands, inbox) = mpsc::channel();
        let recording = Arc::new(Mutex::new(false));

        let held = Arc::clone(&recording);
        std::thread::Builder::new()
            .name("pushos-microphone".to_owned())
            .spawn(move || serve(&inbox, &held))
            .map_err(|error| warn!(%error, "no thread was available for the microphone"))
            .ok();

        Self {
            commands,
            recording,
        }
    }

    fn ask<T>(&self, build: impl FnOnce(mpsc::Sender<T>) -> Command) -> Option<T> {
        let (reply, answer) = mpsc::channel();
        self.commands.send(build(reply)).ok()?;
        answer.recv().ok()
    }
}

impl Default for CoreAudioMicrophone {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Microphone for CoreAudioMicrophone {
    async fn start(&self) -> Result<(), VoiceError> {
        match self.ask(Command::Start) {
            Some(Ok(())) => Ok(()),
            Some(Err(why)) => Err(classify(&why)),
            None => Err(VoiceError::Unavailable {
                context: "the microphone thread is gone; restart PushOS".to_owned(),
            }),
        }
    }

    async fn stop(&self) -> Result<Option<Recording>, VoiceError> {
        Ok(self.ask(Command::Stop).flatten().map(Recording::new))
    }

    async fn discard(&self) {
        let _ = self.commands.send(Command::Discard);
    }

    async fn is_recording(&self) -> bool {
        self.recording.lock().is_ok_and(|recording| *recording)
    }
}

/// Owns the audio device for the life of the process.
fn serve(inbox: &mpsc::Receiver<Command>, recording: &Arc<Mutex<bool>>) {
    let mut open: Option<Session> = None;

    while let Ok(command) = inbox.recv() {
        match command {
            Command::Start(reply) => {
                // Starting again is an operator changing their mind, so
                // whatever was being kept goes, and the device is released
                // before another is opened.
                drop(open.take());
                let started = Session::open();
                let answer = match &started {
                    Ok(_) => Ok(()),
                    Err(why) => Err(why.clone()),
                };
                open = started.ok();
                set(recording, open.is_some());
                let _ = reply.send(answer);
            }

            Command::Stop(reply) => {
                set(recording, false);
                let _ = reply.send(open.take().map(Session::take));
            }

            Command::Discard => {
                set(recording, false);
                open = None;
            }
        }
    }
}

fn set(recording: &Arc<Mutex<bool>>, value: bool) {
    if let Ok(mut held) = recording.lock() {
        *held = value;
    }
}

/// One open input stream and what it has heard.
struct Session {
    stream: cpal::Stream,
    heard: Arc<Mutex<Vec<f32>>>,
    /// How many device samples make one of ours.
    every: usize,
    channels: usize,
}

impl Session {
    /// Opens the default input device and starts keeping what it hears.
    fn open() -> Result<Self, String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| "no microphone is attached".to_owned())?;

        let config = device
            .default_input_config()
            .map_err(|error| format!("could not open the microphone: {error}"))?;

        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        // Devices run at 44100 or 48000; speech recognition wants 16000. Whole
        // numbers divide cleanly enough that averaging groups is both a
        // decimator and a crude low pass, which is what stops the high
        // frequencies folding back down as noise.
        let every = (rate as usize / SAMPLE_RATE as usize).max(1);

        let heard = Arc::new(Mutex::new(Vec::<f32>::new()));
        let writing = Arc::clone(&heard);

        let stream = device
            .build_input_stream(
                config.config(),
                move |samples: &[f32], _| {
                    if let Ok(mut kept) = writing.lock() {
                        kept.extend_from_slice(samples);
                    }
                },
                |error| warn!(%error, "the microphone stopped"),
                None,
            )
            .map_err(|error| format!("could not start the microphone: {error}"))?;

        stream
            .play()
            .map_err(|error| format!("could not start the microphone: {error}"))?;

        debug!(rate, channels, "listening");
        Ok(Self {
            stream,
            heard,
            every,
            channels,
        })
    }

    /// Stops and hands back what was heard, at the rate everything else wants.
    fn take(self) -> Vec<f32> {
        drop(self.stream);
        let raw = self
            .heard
            .lock()
            .map(|kept| kept.clone())
            .unwrap_or_default();

        resample(&raw, self.channels, self.every)
    }
}

/// Mixes to mono and brings the rate down to [`SAMPLE_RATE`].
///
/// Averaging rather than picking every nth sample: dropping samples folds
/// everything above the new limit back down as noise, and noise is exactly what
/// a speech model is worst at.
fn resample(raw: &[f32], channels: usize, every: usize) -> Vec<f32> {
    if raw.is_empty() || channels == 0 {
        return Vec::new();
    }

    let frames = raw.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for frame in 0..frames {
        let start = frame * channels;
        let sum: f32 = raw[start..start + channels].iter().sum();
        let count = u16::try_from(channels).unwrap_or(u16::MAX);
        mono.push(sum / f32::from(count));
    }

    if every <= 1 {
        return mono;
    }

    mono.chunks(every)
        .map(|group| {
            // A group is at most a handful of samples, so the count is exact
            // as a float however wide the platform's usize is.
            let count = u16::try_from(group.len()).unwrap_or(u16::MAX);
            group.iter().sum::<f32>() / f32::from(count)
        })
        .collect()
}

/// Tells a refused microphone apart from a broken one.
///
/// The operator can fix one of these and not the other, and `CoreAudio` reports
/// both as an opaque number.
fn classify(why: &str) -> VoiceError {
    let refused = why.contains("560947818") // '!obj', what a denied device reports
        || why.contains("561017449") // '!pri', not permitted
        || why.to_lowercase().contains("permission")
        || why.to_lowercase().contains("not authorized");

    if refused {
        VoiceError::NotPermitted {
            what: "the microphone".to_owned(),
        }
    } else {
        VoiceError::backend(
            why.to_owned(),
            ErrorClass::ComponentFailure,
            std::io::Error::other(why.to_owned()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_channels_become_one_by_averaging_them() {
        // Picking one channel loses whatever was said into the other.
        let stereo = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(resample(&stereo, 2, 1), [0.5, 0.5, 0.0]);
    }

    #[test]
    fn bringing_the_rate_down_averages_rather_than_dropping_samples() {
        // Dropping folds everything above the new limit back down as noise.
        let samples = [1.0, 3.0, 2.0, 4.0, 0.0, 6.0];
        assert_eq!(resample(&samples, 1, 3), [2.0, 10.0 / 3.0]);
    }

    #[test]
    fn a_rate_that_needs_no_change_is_left_alone() {
        let samples = [0.1, 0.2, 0.3];
        assert_eq!(resample(&samples, 1, 1), samples);
    }

    #[test]
    fn a_trailing_group_that_is_short_is_still_kept() {
        // The last fraction of a second is often the end of the word.
        let samples = [1.0, 1.0, 1.0, 2.0];
        assert_eq!(resample(&samples, 1, 3), [1.0, 2.0]);
    }

    #[test]
    fn nothing_in_produces_nothing_out() {
        assert!(resample(&[], 2, 3).is_empty());
        assert!(resample(&[1.0], 0, 3).is_empty());
    }

    #[test]
    fn a_denied_device_is_reported_as_a_permission_rather_than_a_fault() {
        // CoreAudio reports both as an opaque number, and the operator can only
        // do something about one of them.
        let refused = classify("could not open the microphone: OSStatus: 560947818");
        assert_eq!(refused.class(), ErrorClass::Permission);
        assert!(refused.to_string().contains("System Settings"));

        let broken = classify("could not start the microphone: device disappeared");
        assert_eq!(broken.class(), ErrorClass::ComponentFailure);
    }

    #[test]
    fn no_microphone_at_all_is_a_fault_rather_than_a_refusal() {
        // Telling someone to check System Settings when there is no microphone
        // sends them somewhere with nothing to change.
        assert_eq!(
            classify("no microphone is attached").class(),
            ErrorClass::ComponentFailure
        );
    }
}
