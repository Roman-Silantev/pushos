//! The mel filterbank Whisper expects.
//!
//! Computed rather than shipped as a binary blob, because a blob is a thing
//! nobody can read, review or fix. These are the same triangular filters
//! librosa builds for `mel(sr=16000, n_fft=400, n_mels=80)`, which is what
//! Whisper was trained against; the test pins them against values taken from
//! that reference.

/// Mel below this many hertz is linear in frequency; above it, logarithmic.
const LINEAR_BELOW_HZ: f32 = 1000.0;

/// Hertz per mel in the linear part of the scale.
const HERTZ_PER_MEL: f32 = 200.0 / 3.0;

/// Where the linear part ends, in mel.
const LINEAR_BELOW_MEL: f32 = LINEAR_BELOW_HZ / HERTZ_PER_MEL;

/// Builds the filterbank, flattened the way the spectrogram wants it.
///
/// One row per mel band, each of `1 + fft_size / 2` weights, laid out band
/// after band.
pub(crate) fn filterbank(sample_rate: u32, fft_size: usize, bands: usize) -> Vec<f32> {
    let bins = 1 + fft_size / 2;
    #[allow(clippy::cast_precision_loss)] // A sample rate and an FFT size, both small.
    let nyquist = sample_rate as f32 / 2.0;

    // The frequency each FFT bin sits at.
    let frequencies: Vec<f32> = (0..bins)
        .map(|bin| {
            #[allow(clippy::cast_precision_loss)] // Bin counts are in the hundreds.
            let position = bin as f32 / (bins - 1) as f32;
            position * nyquist
        })
        .collect();

    // The band edges, evenly spaced in mel. Two more than there are bands,
    // because each triangle needs the one before it and the one after it.
    let top = to_mel(nyquist);
    let edges: Vec<f32> = (0..bands + 2)
        .map(|index| {
            #[allow(clippy::cast_precision_loss)] // Band counts are in the tens.
            let position = index as f32 / (bands + 1) as f32;
            to_hertz(position * top)
        })
        .collect();

    let mut weights = vec![0.0_f32; bands * bins];
    for band in 0..bands {
        let (low, centre, high) = (edges[band], edges[band + 1], edges[band + 2]);
        // Slaney normalisation: each triangle is scaled to unit area, so a
        // wide band high up does not drown a narrow one low down.
        let area = 2.0 / (high - low);

        for (bin, &frequency) in frequencies.iter().enumerate() {
            let rising = (frequency - low) / (centre - low);
            let falling = (high - frequency) / (high - centre);
            weights[band * bins + bin] = area * rising.min(falling).max(0.0);
        }
    }

    weights
}

/// Hertz to mel, on the scale Whisper was trained with.
fn to_mel(hertz: f32) -> f32 {
    if hertz < LINEAR_BELOW_HZ {
        hertz / HERTZ_PER_MEL
    } else {
        LINEAR_BELOW_MEL + (hertz / LINEAR_BELOW_HZ).ln() / log_step()
    }
}

/// Mel back to hertz.
fn to_hertz(mel: f32) -> f32 {
    if mel < LINEAR_BELOW_MEL {
        mel * HERTZ_PER_MEL
    } else {
        LINEAR_BELOW_HZ * ((mel - LINEAR_BELOW_MEL) * log_step()).exp()
    }
}

/// How much of the log part of the scale one mel covers.
///
/// Twenty-seven bands spanning six and two fifths octaves, which is where the
/// numbers come from rather than from anything about speech.
fn log_step() -> f32 {
    f32::ln(6.4) / 27.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Whisper actually asks for.
    const BANDS: usize = 80;
    const FFT: usize = 400;
    const BINS: usize = 1 + FFT / 2;

    #[test]
    fn the_bank_is_one_row_per_band() {
        assert_eq!(filterbank(16_000, FFT, BANDS).len(), BANDS * BINS);
    }

    #[test]
    fn every_band_peaks_higher_than_the_one_below_it() {
        // The whole point of the scale: bands march up the spectrum in order.
        let bank = filterbank(16_000, FFT, BANDS);
        let peaks: Vec<usize> = (0..BANDS)
            .map(|band| {
                let row = &bank[band * BINS..(band + 1) * BINS];
                row.iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .map_or(0, |(bin, _)| bin)
            })
            .collect();
        assert!(
            peaks.windows(2).all(|pair| pair[1] >= pair[0]),
            "bands must be ordered by frequency"
        );
        assert!(peaks[0] < peaks[BANDS - 1]);
    }

    #[test]
    fn nothing_is_negative_and_nothing_is_wild() {
        let bank = filterbank(16_000, FFT, BANDS);
        assert!(bank.iter().all(|weight| *weight >= 0.0));
        assert!(bank.iter().all(|weight| weight.is_finite()));
    }

    #[test]
    fn the_bank_matches_the_one_whisper_was_trained_against() {
        // Values read from librosa's `mel(sr=16000, n_fft=400, n_mels=80)`,
        // which is what OpenAI used. A filterbank that drifted from this would
        // still transcribe, just worse, and nothing would say so.
        let bank = filterbank(16_000, FFT, BANDS);
        let peak = |band: usize| {
            bank[band * BINS..(band + 1) * BINS]
                .iter()
                .fold(0.0_f32, |most, weight| most.max(*weight))
        };

        for (band, expected) in [
            (0_usize, 0.024_862_6_f32),
            (40, 0.014_735_6),
            (79, 0.003_164_7),
        ] {
            let found = peak(band);
            assert!(
                (found - expected).abs() < 1e-5,
                "band {band} peaks at {found}, expected {expected}"
            );
        }
    }

    #[test]
    fn the_first_band_starts_at_the_bottom_of_the_spectrum() {
        let bank = filterbank(16_000, FFT, BANDS);
        assert!(bank[0] <= 0.0, "nothing sits at zero hertz");
        assert!(bank[1] > 0.0, "but the band next to it does");
    }
}
