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

/// The most of one recording kept, in seconds.
///
/// More than either engine reads: Whisper looks at thirty seconds and Apple's
/// recogniser at about a minute. A control held for longer, or a release that
/// never arrived because the Push was unplugged mid-phrase, would otherwise
/// keep every sample the device produces, tens of megabytes a minute, for as
/// long as nothing lets go.
const LONGEST_SECONDS: usize = 60;

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
        // Before the device is touched. `CoreAudio` will not prompt, and an
        // unasked microphone fails exactly like a refused one, so asking has
        // to happen here or the operator is never given the chance to say yes.
        crate::apple::ask_to_record()?;

        match self.ask(Command::Start) {
            Some(Ok(())) => Ok(()),
            Some(Err(why)) => {
                // The raw text, before it is sorted into a refusal or a fault:
                // `CoreAudio` reports both as an opaque number, and an
                // operator who has already granted the microphone needs to
                // know which number they are actually looking at.
                warn!(%why, "the microphone would not open");
                Err(classify(&why))
            }
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

/// Whether an input device exists that can actually be opened.
///
/// Two questions again, and the second is the one that was missing: macOS
/// offers a default input device on a Mac with no microphone in it — a virtual
/// one left behind by a meeting application, say — and it is only opening the
/// thing that tells you it will not work. A report that stops at "a device is
/// listed" says everything is fine right up until a control is held.
pub fn input_device_works() -> Result<String, VoiceError> {
    let device =
        cpal::default_host()
            .default_input_device()
            .ok_or_else(|| VoiceError::Unavailable {
                context: "no microphone is attached to this Mac".to_owned(),
            })?;

    // cpal names a device through `Display` rather than a method, and that
    // implementation can fail — `to_string` turns the failure into a panic,
    // and a device too broken to name is exactly the case this is here to
    // report on.
    let named = {
        use std::fmt::Write as _;
        let mut written = String::new();
        if write!(written, "{device}").is_err() {
            "a device that will not say its name".clone_into(&mut written);
        }
        written
    };

    match device.default_input_config() {
        Ok(_) => Ok(named),
        Err(error) => Err(classify(&format!("could not open the microphone: {error}"))),
    }
}

/// One open input stream and what it has heard.
struct Session {
    stream: cpal::Stream,
    heard: Arc<Mutex<Vec<f32>>>,
    /// What the device is actually running at.
    rate: u32,
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

        let most = (rate as usize)
            .saturating_mul(channels)
            .saturating_mul(LONGEST_SECONDS);
        let heard = Arc::new(Mutex::new(Vec::<f32>::new()));
        let writing = Arc::clone(&heard);

        let stream = device
            .build_input_stream(
                config.config(),
                move |samples: &[f32], _| {
                    if let Ok(mut kept) = writing.lock() {
                        keep(&mut kept, samples, most);
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
            rate,
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

        resample(&raw, self.channels, self.rate)
    }
}

/// Adds what the device just heard, up to `most` samples in all.
///
/// The start is kept and the rest dropped, the way a transcriber that reads
/// only so far would drop it.
fn keep(kept: &mut Vec<f32>, samples: &[f32], most: usize) {
    let room = most.saturating_sub(kept.len());
    kept.extend_from_slice(&samples[..samples.len().min(room)]);
}

/// Mixes to mono and brings the rate to [`SAMPLE_RATE`].
///
/// By ratio, not by taking every nth sample. Dividing the rates as whole
/// numbers is only right when one is a multiple of the other, and the rates
/// microphones actually run at are mostly not: 44100 over 16000 rounds down to
/// two, which hands the recogniser audio a third too fast, and a headset at
/// 24000 rounds down to one, which makes it half again too fast. Neither
/// reports an error. The transcript is simply wrong, which is the worst way for
/// this to fail.
///
/// Each output sample is the average of the input window it covers, which is
/// both a decimator and a crude low pass — dropping samples instead would fold
/// everything above the new limit back down as noise, and noise is what a
/// speech model is worst at.
fn resample(raw: &[f32], channels: usize, rate: u32) -> Vec<f32> {
    if raw.is_empty() || channels == 0 || rate == 0 {
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

    if rate == SAMPLE_RATE {
        return mono;
    }

    // How many samples the same span of time is at the rate everything else
    // works in. Worked out in `u128` because a minute of 96 kHz audio times a
    // sample rate overflows a `u64` on nothing like a large machine.
    let ours = u128::from(SAMPLE_RATE);
    let theirs = u128::from(rate);
    let wanted = usize::try_from(frames as u128 * ours / theirs).unwrap_or(usize::MAX);
    let mut out = Vec::with_capacity(wanted);

    for at in 0..wanted {
        let from = usize::try_from(at as u128 * theirs / ours).unwrap_or(frames);
        let to = usize::try_from((at as u128 + 1) * theirs / ours)
            .unwrap_or(frames)
            .max(from + 1)
            .min(frames);
        let window = &mono[from..to];
        let count = u16::try_from(window.len()).unwrap_or(u16::MAX).max(1);
        out.push(window.iter().sum::<f32>() / f32::from(count));
    }

    out
}

/// Tells a refused microphone apart from an unusable one.
///
/// `CoreAudio` reports both as an opaque number, and the operator can only fix
/// one of them — but they are fixed in two different places, so guessing sends
/// them to the wrong pane of System Settings and everything there looks right.
///
/// `560947818` is `'!obj'`, `kAudioHardwareBadObjectError`, and it was written
/// down here as what a refused device reports. It is not. It is what an input
/// device that cannot be opened reports, which on a Mac with no microphone in
/// it is the device macOS offers anyway: a Mac mini has no built-in
/// microphone, and a virtual one left behind by a meeting application will be
/// picked as the default input and then refuse to open. That mistake sent an
/// operator to Privacy and Security, where the microphone was already allowed.
fn classify(why: &str) -> VoiceError {
    let refused = why.contains("561017449") // '!pri', not permitted
        || why.to_lowercase().contains("permission")
        || why.to_lowercase().contains("not authorized");

    if refused {
        return VoiceError::NotPermitted {
            what: "the microphone".to_owned(),
        };
    }

    if why.contains("560947818") {
        return VoiceError::Unavailable {
            context: concat!(
                "the input device macOS offers cannot be opened. Pick a working one in ",
                "System Settings under Sound, Input; a Mac with no microphone of its own ",
                "offers a virtual device that does not record"
            )
            .to_owned(),
        };
    }

    VoiceError::backend(
        why.to_owned(),
        ErrorClass::ComponentFailure,
        std::io::Error::other(why.to_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_stops_growing_at_its_longest_and_keeps_its_start() {
        let mut kept = Vec::new();
        keep(&mut kept, &[1.0, 2.0, 3.0], 5);
        keep(&mut kept, &[4.0, 5.0, 6.0], 5);
        keep(&mut kept, &[7.0], 5);
        assert_eq!(kept, [1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn two_channels_become_one_by_averaging_them() {
        // Picking one channel loses whatever was said into the other.
        let stereo = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(resample(&stereo, 2, SAMPLE_RATE), [0.5, 0.5, 0.0]);
    }

    #[test]
    fn bringing_the_rate_down_averages_rather_than_dropping_samples() {
        // Three device samples to one of ours: 48 kHz, the common case.
        // Dropping folds everything above the new limit back down as noise.
        let samples = [1.0, 3.0, 2.0, 4.0, 0.0, 6.0];
        assert_eq!(resample(&samples, 1, SAMPLE_RATE * 3), [2.0, 10.0 / 3.0]);
    }

    #[test]
    fn a_rate_that_needs_no_change_is_left_alone() {
        let samples = [0.1, 0.2, 0.3];
        assert_eq!(resample(&samples, 1, SAMPLE_RATE), samples);
    }

    #[test]
    fn a_tail_too_short_to_be_a_whole_sample_is_dropped() {
        // The last fraction of a second is often the end of the word.
        let samples = [1.0, 1.0, 1.0, 2.0];
        // Four device samples at three to one is one of ours and a third.
        // The third is dropped rather than promoted to a whole sample: a
        // recording's length is worked out from how many samples it has, so
        // rounding up would say it lasted longer than it did. A third of a
        // sample at 16 kHz is twenty microseconds.
        assert_eq!(resample(&samples, 1, SAMPLE_RATE * 3), [1.0]);
    }

    #[test]
    fn nothing_in_produces_nothing_out() {
        assert!(resample(&[], 2, 48_000).is_empty());
        assert!(resample(&[1.0], 0, 48_000).is_empty());
        assert!(resample(&[1.0], 1, 0).is_empty());
    }

    /// How long, in samples of our own, a span of device audio comes back as.
    fn lengths_for(rate: u32, seconds: usize) -> usize {
        let raw = vec![0.5f32; rate as usize * seconds];
        resample(&raw, 1, rate).len()
    }

    #[test]
    fn every_rate_a_microphone_runs_at_comes_back_the_right_length() {
        // The bug this replaced divided the rates as whole numbers, so only a
        // multiple of 16 kHz came out right. Everything else was handed to the
        // recogniser too fast, with no error anywhere: 44.1 kHz a third too
        // fast, a Bluetooth headset at 24 kHz half again too fast. A wrong
        // transcript is the worst way for this to fail, because nothing looks
        // broken.
        for rate in [
            8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 96_000,
        ] {
            let got = lengths_for(rate, 2);
            let wanted = SAMPLE_RATE as usize * 2;
            let out_by = got.abs_diff(wanted);
            assert!(
                out_by <= 1,
                "{rate} Hz came back as {got} samples where {wanted} was wanted, \
                 which is speech {}% off the right speed",
                out_by * 100 / wanted.max(1)
            );
        }
    }

    #[test]
    fn a_rate_below_ours_is_stretched_rather_than_silently_mislabelled() {
        // A headset at 8 kHz used to come back unchanged and be treated as
        // 16 kHz, which is half speed.
        let raw = vec![1.0f32; 8_000];
        let out = resample(&raw, 1, 8_000);
        assert_eq!(out.len(), 16_000, "half a second at 8 kHz is half a second");
        assert!(out.iter().all(|sample| (sample - 1.0).abs() < f32::EPSILON));
    }

    #[test]
    fn a_ramp_keeps_its_shape_through_the_rate_change() {
        // Not just the right length: the audio has to still be the audio.
        let raw: Vec<f32> = (0..48_000u16).map(|i| f32::from(i) / 48_000.0).collect();
        let out = resample(&raw, 1, 48_000);
        assert_eq!(out.len(), 16_000);
        assert!(out[0] < 0.01, "it should still start low");
        assert!(out[out.len() - 1] > 0.99, "and still end high");
        for pair in out.windows(2) {
            assert!(pair[1] >= pair[0], "a ramp should not go backwards");
        }
    }

    #[test]
    fn a_denied_device_is_reported_as_a_permission_rather_than_a_fault() {
        // CoreAudio reports both as an opaque number, and the operator can only
        // do something about one of them.
        let refused = classify("could not open the microphone: OSStatus: 561017449");
        assert_eq!(refused.class(), ErrorClass::Permission);
        assert!(refused.to_string().contains("System Settings"));

        let broken = classify("could not start the microphone: device disappeared");
        assert_eq!(broken.class(), ErrorClass::ComponentFailure);
    }

    #[test]
    fn a_device_that_will_not_open_sends_the_operator_to_sound_not_to_privacy() {
        // This test previously asserted the opposite, and the mistake cost an
        // operator an hour. `'!obj'` is `kAudioHardwareBadObjectError`: the
        // device cannot be opened, which is not the same as not being allowed
        // to open it. On a Mac mini — no microphone of its own — macOS offers
        // whatever virtual device a meeting application left behind, and that
        // is what this is.
        let unusable = classify("could not open the microphone: OSStatus: 560947818");

        assert_ne!(
            unusable.class(),
            ErrorClass::Permission,
            "the microphone was already allowed; sending them to Privacy finds nothing to change"
        );
        let said = unusable.to_string();
        assert!(said.contains("Sound"), "{said}");
        assert!(said.contains("Input"), "{said}");
    }

    #[test]
    fn the_two_opaque_numbers_are_not_confused_for_one_another() {
        let not_permitted = classify("OSStatus: 561017449");
        let will_not_open = classify("OSStatus: 560947818");
        assert_eq!(not_permitted.class(), ErrorClass::Permission);
        assert_ne!(will_not_open.class(), ErrorClass::Permission);
        assert_ne!(not_permitted.to_string(), will_not_open.to_string());
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
