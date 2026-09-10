//! Choosing which engine works out what was said.
//!
//! Both run on this Mac and neither sends audio anywhere. One is already
//! installed and one is a file the operator fetched, and the difference between
//! them is entirely a matter of what they had to set up.

use std::path::Path;
use std::sync::Arc;

use pushos_domain::ports::{Transcriber, VoiceError};
use pushos_domain::voice::VoiceEngine;

/// Builds the engine the operator asked for.
///
/// Fails rather than falling back. An operator who chose an engine and got a
/// different one would have no way of telling, and the two do not hear the
/// same things.
pub fn transcriber(
    engine: VoiceEngine,
    model: Option<&Path>,
) -> Result<Arc<dyn Transcriber>, VoiceError> {
    match engine {
        VoiceEngine::Apple => apple(),
        VoiceEngine::Whisper => whisper(model),
    }
}

// The Result is the shape the other host needs, not this one: macOS always has
// an engine, and the caller must not have to know which host it is on.
#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_wraps)]
fn apple() -> Result<Arc<dyn Transcriber>, VoiceError> {
    Ok(Arc::new(crate::apple::AppleSpeech::new()))
}

#[cfg(not(target_os = "macos"))]
fn apple() -> Result<Arc<dyn Transcriber>, VoiceError> {
    Err(VoiceError::unavailable(
        "the `apple` speech engine needs macOS; set `engine = \"whisper\"` instead",
    ))
}

#[cfg(feature = "whisper")]
fn whisper(model: Option<&Path>) -> Result<Arc<dyn Transcriber>, VoiceError> {
    let model = model.ok_or_else(|| {
        VoiceError::unavailable("the `whisper` speech engine needs `model` set to a model file")
    })?;
    Ok(Arc::new(crate::whisper::WhisperSpeech::open(model)?))
}

#[cfg(not(feature = "whisper"))]
fn whisper(_model: Option<&Path>) -> Result<Arc<dyn Transcriber>, VoiceError> {
    Err(VoiceError::unavailable(
        "this build of PushOS has no Whisper in it; build with `--features whisper`, \
         or set `engine = \"apple\"` to use the one macOS already has",
    ))
}

/// Builds the microphone for this host.
pub fn microphone() -> Result<Arc<dyn pushos_domain::ports::Microphone>, VoiceError> {
    #[cfg(target_os = "macos")]
    {
        Ok(Arc::new(crate::recorder::CoreAudioMicrophone::new()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(VoiceError::unavailable("PushOS can only listen on macOS"))
    }
}

/// Whether macOS will let this process hear anything.
///
/// Asked of the system rather than inferred from finding a device. Those are
/// different questions and the difference is the one that bites: a machine with
/// a microphone in it reports a microphone, and the process still cannot open
/// it. Which process is being asked about matters too. macOS grants this to
/// whatever was responsible for starting PushOS, so one started from a terminal
/// window is granted alongside that terminal, and one started from somewhere
/// the operator cannot see is a permission they can never give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordPermission {
    /// Nobody has been asked yet. macOS will ask the first time it is needed.
    Unasked,
    /// This process may record.
    Granted,
    /// It may not, and asking again will not help.
    Denied,
    /// The system did not say, which is not a state macOS documents.
    Unknown,
}

impl RecordPermission {
    /// What the system says about this process right now.
    ///
    /// Asked in `apple`, which is the one module allowed to speak
    /// Objective-C, because there is no other way to ask macOS this.
    #[cfg(target_os = "macos")]
    pub fn current() -> Self {
        crate::apple::record_permission()
    }

    /// What the system says about this process right now.
    #[cfg(not(target_os = "macos"))]
    pub const fn current() -> Self {
        Self::Denied
    }

    /// What to tell an operator about it.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Granted => "allowed",
            Self::Unasked => {
                "not asked for yet; macOS asks the first time a voice control is held, \
                 and asks whichever application started PushOS"
            }
            Self::Denied => {
                "refused; allow the application that starts PushOS under System Settings, \
                 Privacy and Security, Microphone"
            }
            Self::Unknown => "unknown; macOS did not say",
        }
    }

    /// Whether listening would work.
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Granted | Self::Unasked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn the_default_engine_is_available_without_the_operator_setting_anything_up() {
        let built = transcriber(VoiceEngine::Apple, None).expect("macOS has one built in");
        assert_eq!(built.name(), "macOS");
    }

    #[test]
    #[cfg(not(feature = "whisper"))]
    fn asking_for_an_engine_this_build_does_not_have_says_how_to_get_it() {
        // Falling back to the other engine would be worse: the operator chose
        // one, and could not tell they had not got it.
        let error = transcriber(VoiceEngine::Whisper, None).expect_err("not built in");
        assert!(error.to_string().contains("--features whisper"));
    }
}
