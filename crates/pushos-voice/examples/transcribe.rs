//! Transcribes a recording, so an engine can be checked without a Push 2.
//!
//! ```text
//! say -o /tmp/said.aiff "stop the build"
//! afconvert -f WAVE -d LEI16@16000 -c 1 /tmp/said.aiff /tmp/said.wav
//! cargo run -p pushos-voice --features whisper --example transcribe -- /tmp/said.wav [model-dir]
//! ```
//!
//! With no model directory it uses whichever engine macOS ships, which is what
//! PushOS uses by default.

use std::path::Path;

use pushos_domain::ports::{Recording, SAMPLE_RATE};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let audio = arguments
        .next()
        .ok_or("usage: transcribe <wav> [model-dir]")?;
    let model = arguments.next();

    let samples = read_wav(Path::new(&audio))?;
    let recording = Recording::new(samples);
    println!(
        "read {} ({:.2}s, {}silent)",
        audio,
        recording.duration.as_secs_f32(),
        if recording.is_silent() { "" } else { "not " }
    );

    let engine = match &model {
        Some(directory) => pushos_voice::transcriber(
            pushos_domain::voice::VoiceEngine::Whisper,
            Some(Path::new(directory)),
        )?,
        None => pushos_voice::transcriber(pushos_domain::voice::VoiceEngine::Apple, None)?,
    };

    let started = std::time::Instant::now();
    let said = engine.transcribe(&recording).await?;
    println!(
        "{} heard \"{}\" in {:.2}s",
        engine.name(),
        said.text,
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Reads a mono sixteen-bit WAV, which is what `afconvert` produces.
///
/// Deliberately minimal: this is a development tool, and a WAV reader with
/// opinions would be a dependency the runtime does not need.
fn read_wav(path: &Path) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("that is not a WAV file".into());
    }

    let mut cursor = 12;
    let mut rate = 0_u32;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into()?) as usize;
        let body = cursor + 8;

        if id == b"fmt " && body + 12 <= bytes.len() {
            rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into()?);
        } else if id == b"data" {
            if rate != SAMPLE_RATE {
                return Err(
                    format!("the file is {rate} Hz; PushOS works in {SAMPLE_RATE} Hz").into(),
                );
            }
            let end = (body + size).min(bytes.len());
            return Ok(bytes[body..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32768.0)
                .collect());
        }

        cursor = body + size + (size % 2);
    }

    Err("the file has no audio in it".into())
}
