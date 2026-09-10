//! Push to talk.
//!
//! A control is held, which starts listening, and released, which stops it.
//! There is no wake word: the operator's finger decides when PushOS is
//! listening, and they can always see that it is down.
//!
//! Everything happens on this Mac. Nothing here sends audio anywhere, and an
//! implementation that did would be the wrong implementation.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]
#![doc(html_no_source)]

mod engines;
mod listener;
#[cfg(feature = "whisper")]
mod mel;
mod router;
#[cfg(feature = "whisper")]
mod whisper;

/// Speaking to macOS's own frameworks, which means speaking Objective-C.
///
/// The only place in PushOS where `unsafe` is allowed. Everything it touches is
/// a system object made and dropped inside one function, and the module hands
/// the rest of PushOS nothing but a `String` and an error. Widening this
/// exception would need a reason as good as "there is no other way to ask macOS
/// what was said".
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod apple;
#[cfg(target_os = "macos")]
mod recorder;

pub use engines::{RecordPermission, microphone, transcriber};
pub use listener::{Heard, Pending, VoiceListener};
pub use router::VoiceRouter;
#[cfg(feature = "whisper")]
pub use whisper::WhisperSpeech;

#[cfg(target_os = "macos")]
pub use apple::{AppleSpeech, Readiness};
#[cfg(target_os = "macos")]
pub use recorder::CoreAudioMicrophone;
