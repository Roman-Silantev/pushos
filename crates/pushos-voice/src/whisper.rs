//! Whisper, run on this Mac.
//!
//! The alternative engine, for an operator who would rather choose their own
//! model than use the one macOS ships. It costs them a download and the disk to
//! keep it on, and it buys them a model they picked.
//!
//! Nothing here reaches the network. The model is a directory the operator
//! already has, and the audio never leaves the process.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{self as whisper, Config, audio, model::Whisper};
use pushos_domain::error::ErrorClass;
use pushos_domain::ports::{Recording, Transcriber, VoiceError};
use pushos_domain::voice::Utterance;
use tokenizers::Tokenizer;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// The three files a Whisper model is made of.
const WEIGHTS: &str = "model.safetensors";
const TOKENIZER: &str = "tokenizer.json";
const SETTINGS: &str = "config.json";

/// How many words one held phrase may produce.
///
/// Push to talk is a sentence, not a dictation session. A model that runs past
/// this is repeating itself, which is the failure Whisper has when it is unsure.
const MOST_TOKENS: usize = 224;

/// Whisper, loaded and ready.
#[derive(Debug)]
pub struct WhisperSpeech {
    /// Held behind a lock because one model cannot decode two phrases at once,
    /// and push to talk never asks it to.
    inner: Arc<Mutex<Loaded>>,
    name: String,
}

/// Everything decoding one phrase needs.
struct Loaded {
    model: Whisper,
    tokenizer: Tokenizer,
    device: Device,
    /// The filterbank, built once rather than per phrase.
    filters: Vec<f32>,
    /// The tokens that start a transcript, and the one that ends it.
    opening: Vec<u32>,
    ending: u32,
    /// Tokens the model must never emit, from the model's own settings.
    suppressed: Vec<u32>,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl WhisperSpeech {
    /// Loads a model from a directory the operator already has.
    ///
    /// Wants the three files a Hugging Face Whisper repository holds. Says
    /// which one is missing rather than failing on the first thing it opens.
    pub fn open(directory: &Path) -> Result<Self, VoiceError> {
        let files = ModelFiles::in_directory(directory)?;

        let config: Config = serde_json::from_slice(&read(&files.settings)?)
            .map_err(|error| broken(&files.settings, &error))?;
        let tokenizer = Tokenizer::from_file(&files.tokenizer)
            .map_err(|error| broken(&files.tokenizer, &*error))?;

        // Metal where there is one, which on this hardware there is. The
        // fallback is slow rather than absent, and saying so beats refusing.
        let device = Device::new_metal(0).unwrap_or_else(|error| {
            warn!(%error, "no Metal device; Whisper will run on the processor and be slower");
            Device::Cpu
        });

        // Read rather than mapped. Mapping is the faster way and needs
        // `unsafe`, which in PushOS is reserved for asking macOS what was said.
        // A model is loaded once, at startup, so the difference is a second.
        let weights =
            VarBuilder::from_buffered_safetensors(read(&files.weights)?, whisper::DTYPE, &device)
                .map_err(|error| broken(&files.weights, &error))?;
        let model = Whisper::load(&weights, config.clone())
            .map_err(|error| broken(&files.weights, &error))?;

        let token = |name: &str| {
            tokenizer.token_to_id(name).ok_or_else(|| {
                VoiceError::unavailable(format!(
                    "the tokenizer in {} has no `{name}`, so it is not a Whisper tokenizer",
                    directory.display()
                ))
            })
        };

        let filters = crate::mel::filterbank(
            whisper::SAMPLE_RATE
                .try_into()
                .unwrap_or(pushos_domain::ports::SAMPLE_RATE),
            whisper::N_FFT,
            config.num_mel_bins,
        );

        let name = directory.file_name().map_or_else(
            || "whisper".to_owned(),
            |name| name.to_string_lossy().into(),
        );
        info!(model = %name, device = ?device, "Whisper loaded");

        Ok(Self {
            inner: Arc::new(Mutex::new(Loaded {
                model,
                // Timestamps would be words the operator did not say.
                opening: vec![
                    token(whisper::SOT_TOKEN)?,
                    token(whisper::TRANSCRIBE_TOKEN)?,
                    token(whisper::NO_TIMESTAMPS_TOKEN)?,
                ],
                ending: token(whisper::EOT_TOKEN)?,
                suppressed: config.suppress_tokens.clone(),
                tokenizer,
                device,
                filters,
            })),
            name,
        })
    }
}

#[async_trait]
impl Transcriber for WhisperSpeech {
    fn name(&self) -> &str {
        &self.name
    }

    async fn transcribe(&self, recording: &Recording) -> Result<Utterance, VoiceError> {
        let samples = recording.samples.clone();
        let held = recording.duration;
        let inner = Arc::clone(&self.inner);

        // Inference is seconds of arithmetic. Doing it on the runtime would
        // stop the surface answering, which is the one thing it must keep
        // doing while PushOS thinks.
        let text = tokio::task::spawn_blocking(move || {
            let mut loaded = inner.blocking_lock();
            loaded.decode(&samples)
        })
        .await
        .map_err(|error| VoiceError::backend("Whisper stopped", ErrorClass::Retryable, error))??;

        debug!(%text, "Whisper finished");
        Ok(Utterance::new(text, held))
    }
}

impl Loaded {
    /// Works out what one held phrase said.
    fn decode(&mut self, samples: &[f32]) -> Result<String, VoiceError> {
        let mel = self.spectrogram(samples)?;

        self.model.reset_kv_cache();
        let audio = self
            .model
            .encoder
            .forward(&mel, true)
            .map_err(|error| failed("could not read the audio", error))?;

        let mut tokens = self.opening.clone();
        for step in 0..MOST_TOKENS {
            // The whole sequence, every step. Only the encoder's contribution
            // is cached here; the decoder's own attention is recomputed, and
            // handing it one token would lose both the history and the
            // position that go with it.
            let input = Tensor::new(tokens.as_slice(), &self.device)
                .and_then(|tensor| tensor.unsqueeze(0))
                .map_err(|error| failed("could not build the next step", error))?;

            let hidden = self
                .model
                .decoder
                .forward(&input, &audio, step == 0)
                .map_err(|error| failed("could not work out the next word", error))?;

            let last = hidden
                .dim(1)
                .and_then(|length| hidden.narrow(1, length - 1, 1))
                .and_then(|last| self.model.decoder.final_linear(&last))
                .and_then(|logits| logits.i(0))
                .and_then(|logits| logits.i(0))
                .map_err(|error| failed("could not work out the next word", error))?;

            let next = self.most_likely(&last)?;
            if next == self.ending {
                break;
            }
            tokens.push(next);
        }

        let said = &tokens[self.opening.len()..];
        self.tokenizer
            .decode(said, true)
            .map(|text| text.trim().to_owned())
            .map_err(|error| failed("could not turn the result into words", &*error))
    }

    /// Turns the samples into what the encoder reads.
    ///
    /// Whisper only ever looks at thirty seconds. A held phrase is shorter than
    /// that, so the rest is silence; longer, and the tail is what gets dropped,
    /// which is the same thing the operator would expect from letting go.
    fn spectrogram(&self, samples: &[f32]) -> Result<Tensor, VoiceError> {
        let mut padded = samples.to_vec();
        padded.truncate(whisper::N_SAMPLES);
        padded.resize(whisper::N_SAMPLES, 0.0);

        let bands = self.filters.len() / (1 + whisper::N_FFT / 2);
        let config = Config {
            num_mel_bins: bands,
            max_source_positions: 0,
            d_model: 0,
            encoder_attention_heads: 0,
            encoder_layers: 0,
            vocab_size: 0,
            max_target_positions: 0,
            decoder_attention_heads: 0,
            decoder_layers: 0,
            suppress_tokens: Vec::new(),
        };
        let mel = audio::pcm_to_mel(&config, &padded, &self.filters);
        let frames = mel.len() / bands;

        // The spectrogram is padded past the thirty seconds it was given, and
        // the encoder is built for exactly thirty. Taking the front is what the
        // padding was there to make safe.
        Tensor::from_vec(mel, (1, bands, frames), &self.device)
            .and_then(|mel| mel.narrow(2, 0, whisper::N_FRAMES.min(frames)))
            .map_err(|error| failed("could not prepare the audio", error))
    }

    /// The most likely next token, with the model's own suppressions honoured.
    fn most_likely(&self, logits: &Tensor) -> Result<u32, VoiceError> {
        let mut scores: Vec<f32> = logits
            .to_vec1()
            .map_err(|error| failed("could not read the model's answer", error))?;

        for token in &self.suppressed {
            if let Some(score) = scores.get_mut(*token as usize) {
                *score = f32::NEG_INFINITY;
            }
        }

        // Greedy rather than sampled. An operator saying "stop" wants the same
        // word back every time, and a temperature would occasionally disagree.
        let best = scores
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(token, _)| token)
            .ok_or_else(|| VoiceError::unavailable("the model answered with nothing"))?;

        u32::try_from(best)
            .map_err(|error| failed("the model answered with an impossible token", error))
    }
}

/// Where the three parts of a model are.
struct ModelFiles {
    weights: PathBuf,
    tokenizer: PathBuf,
    settings: PathBuf,
}

impl ModelFiles {
    /// Finds them, saying which one is missing rather than which one it tried.
    fn in_directory(directory: &Path) -> Result<Self, VoiceError> {
        if !directory.is_dir() {
            return Err(VoiceError::unavailable(format!(
                "`{}` is not a directory; point `model` at a Whisper model directory \
                 holding {WEIGHTS}, {TOKENIZER} and {SETTINGS}",
                directory.display()
            )));
        }

        let files = Self {
            weights: directory.join(WEIGHTS),
            tokenizer: directory.join(TOKENIZER),
            settings: directory.join(SETTINGS),
        };

        let missing: Vec<&str> = [
            (&files.weights, WEIGHTS),
            (&files.tokenizer, TOKENIZER),
            (&files.settings, SETTINGS),
        ]
        .into_iter()
        .filter_map(|(path, name)| (!path.is_file()).then_some(name))
        .collect();

        if missing.is_empty() {
            Ok(files)
        } else {
            Err(VoiceError::unavailable(format!(
                "the model directory `{}` is missing {}",
                directory.display(),
                missing.join(" and ")
            )))
        }
    }
}

fn read(path: &Path) -> Result<Vec<u8>, VoiceError> {
    std::fs::read(path).map_err(|error| broken(path, &error))
}

/// Reports a model file that is there but wrong.
fn broken(path: &Path, error: &dyn std::fmt::Display) -> VoiceError {
    VoiceError::unavailable(format!("`{}` could not be read: {error}", path.display()))
}

/// Reports the model failing partway through a phrase.
fn failed(context: &'static str, error: impl std::fmt::Display) -> VoiceError {
    VoiceError::backend(
        format!("{context}: {error}"),
        ErrorClass::Retryable,
        std::io::Error::other(error.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_is_not_a_directory_says_what_it_should_have_been() {
        let error =
            WhisperSpeech::open(Path::new("/nowhere/at/all")).expect_err("there is no model there");
        let message = error.to_string();
        assert!(message.contains("model.safetensors"));
        assert!(message.contains("directory"));
    }

    #[test]
    fn a_directory_missing_files_names_every_one_of_them() {
        let directory = std::env::temp_dir().join(format!(
            "pushos-whisper-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("the temporary directory is writable");
        std::fs::write(directory.join(SETTINGS), "{}").expect("writable");

        let error = WhisperSpeech::open(&directory).expect_err("two files are missing");
        let message = error.to_string();
        assert!(message.contains(WEIGHTS), "{message}");
        assert!(message.contains(TOKENIZER), "{message}");
        assert!(
            !message.contains(SETTINGS),
            "the one that is there must not be listed: {message}"
        );

        std::fs::remove_dir_all(&directory).ok();
    }
}
