//! Bursts in, text out, in order — without ever making the user wait.
//!
//! Recording and transcription run at different speeds, so they are separate
//! layers joined by a queue: the hotkey starts and stops recording instantly,
//! finished bursts queue up, and one worker thread decodes them in the
//! background. You can stop one burst and start the next while the first is
//! still decoding, and bursts recorded before the model has even loaded
//! simply wait for it.
//!
//! Order is guaranteed by construction: a single worker decodes one burst at a
//! time, so completions cannot interleave and nothing needs reordering.
//!
//! The transcriber is injected so this can be tested in milliseconds with a
//! fake, without a model or a GPU.

use crate::language::Language;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

/// One recording: 16 kHz mono samples, and the language it is in. Carried with
/// the burst so switching language never relabels bursts already queued.
pub struct Burst {
    pub samples: Vec<f32>,
    pub language: Language,
}

/// Turns one burst into text. May fail; the failure is shown and the queue
/// carries on with the next burst.
pub type Transcriber = Box<dyn FnMut(&Burst) -> anyhow::Result<String> + Send>;

/// Called from the worker whenever there is something new to show.
pub type Notify = Box<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Idle,
    Recording,
    Transcribing,
    Done,
    Error,
}

/// What the window shows. A snapshot; never holds the transcript, because the
/// transcript is user-editable and belongs to the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub phase: Phase,
    /// Bursts submitted but not yet turned into text.
    pub pending: usize,
    pub message: String,
}

pub struct Session {
    shared: Arc<Shared>,
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
    notify: Notify,
}

#[derive(Default)]
struct Inner {
    queue: VecDeque<Burst>,
    transcriber: Option<Transcriber>,
    loaded: bool,
    decoding: bool,
    recording: bool,
    appended: bool,
    finished: Vec<String>,
    /// Shown until the next recording starts: a failed burst, a hint.
    note: Option<String>,
    /// Shown only while loading, e.g. download progress.
    progress: Option<String>,
    /// Shown until it is resolved, which for a model that failed to load is
    /// never: recording again does not make the model appear.
    fatal: Option<String>,
    shutdown: bool,
}

impl Session {
    pub fn new(notify: Notify) -> Self {
        let shared = Arc::new(Shared { inner: Mutex::default(), wake: Condvar::new(), notify });
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("decode".into())
            .spawn(move || worker.run())
            .expect("spawn decode thread");
        Self { shared }
    }

    /// Supply the model once it has loaded. Queued bursts start draining.
    pub fn set_transcriber(&self, transcriber: Transcriber) {
        self.update(|s| {
            s.transcriber = Some(transcriber);
            s.loaded = true;
        });
        self.shared.wake.notify_all();
    }

    /// Only the status cue; audio capture itself lives elsewhere.
    pub fn set_recording(&self, recording: bool) {
        self.update(|s| {
            s.recording = recording;
            if recording {
                s.note = None;
                s.appended = false;
            }
        });
    }

    pub fn submit(&self, burst: Burst) {
        self.update(|s| s.queue.push_back(burst));
        self.shared.wake.notify_all();
    }

    /// Loading progress; disappears by itself once the model is in.
    pub fn progress(&self, message: impl Into<String>) {
        let message = message.into();
        self.update(|s| s.progress = Some(message));
    }

    /// A transient message, e.g. a failed burst or a hint.
    pub fn note(&self, message: impl Into<String>) {
        let message = message.into();
        self.update(|s| s.note = Some(message));
    }

    pub fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        self.update(|s| s.fatal = Some(message));
    }

    pub fn status(&self) -> Status {
        self.shared.lock().status()
    }

    /// Text finished since the last call, oldest first.
    pub fn take_finished(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().finished)
    }

    fn update(&self, change: impl FnOnce(&mut Inner)) {
        change(&mut self.shared.lock());
        (self.shared.notify)();
    }
}

impl Drop for Session {
    /// Signals the worker and does not wait for it. A decode in flight cannot
    /// be interrupted, and blocking quit on it would make the window hang for
    /// as long as the burst was long.
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.wake.notify_all();
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic mid-decode poisons the lock; the data is still coherent
        // (every mutation is a single assignment), so carry on.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(&self) {
        loop {
            let (burst, mut transcriber) = {
                let mut s = self.lock();
                while !s.shutdown && (s.queue.is_empty() || s.transcriber.is_none()) {
                    s = self.wake.wait(s).unwrap_or_else(|e| e.into_inner());
                }
                if s.shutdown {
                    return;
                }
                s.decoding = true;
                // Taken out so the decode runs without holding the lock.
                (s.queue.pop_front().unwrap(), s.transcriber.take().unwrap())
            };
            (self.notify)();

            let result = transcriber(&burst);

            {
                let mut s = self.lock();
                s.transcriber = Some(transcriber);
                s.decoding = false;
                match result {
                    Ok(text) if !text.trim().is_empty() => {
                        s.finished.push(text.trim().to_owned());
                        s.appended = true;
                    }
                    Ok(_) => {}
                    Err(e) => s.note = Some(format!("Transcription failed: {e:#}")),
                }
            }
            (self.notify)();
        }
    }
}

impl Inner {
    fn status(&self) -> Status {
        let pending = self.queue.len() + usize::from(self.decoding);
        let phase = if self.fatal.is_some() {
            Phase::Error
        } else if self.recording {
            Phase::Recording
        } else if !self.loaded {
            Phase::Loading
        } else if pending > 0 {
            Phase::Transcribing
        } else if self.appended {
            Phase::Done
        } else {
            Phase::Idle
        };
        let progress = self.progress.clone().filter(|_| phase == Phase::Loading);
        let message =
            self.fatal.clone().or_else(|| self.note.clone()).or(progress).unwrap_or_default();
        Status { phase, pending, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn burst(len: usize) -> Burst {
        Burst { samples: vec![0.0; len], language: Language::English }
    }

    fn quiet() -> Notify {
        Box::new(|| {})
    }

    /// Echoes the burst length, so each result identifies its burst.
    fn echo() -> Transcriber {
        Box::new(|b: &Burst| Ok(format!("{}", b.samples.len())))
    }

    fn wait_until(session: &Session, done: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let status = session.status();
            if done(&status) {
                return status;
            }
            assert!(Instant::now() < deadline, "timed out at {status:?}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn starts_loading() {
        assert_eq!(Session::new(quiet()).status().phase, Phase::Loading);
    }

    #[test]
    fn bursts_before_the_model_wait_for_it_and_keep_their_order() {
        let session = Session::new(quiet());
        session.submit(burst(1));
        session.submit(burst(2));
        session.submit(burst(3));
        assert_eq!(session.status().pending, 3);

        session.set_transcriber(echo());
        wait_until(&session, |s| s.phase == Phase::Done);
        assert_eq!(session.take_finished(), ["1", "2", "3"]);
        assert!(session.take_finished().is_empty(), "taking drains");
    }

    #[test]
    fn recording_is_never_blocked_by_a_decode_in_flight() {
        let (release, gate) = mpsc::channel::<()>();
        let session = Session::new(quiet());
        session.set_transcriber(Box::new(move |_: &Burst| {
            gate.recv().ok();
            Ok("text".into())
        }));

        session.submit(burst(10));
        wait_until(&session, |s| s.phase == Phase::Transcribing);

        // The next burst starts while the first is still decoding.
        session.set_recording(true);
        let status = session.status();
        assert_eq!(status.phase, Phase::Recording);
        assert_eq!(status.pending, 1);

        session.set_recording(false);
        session.submit(burst(10));
        assert_eq!(session.status().pending, 2);

        release.send(()).unwrap();
        release.send(()).unwrap();
        wait_until(&session, |s| s.pending == 0);
        assert_eq!(session.take_finished(), ["text", "text"]);
    }

    #[test]
    fn a_failed_burst_is_reported_and_the_queue_carries_on() {
        let session = Session::new(quiet());
        session.set_transcriber(Box::new(|b: &Burst| {
            if b.samples.len() == 1 { anyhow::bail!("boom") } else { Ok("fine".into()) }
        }));
        session.submit(burst(1));
        session.submit(burst(2));

        let status = wait_until(&session, |s| s.pending == 0);
        assert!(status.message.contains("boom"), "{status:?}");
        assert_eq!(session.take_finished(), ["fine"]);

        session.set_recording(true);
        assert_eq!(session.status().message, "", "a new burst clears the old failure");
    }

    #[test]
    fn each_burst_is_decoded_in_its_own_language() {
        let session = Session::new(quiet());
        session.submit(Burst { samples: vec![0.0], language: Language::Chinese });
        session.submit(Burst { samples: vec![0.0], language: Language::English });
        session.set_transcriber(Box::new(|b: &Burst| Ok(b.language.code().into())));
        wait_until(&session, |s| s.phase == Phase::Done);
        assert_eq!(session.take_finished(), ["zh", "en"]);
    }

    #[test]
    fn empty_text_appends_nothing() {
        let session = Session::new(quiet());
        session.set_transcriber(Box::new(|_: &Burst| Ok("  ".into())));
        session.submit(burst(1));
        let status = wait_until(&session, |s| s.pending == 0 && s.phase != Phase::Transcribing);
        assert_eq!(status.phase, Phase::Idle);
        assert!(session.take_finished().is_empty());
    }

    #[test]
    fn download_progress_disappears_once_the_model_is_in() {
        let session = Session::new(quiet());
        session.progress("Downloading model 40%");
        assert_eq!(session.status().message, "Downloading model 40%");
        session.set_transcriber(echo());
        assert_eq!(
            session.status(),
            Status { phase: Phase::Idle, pending: 0, message: String::new() }
        );
    }

    #[test]
    fn a_hint_survives_the_model_loading() {
        let session = Session::new(quiet());
        session.note("No global shortcut on this desktop");
        session.set_transcriber(echo());
        assert_eq!(session.status().message, "No global shortcut on this desktop");
    }

    #[test]
    fn a_fatal_error_outlives_new_recordings() {
        let session = Session::new(quiet());
        session.fail("model failed to load");
        session.set_recording(true);
        let status = session.status();
        assert_eq!(status.phase, Phase::Error);
        assert_eq!(status.message, "model failed to load");
    }

    #[test]
    fn the_worker_notifies_on_progress() {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let session = Session::new(Box::new(move || {
            tx.lock().unwrap().send(()).ok();
        }));
        session.set_transcriber(echo());
        session.submit(burst(1));
        wait_until(&session, |s| s.phase == Phase::Done);
        // set_transcriber, submit, decode start, decode end.
        assert!(rx.try_iter().count() >= 4);
    }

    #[test]
    fn dropping_with_a_model_that_never_arrived_does_not_hang() {
        let session = Session::new(quiet());
        session.submit(burst(1));
        let started = Instant::now();
        drop(session);
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
