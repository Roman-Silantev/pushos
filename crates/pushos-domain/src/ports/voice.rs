//! The voice ports.
//!
//! Two things, kept apart on purpose. One captures sound while a control is
//! held; the other turns sound into words. They fail for entirely different
//! reasons, and an operator told "the microphone is not permitted" needs to do
//! something different from one told "no speech model is installed".

use std::time::Duration;

use async_trait::async_trait;

use crate::voice::Utterance;

/// Sound, as PushOS captures it.
///
/// Mono, sixteen thousand samples a second, one float per sample between minus
/// one and one. Every engine wants this and none of them agree on anything
/// else, so the conversion happens once, at the microphone.
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// The samples.
    pub samples: Vec<f32>,
    /// How long it lasted.
    pub duration: Duration,
}

/// How many samples a second everything here works in.
pub const SAMPLE_RATE: u32 = 16_000;

impl Recording {
    /// Builds a recording from samples already at [`SAMPLE_RATE`].
    pub fn new(samples: Vec<f32>) -> Self {
        // Whole numbers throughout: the length follows from the count exactly,
        // and a duration that drifted would make a held control feel wrong.
        let micros = samples.len() as u64 * 1_000_000 / u64::from(SAMPLE_RATE);
        let duration = Duration::from_micros(micros);
        Self { samples, duration }
    }

    /// Whether there is anything in it at all.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Whether it is nothing but room noise.
    ///
    /// A control brushed rather than held produces a fraction of a second of
    /// almost nothing, and sending that to a transcriber wastes time to arrive
    /// at an empty string.
    pub fn is_silent(&self) -> bool {
        const FLOOR: f32 = 0.005;

        if self.samples.is_empty() {
            return true;
        }

        // Accumulated in step rather than divided at the end, so the sample
        // count never has to become a float at all.
        let square = f64::from(FLOOR) * f64::from(FLOOR);
        let (energy, floor) =
            self.samples
                .iter()
                .fold((0.0_f64, 0.0_f64), |(energy, floor), sample| {
                    (
                        energy + f64::from(*sample) * f64::from(*sample),
                        floor + square,
                    )
                });
        energy < floor
    }
}

/// Captures what the operator says while a control is held.
#[async_trait]
pub trait Microphone: Send + Sync + std::fmt::Debug {
    /// Begins keeping audio.
    ///
    /// Called on the press. Calling it again while already recording restarts
    /// rather than failing: two presses is an operator changing their mind.
    async fn start(&self) -> Result<(), VoiceError>;

    /// Stops, and hands back what was said.
    ///
    /// Called on the release. Returns nothing when it was not recording, which
    /// is what a release with no press before it means.
    async fn stop(&self) -> Result<Option<Recording>, VoiceError>;

    /// Stops and throws away what was said.
    async fn discard(&self);

    /// Whether audio is being kept right now.
    async fn is_recording(&self) -> bool;
}

/// Turns sound into words.
#[async_trait]
pub trait Transcriber: Send + Sync + std::fmt::Debug {
    /// What the engine is called, for the display and the log.
    fn name(&self) -> &str;

    /// Works out what was said.
    ///
    /// Runs on the device. Nothing here sends audio anywhere, and an
    /// implementation that did would be the wrong implementation.
    async fn transcribe(&self, recording: &Recording) -> Result<Utterance, VoiceError>;
}

/// Why listening or transcribing failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VoiceError {
    /// The operator has not allowed it.
    ///
    /// Separate from every other failure because it is the only one the
    /// operator can do something about, and the something is specific.
    #[error("{what} is not permitted; allow it in System Settings under Privacy and Security")]
    NotPermitted {
        /// What was refused: the microphone, or speech recognition.
        what: String,
    },

    /// No engine is available to do the work.
    #[error("{context}")]
    Unavailable {
        /// What is missing, and what would fix it.
        context: String,
    },

    /// The engine reported a fault.
    #[error("{context}")]
    Backend {
        /// What PushOS was attempting.
        context: String,
        /// How a caller should react.
        class: crate::error::ErrorClass,
        /// The originating fault.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl VoiceError {
    /// How a caller should react.
    pub const fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            // The only one the operator can do something about, and the
            // message already says what.
            Self::NotPermitted { .. } => ErrorClass::Permission,
            Self::Unavailable { .. } => ErrorClass::Validation,
            Self::Backend { class, .. } => *class,
        }
    }

    /// Reports that the operator has not allowed something.
    pub fn not_permitted(what: impl Into<String>) -> Self {
        Self::NotPermitted { what: what.into() }
    }

    /// Reports that there is nothing available to do the work.
    pub fn unavailable(context: impl Into<String>) -> Self {
        Self::Unavailable {
            context: context.into(),
        }
    }

    /// Wraps an engine fault with the context that makes it actionable.
    pub fn backend(
        context: impl Into<String>,
        class: crate::error::ErrorClass,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Backend {
            context: context.into(),
            class,
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::*;

    fn tone(samples: u32, amplitude: f32) -> Recording {
        Recording::new(
            (0..samples)
                .map(|index| amplitude * f32::sin(index as f32 * 0.1))
                .collect(),
        )
    }

    #[test]
    fn a_recording_knows_how_long_it_is_from_how_many_samples_it_has() {
        let one_second = Recording::new(vec![0.0; SAMPLE_RATE as usize]);
        assert_eq!(one_second.duration, Duration::from_secs(1));
    }

    #[test]
    fn a_brushed_control_produces_something_that_is_recognised_as_nothing() {
        // Sending this to a transcriber wastes a second to arrive at "".
        assert!(Recording::new(Vec::new()).is_silent());
        assert!(tone(8_000, 0.0005).is_silent());
    }

    #[test]
    fn speech_is_not_mistaken_for_silence() {
        assert!(!tone(8_000, 0.2).is_silent());
    }

    #[test]
    fn an_empty_recording_says_so() {
        assert!(Recording::new(Vec::new()).is_empty());
        assert!(!tone(10, 0.5).is_empty());
    }

    #[test]
    fn a_refused_microphone_is_told_apart_from_a_broken_one() {
        // The operator can fix one of these and not the other, and the message
        // has to say which.
        let refused = VoiceError::NotPermitted {
            what: "the microphone".to_owned(),
        };
        assert_eq!(refused.class(), crate::error::ErrorClass::Permission);
        assert!(refused.to_string().contains("System Settings"));

        let broken = VoiceError::backend(
            "the engine stopped",
            crate::error::ErrorClass::ComponentFailure,
            std::io::Error::other("gone"),
        );
        assert_eq!(broken.class(), crate::error::ErrorClass::ComponentFailure);
    }
}
