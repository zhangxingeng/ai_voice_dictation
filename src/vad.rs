//! Is there any speech in this burst at all?
//!
//! Whisper hallucinates confidently on silence -- digital silence decodes to
//! "you", room tone to "." -- so pressing the hotkey and saying nothing would
//! insert invented text. The gate comes from the audio, not the model.
//!
//! Measured on a real microphone:
//!
//! ```text
//! room tone   rms 0.0086   crest 1.1   (steady, flat)
//! speech      rms 0.1421   crest 5.5   (peaky, intermittent)
//! ```
//!
//! An absolute threshold would encode one microphone's gain. Instead the noise
//! floor is estimated from the burst itself and speech is whatever stands well
//! above it, which works at any gain.

const FRAME: usize = 480; // 30 ms at 16 kHz

/// A frame is speech at this multiple of the noise floor. Room tone sits
/// ~1.1x its own floor; speech peaks 15x and more above it.
const FLOOR_MULTIPLE: f32 = 4.0;

/// Below this nothing counts, however it compares with the floor: dividing by
/// digital silence makes every ratio meaningless.
const ABS_FLOOR: f32 = 0.003;

/// Share of frames that must qualify. Low enough to pass one short word in a
/// long burst, high enough that a single click or key tap does not.
const MIN_SPEECH_FRACTION: f32 = 0.05;

pub fn has_speech(samples: &[f32]) -> bool {
    speech_fraction(samples) >= MIN_SPEECH_FRACTION
}

fn speech_fraction(samples: &[f32]) -> f32 {
    let mut rms: Vec<f32> = samples
        .as_chunks::<FRAME>()
        .0
        .iter()
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt())
        .collect();
    if rms.is_empty() {
        return 0.0;
    }
    // The 10th percentile approximates "the quiet parts" without assuming any
    // particular amount of silence is present.
    rms.sort_by(f32::total_cmp);
    let floor = rms[rms.len() / 10];
    let threshold = (floor * FLOOR_MULTIPLE).max(ABS_FLOOR);
    rms.iter().filter(|&&r| r > threshold).count() as f32 / rms.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amplitude * (i as f32 * 0.07).sin()).collect()
    }

    #[test]
    fn empty_and_digital_silence_are_not_speech() {
        assert!(!has_speech(&[]));
        assert!(!has_speech(&[0.0; 16_000]));
    }

    #[test]
    fn steady_room_tone_is_not_speech_at_any_gain() {
        assert!(!has_speech(&tone(0.01, 32_000)));
        assert!(!has_speech(&tone(0.2, 32_000)));
    }

    #[test]
    fn a_short_word_in_a_long_quiet_burst_is_speech() {
        let mut burst = tone(0.005, 5 * 16_000);
        burst.extend(tone(0.2, 8_000)); // half a second of "yes"
        assert!(has_speech(&burst));
    }

    #[test]
    fn a_single_click_is_not_speech() {
        let mut burst = tone(0.005, 5 * 16_000);
        burst[40_000..40_000 + FRAME].fill(0.8);
        assert!(!has_speech(&burst));
    }
}
