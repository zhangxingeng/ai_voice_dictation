//! Whisper, via whisper.cpp.
//!
//! whisper.cpp does the whole pipeline -- mel spectrogram, encoder, KV-cached
//! decode, and seeking across 30-second windows by timestamp, which is what
//! keeps words at the window boundary from being lost.
//!
//! Built with Vulkan, it runs on whatever GPU the graphics driver exposes and
//! falls back to the CPU by itself when there is none. One thing must never be
//! called: `whisper_rs::vulkan::list_devices()` throws a C++ exception across
//! the FFI boundary on a machine without a Vulkan driver, which aborts the
//! process. `device()` asks ggml's device registry instead, which is what
//! whisper.cpp itself uses to choose.

use crate::language::Language;
use anyhow::{Context, Result};
use std::ffi::CStr;
use std::path::Path;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};
use whisper_rs_sys as sys;

pub struct Engine {
    state: WhisperState,
    threads: i32,
}

impl Engine {
    pub fn load(model: &Path) -> Result<Self> {
        // Otherwise whisper.cpp prints its whole model report to stderr.
        whisper_rs::install_logging_hooks();

        let path = model.to_str().context("model path is not UTF-8")?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("load {}", model.display()))?;
        let state = ctx.create_state().context("create decoder state")?;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;

        let mut engine = Self { state, threads };
        // The first run compiles GPU pipelines. Pay for it now, while the
        // window says "loading", rather than on the user's first sentence.
        engine.transcribe(&[0.0; 16_000], Language::default())?;
        Ok(engine)
    }

    pub fn transcribe(&mut self, samples: &[f32], language: Language) -> Result<String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language.code()));
        if let Some(prompt) = language.prompt() {
            params.set_initial_prompt(prompt);
        }
        params.set_n_threads(self.threads);
        params.set_suppress_nst(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);

        self.state.full(params, samples).context("transcribe")?;
        let mut text = String::new();
        for segment in self.state.as_iter() {
            text.push_str(&segment.to_str_lossy()?);
        }
        Ok(text.trim().to_owned())
    }
}

/// What whisper.cpp will run on, e.g. "NVIDIA GeForce RTX 3090 Ti" or "CPU".
///
/// Mirrors its choice: the first discrete GPU, else the first integrated one.
pub fn device() -> String {
    let kinds = [
        sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU,
        sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU,
    ];
    for kind in kinds {
        // SAFETY: plain lookups in ggml's static device registry; a null
        // device means none of that kind, and descriptions are static strings.
        unsafe {
            let dev = sys::ggml_backend_dev_by_type(kind);
            if !dev.is_null() {
                let description = sys::ggml_backend_dev_description(dev);
                if !description.is_null() {
                    return CStr::from_ptr(description).to_string_lossy().into_owned();
                }
            }
        }
    }
    "CPU".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the real model and a 16 kHz mono WAV, so it is opt-in:
    /// `DICTATION_WAV=clip.wav [DICTATION_LANG=zh] cargo test -- --ignored`
    #[test]
    #[ignore]
    fn transcribes_a_real_recording() {
        let wav = std::env::var("DICTATION_WAV").expect("set DICTATION_WAV");
        let bytes = std::fs::read(wav).unwrap();
        // Canonical 44-byte header, 16-bit PCM.
        let samples: Vec<f32> = bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b) as f32 / 32768.0)
            .collect();

        let model = crate::paths::models_dir().join(crate::models::FILENAME);
        let mut engine = Engine::load(&model).unwrap();
        let started = std::time::Instant::now();
        let language = match std::env::var("DICTATION_LANG").as_deref() {
            Ok("zh") => Language::Chinese,
            _ => Language::English,
        };
        let text = engine.transcribe(&samples, language).unwrap();
        eprintln!("{} in {:?}: {text}", device(), started.elapsed());
        assert!(!text.is_empty());
    }
}
