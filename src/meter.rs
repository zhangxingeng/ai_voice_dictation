//! The level meter's number: how loud the microphone is right now, 0..1.
//!
//! dB-scaled because speech sits around 0.01-0.1 RMS, where a linear bar
//! barely moves and looks broken.
//!
//! The floor sits above room tone (a quiet room measured -42 to -39.6 dBFS
//! on a real USB microphone), so the bar is empty until someone speaks. An empty bar while
//! talking then means one thing: the wrong microphone, or none at all.

const FLOOR_DB: f32 = -36.0;
const CEILING_DB: f32 = -12.0;

pub fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_square = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    if mean_square <= 0.0 {
        return 0.0;
    }
    let db = 10.0 * mean_square.log10();
    ((db - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_and_empty_read_zero() {
        assert_eq!(level(&[]), 0.0);
        assert_eq!(level(&[0.0; 1600]), 0.0);
    }

    #[test]
    fn room_tone_reads_zero() {
        // -39.6 dBFS: the loudest 100 ms of a quiet room, measured.
        assert_eq!(level(&[0.0105; 1600]), 0.0);
    }

    #[test]
    fn quiet_speech_still_registers() {
        assert!(level(&[0.025; 1600]) > 0.1);
    }

    #[test]
    fn full_scale_reads_one() {
        assert_eq!(level(&[1.0; 1600]), 1.0);
    }

    #[test]
    fn speech_levels_land_mid_scale() {
        // 0.05 RMS is ordinary speech into an ordinary microphone.
        let l = level(&[0.05; 1600]);
        assert!(l > 0.3 && l < 0.9, "{l}");
    }

    #[test]
    fn louder_is_higher() {
        assert!(level(&[0.1; 1600]) > level(&[0.01; 1600]));
    }
}
