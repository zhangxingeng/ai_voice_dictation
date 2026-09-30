//! Microphone capture, one burst at a time.
//!
//! Opening the stream takes milliseconds, so recording starts on the hotkey
//! press with nothing to wait for. Whisper wants 16 kHz mono: the device is
//! asked for that directly, and when it cannot do it the burst is captured at
//! the device's own rate and converted once, at the end.
//!
//! The stream lives on its own thread for the length of the burst, so the
//! recorder handle can be driven from any thread -- the hotkey arrives on the
//! socket's thread, not the window's.

use crate::meter;
use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

pub const SAMPLE_RATE: u32 = 16_000;

pub struct Recorder {
    captured: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    stop: mpsc::Sender<()>,
    thread: thread::JoinHandle<()>,
}

impl Recorder {
    pub fn start() -> Result<Self> {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = mpsc::channel();
        let (ready, opened) = mpsc::channel();

        let sink = Arc::clone(&captured);
        let thread =
            thread::Builder::new().name("capture".into()).spawn(move || match open(sink) {
                Ok((stream, rate)) => {
                    ready.send(Ok(rate)).ok();
                    stopped.recv().ok();
                    drop(stream);
                }
                Err(e) => {
                    ready.send(Err(e)).ok();
                }
            })?;

        let rate = opened.recv().context("capture thread died")??;
        Ok(Self { captured, rate, stop, thread })
    }

    /// The meter's reading: the last tenth of a second.
    pub fn level(&self) -> f32 {
        let captured = self.captured.lock().unwrap();
        let tail = (self.rate / 10) as usize;
        meter::level(&captured[captured.len().saturating_sub(tail)..])
    }

    /// End the burst and return it as 16 kHz mono.
    pub fn stop(self) -> Vec<f32> {
        self.stop.send(()).ok();
        self.thread.join().ok();
        let captured = std::mem::take(&mut *self.captured.lock().unwrap());
        resample(&captured, self.rate, SAMPLE_RATE)
    }
}

fn open(sink: Arc<Mutex<Vec<f32>>>) -> Result<(cpal::Stream, u32)> {
    let device = cpal::default_host().default_input_device().context("no microphone found")?;
    let supported = choose(&device)?;
    let config: StreamConfig = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, sink, |s| s),
        SampleFormat::I16 => build::<i16>(&device, &config, sink, |s| s as f32 / 32768.0),
        other => bail!("unsupported microphone sample format {other:?}"),
    }?;
    stream.play().context("start the microphone")?;
    Ok((stream, config.sample_rate))
}

/// 16 kHz float if the device offers it, which spares a conversion; otherwise
/// whatever the device prefers.
fn choose(device: &cpal::Device) -> Result<SupportedStreamConfig> {
    let exact = device.supported_input_configs().ok().and_then(|mut ranges| {
        ranges.find_map(|r| {
            (r.sample_format() == SampleFormat::F32).then(|| r.try_with_sample_rate(SAMPLE_RATE))?
        })
    });
    match exact {
        Some(config) => Ok(config),
        None => device.default_input_config().context("read the microphone's format"),
    }
}

fn build<T: SizedSample + Copy + Send + 'static>(
    device: &cpal::Device,
    config: &StreamConfig,
    sink: Arc<Mutex<Vec<f32>>>,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream> {
    let channels = config.channels.max(1) as usize;
    device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                let mut sink = sink.lock().unwrap();
                sink.extend(data.chunks(channels).map(|frame| {
                    frame.iter().map(|&s| to_f32(s)).sum::<f32>() / frame.len() as f32
                }));
            },
            // A dropout mid-burst loses a few milliseconds of audio; there is
            // nobody to tell and nothing better to do than keep recording.
            |_| {},
            None,
        )
        .map_err(|e| anyhow!("open the microphone: {e}"))
}

/// Convert sample rate by averaging each output sample's span of input.
///
/// The averaging is a crude low-pass filter, which matters when going down:
/// plain decimation of 48 kHz audio folds everything above 8 kHz back into
/// the speech band.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let len = (input.len() as f64 / ratio).floor() as usize;
    (0..len)
        .map(|i| {
            let start = (i as f64 * ratio) as usize;
            let end = (((i + 1) as f64 * ratio) as usize).clamp(start + 1, input.len());
            input[start..end].iter().sum::<f32>() / (end - start) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_untouched() {
        assert_eq!(resample(&[0.1, 0.2, 0.3], 16_000, 16_000), [0.1, 0.2, 0.3]);
    }

    #[test]
    fn duration_is_preserved() {
        let second = vec![0.0; 48_000];
        assert_eq!(resample(&second, 48_000, 16_000).len(), 16_000);
        assert_eq!(resample(&vec![0.0; 44_100], 44_100, 16_000).len(), 16_000);
    }

    #[test]
    fn content_below_nyquist_survives() {
        // A 440 Hz tone keeps its amplitude through 48k -> 16k.
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();
        let out = resample(&tone, 48_000, 16_000);
        let peak = out.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak > 0.95, "{peak}");
    }

    #[test]
    fn content_far_above_the_new_nyquist_is_attenuated() {
        // 15 kHz cannot exist at 16 kHz; decimation would alias it into
        // the speech band at full strength.
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 15_000.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();
        let out = resample(&tone, 48_000, 16_000);
        let peak = out.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak < 0.5, "{peak}");
    }
}
