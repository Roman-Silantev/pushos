//! What macOS itself makes of the audio.
//!
//! The Speech framework, asked to keep the audio on this machine. It needs
//! nothing downloaded and nothing built, so voice works the moment PushOS is
//! updated, on a Mac that has never been online since.
//!
//! Apple's own documentation says on-device recognition is less accurate than
//! their servers. That is the trade this makes, deliberately: a control that
//! sends a recording of the operator's office to anybody is not a control they
//! would want on their desk.

use std::sync::mpsc;
use std::time::Duration;

use async_trait::async_trait;
use block2::RcBlock;
use objc2::AnyThread as _;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioCommonFormat, AVAudioFormat,
    AVAudioPCMBuffer,
};
use objc2_foundation::{NSError, NSOperationQueue};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognizer,
    SFSpeechRecognizerAuthorizationStatus,
};
use pushos_domain::error::ErrorClass;
use pushos_domain::ports::{Recording, SAMPLE_RATE, Transcriber, VoiceError};
use pushos_domain::voice::Utterance;
use tracing::{debug, warn};

/// How long to wait for the system to work out a held phrase.
///
/// Push to talk is seconds of speech, not a recording of a meeting. Waiting
/// longer than this means something is wrong rather than slow.
const PATIENCE: Duration = Duration::from_secs(30);

/// macOS's own speech recognition, kept on the device.
#[derive(Debug)]
pub struct AppleSpeech;

impl AppleSpeech {
    /// Builds the engine.
    ///
    /// Touches nothing: whether the operator has allowed speech recognition is
    /// asked when they first hold the control, not when PushOS starts.
    pub const fn new() -> Self {
        Self
    }

    /// What macOS says about speech recognition on this machine.
    ///
    /// Reported by `pushos doctor`, so the operator can find out what is wrong
    /// without holding a pad and wondering.
    pub fn readiness() -> Readiness {
        let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
        let permitted = status == SFSpeechRecognizerAuthorizationStatus::Authorized;
        let asked = status != SFSpeechRecognizerAuthorizationStatus::NotDetermined;

        let recognizer = unsafe { SFSpeechRecognizer::new() };
        let (available, on_device) = unsafe {
            (
                recognizer.isAvailable(),
                recognizer.supportsOnDeviceRecognition(),
            )
        };

        Readiness {
            permitted,
            asked,
            available,
            on_device,
        }
    }

    /// Asks the operator, if they have not been asked.
    ///
    /// Blocks until they answer, which is what holding a control to speak
    /// already implies they are willing to do.
    fn authorise() -> Result<(), VoiceError> {
        let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
        if status == SFSpeechRecognizerAuthorizationStatus::Authorized {
            return Ok(());
        }
        if status != SFSpeechRecognizerAuthorizationStatus::NotDetermined {
            return Err(VoiceError::NotPermitted {
                what: "speech recognition".to_owned(),
            });
        }

        let (answered, answer) = mpsc::channel();
        let handler = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
            let _ = answered.send(status);
        });
        unsafe { SFSpeechRecognizer::requestAuthorization(&handler) };

        match answer.recv_timeout(PATIENCE) {
            Ok(SFSpeechRecognizerAuthorizationStatus::Authorized) => Ok(()),
            Ok(_) => Err(VoiceError::NotPermitted {
                what: "speech recognition".to_owned(),
            }),
            Err(_) => Err(VoiceError::Unavailable {
                context: "macOS did not answer the request to use speech recognition".to_owned(),
            }),
        }
    }
}

impl Default for AppleSpeech {
    fn default() -> Self {
        Self::new()
    }
}

/// What macOS says about speech recognition here.
///
/// Four separate answers rather than one, because each of them means a
/// different thing for the operator to do about it.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readiness {
    /// Whether the operator has allowed it.
    pub permitted: bool,
    /// Whether they have been asked at all.
    pub asked: bool,
    /// Whether the recogniser is usable at all.
    pub available: bool,
    /// Whether it can work without sending anything anywhere.
    pub on_device: bool,
}

impl Readiness {
    /// Whether PushOS could transcribe right now.
    pub const fn is_ready(self) -> bool {
        self.permitted && self.available && self.on_device
    }

    /// What is wrong, and what would fix it.
    pub const fn explain(self) -> &'static str {
        if !self.available {
            "macOS has no speech recogniser for this language"
        } else if !self.on_device {
            "macOS cannot recognise this language without sending audio away, so PushOS will not"
        } else if !self.asked {
            "not asked yet; hold the voice control once and macOS will ask"
        } else if !self.permitted {
            "refused; allow speech recognition in System Settings under Privacy and Security"
        } else {
            "ready"
        }
    }
}

#[async_trait]
impl Transcriber for AppleSpeech {
    fn name(&self) -> &'static str {
        "macOS"
    }

    async fn transcribe(&self, recording: &Recording) -> Result<Utterance, VoiceError> {
        let samples = recording.samples.clone();
        let held = recording.duration;

        // Every Objective-C object is made and dropped inside this closure, so
        // nothing that cannot cross a thread ever does.
        tokio::task::spawn_blocking(move || work_out(&samples, held))
            .await
            .map_err(|error| {
                VoiceError::backend(
                    "the transcription thread did not finish",
                    ErrorClass::Retryable,
                    std::io::Error::other(error.to_string()),
                )
            })?
    }
}

/// Hands the audio to macOS and waits for the words.
fn work_out(samples: &[f32], held: Duration) -> Result<Utterance, VoiceError> {
    AppleSpeech::authorise()?;

    let recognizer = unsafe { SFSpeechRecognizer::new() };
    if !unsafe { recognizer.isAvailable() } {
        return Err(VoiceError::Unavailable {
            context: "macOS has no speech recogniser for this language".to_owned(),
        });
    }

    let request = unsafe { SFSpeechAudioBufferRecognitionRequest::new() };
    unsafe {
        // The whole point. Without this macOS may send the recording to Apple,
        // and a control that does that is not one to have on a desk.
        request.setRequiresOnDeviceRecognition(true);
        request.setShouldReportPartialResults(false);
    }

    let buffer = buffer_of(samples)?;
    unsafe {
        request.appendAudioPCMBuffer(&buffer);
        request.endAudio();
    }

    // The recogniser answers on the main queue unless told otherwise, and
    // PushOS never drains it: the main thread is running the runtime. Its own
    // queue is what makes the answer arrive at all.
    let queue = NSOperationQueue::new();
    unsafe { recognizer.setQueue(&queue) };

    let (finished, result) = mpsc::channel();
    let handler = RcBlock::new(
        move |result_ptr: *mut SFSpeechRecognitionResult, error: *mut NSError| {
            // Partial results are switched off, so the first thing that arrives
            // with anything in it is the answer.
            let said = unsafe { result_ptr.as_ref() }.map(|found| unsafe {
                let transcription = found.bestTranscription();
                (transcription.formattedString().to_string(), found.isFinal())
            });
            let failed =
                unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
            let _ = finished.send((said, failed));
        },
    );

    let _task = unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler) };

    loop {
        match result.recv_timeout(PATIENCE) {
            Ok((Some((text, true)), _)) => {
                debug!(%text, "macOS worked out what was said");
                return Ok(Utterance::new(text, held));
            }
            // Something arrived that is not the final answer; keep waiting.
            Ok((Some(_), None)) => {}
            Ok((_, Some(why))) => {
                warn!(%why, "macOS could not work out what was said");
                return Err(classify(&why));
            }
            Ok((None, None)) => {
                return Ok(Utterance::new(String::new(), held));
            }
            Err(_) => {
                return Err(VoiceError::backend(
                    "macOS did not answer in time",
                    ErrorClass::Retryable,
                    std::io::Error::other("timed out"),
                ));
            }
        }
    }
}

/// Wraps the samples in the shape the Speech framework wants.
fn buffer_of(samples: &[f32]) -> Result<Retained<AVAudioPCMBuffer>, VoiceError> {
    let unusable = || VoiceError::Unavailable {
        context: "macOS would not take the audio".to_owned(),
    };

    let format = unsafe {
        AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(
            AVAudioFormat::alloc(),
            AVAudioCommonFormat::PCMFormatFloat32,
            f64::from(SAMPLE_RATE),
            1,
            false,
        )
    }
    .ok_or_else(unusable)?;

    let frames = u32::try_from(samples.len()).map_err(|_| unusable())?;
    let buffer = unsafe {
        AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &format,
            frames,
        )
    }
    .ok_or_else(unusable)?;

    unsafe {
        buffer.setFrameLength(frames);
        let channels = buffer.floatChannelData();
        if channels.is_null() {
            return Err(unusable());
        }
        let first = (*channels).as_ptr();
        std::ptr::copy_nonoverlapping(samples.as_ptr(), first, samples.len());
    }

    Ok(buffer)
}

/// Tells a refusal apart from a fault, since only one is the operator's to fix.
fn classify(why: &str) -> VoiceError {
    let lowered = why.to_lowercase();
    if lowered.contains("not authorized") || lowered.contains("denied") {
        VoiceError::NotPermitted {
            what: "speech recognition".to_owned(),
        }
    } else if lowered.contains("no speech") || lowered.contains("1110") {
        // macOS reports "no speech detected" as a failure. It is not one: the
        // operator held the control and said nothing, which happens.
        VoiceError::backend(
            "nothing was said",
            ErrorClass::Validation,
            std::io::Error::other(why.to_owned()),
        )
    } else {
        VoiceError::backend(
            why.to_owned(),
            ErrorClass::ComponentFailure,
            std::io::Error::other(why.to_owned()),
        )
    }
}

/// What macOS says about this process's permission to record.
///
/// Asked of the system rather than inferred from finding a device. Those are
/// different questions, and the difference is the one that bites: a machine
/// with a microphone in it reports a microphone, and the process still cannot
/// open it.
pub(crate) fn record_permission() -> crate::engines::RecordPermission {
    use crate::engines::RecordPermission;

    // Both are reads of process-wide state, which Apple documents as safe from
    // any thread, and neither hands anything back but a tagged integer.
    let held = unsafe { AVAudioApplication::sharedInstance() };
    match unsafe { held.recordPermission() } {
        AVAudioApplicationRecordPermission::Granted => RecordPermission::Granted,
        AVAudioApplicationRecordPermission::Denied => RecordPermission::Denied,
        AVAudioApplicationRecordPermission::Undetermined => RecordPermission::Unasked,
        _ => RecordPermission::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_says_what_to_do_rather_than_what_is_wrong() {
        // An operator reading `doctor` needs the next step, not a diagnosis.
        let never_asked = Readiness {
            permitted: false,
            asked: false,
            available: true,
            on_device: true,
        };
        assert!(!never_asked.is_ready());
        assert!(never_asked.explain().contains("hold the voice control"));

        let refused = Readiness {
            asked: true,
            ..never_asked
        };
        assert!(refused.explain().contains("System Settings"));
    }

    #[test]
    fn a_language_that_would_need_the_network_is_refused_rather_than_used() {
        // A control that sends a recording of the operator's office to anybody
        // is not one to have on a desk.
        let cloud_only = Readiness {
            permitted: true,
            asked: true,
            available: true,
            on_device: false,
        };
        assert!(!cloud_only.is_ready());
        assert!(cloud_only.explain().contains("sending audio away"));
    }

    #[test]
    fn everything_in_place_reads_as_ready() {
        let ready = Readiness {
            permitted: true,
            asked: true,
            available: true,
            on_device: true,
        };
        assert!(ready.is_ready());
        assert_eq!(ready.explain(), "ready");
    }

    #[test]
    fn nothing_said_is_not_treated_as_a_broken_engine() {
        // macOS reports it as a failure; it is an operator holding a control
        // and saying nothing, which happens.
        assert_eq!(
            classify("No speech detected").class(),
            ErrorClass::Validation
        );
        assert_eq!(
            classify("Speech recognition not authorized").class(),
            ErrorClass::Permission
        );
        assert_eq!(
            classify("the service crashed").class(),
            ErrorClass::ComponentFailure
        );
    }

    #[test]
    fn what_macos_says_about_this_machine_can_be_asked_without_permission() {
        // `doctor` must work before anything has been allowed.
        let found = AppleSpeech::readiness();
        assert!(!found.explain().is_empty());
    }
}
