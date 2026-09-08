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
