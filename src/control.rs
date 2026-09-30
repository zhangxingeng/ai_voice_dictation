//! The one action the hotkey performs: start a burst, or end it and queue it.
//!
//! Callable from any thread. The hotkey arrives on the socket's thread and
//! must work while the window is unfocused or hidden, so nothing here waits
//! for the window.

use crate::audio::Recorder;
use crate::language::Language;
use crate::session::{Burst, Session};
use std::sync::Mutex;

pub struct Dictation {
    pub session: Session,
    recorder: Mutex<Option<Recorder>>,
    language: Mutex<Language>,
    device: Mutex<String>,
}

impl Dictation {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            recorder: Mutex::new(None),
            language: Mutex::new(Language::default()),
            device: Mutex::new(String::new()),
        }
    }

    pub fn toggle(&self) {
        let mut recorder = self.recorder.lock().unwrap();
        match recorder.take() {
            Some(burst) => {
                self.session.set_recording(false);
                // Bound at stop, so switching language mid-sentence applies
                // to the burst being recorded; queued ones keep their own.
                let language = self.language();
                self.session.submit(Burst { samples: burst.stop(), language });
            }
            None => match Recorder::start() {
                Ok(started) => {
                    *recorder = Some(started);
                    self.session.set_recording(true);
                }
                Err(e) => self.session.note(format!("Microphone: {e:#}")),
            },
        }
    }

    /// 0..1 while recording, 0 otherwise.
    pub fn level(&self) -> f32 {
        self.recorder.lock().unwrap().as_ref().map_or(0.0, Recorder::level)
    }

    /// Applies to the burst being recorded and every one after it.
    pub fn language(&self) -> Language {
        *self.language.lock().unwrap()
    }

    pub fn set_language(&self, language: Language) {
        *self.language.lock().unwrap() = language;
    }

    /// What the model runs on, once it has loaded.
    pub fn device(&self) -> String {
        self.device.lock().unwrap().clone()
    }

    pub fn set_device(&self, device: String) {
        *self.device.lock().unwrap() = device;
    }
}
