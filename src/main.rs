//! AI Voice Dictation: press a key anywhere, speak, press it again, get text.
//!
//! One binary, three modes:
//!
//! * `ai_voice_dictation` -- the app: a window, a microphone, and the model.
//! * `ai_voice_dictation --toggle` -- what the GNOME shortcut runs. Tells the
//!   running app to start or stop recording, then exits.
//! * `ai_voice_dictation --unbind` -- removes the shortcut from GNOME.
//!
//! The ordering here is the product: the window and the hotkey are live
//! within milliseconds, the model loads in the background, and bursts
//! recorded before it is ready wait in the queue rather than being refused.

mod audio;
mod control;
mod engine;
mod hotkey;
mod ipc;
mod language;
mod meter;
mod models;
mod paths;
mod session;
mod ui;
mod update;
mod vad;

use control::Dictation;
use eframe::egui;
use session::Session;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::thread;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => run(),
        Some("--toggle") => toggle(),
        Some("--unbind") => unbind(),
        Some(other) => {
            eprintln!("unknown argument {other}\nusage: ai_voice_dictation [--toggle | --unbind]");
            ExitCode::FAILURE
        }
    }
}

fn toggle() -> ExitCode {
    match ipc::send(&paths::socket(), ipc::TOGGLE) {
        Some(_) => ExitCode::SUCCESS,
        None => {
            eprintln!("ai_voice_dictation is not running");
            ExitCode::FAILURE
        }
    }
}

fn unbind() -> ExitCode {
    match hotkey::unregister(&hotkey::gsettings) {
        Ok(removed) => {
            println!("shortcut: {}", if removed { "removed" } else { "not present" });
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> ExitCode {
    // The window's context only exists once eframe has started, but the
    // session and the hotkey start before it. Until then there is simply
    // nothing to repaint.
    let window: Arc<OnceLock<egui::Context>> = Arc::default();
    let repaint = Arc::clone(&window);
    let session = Session::new(Box::new(move || {
        if let Some(ctx) = repaint.get() {
            ctx.request_repaint();
        }
    }));
    let dictation = Arc::new(Dictation::new(session));
    let repaint = Arc::clone(&window);
    let updater = update::Updater::check(move || {
        if let Some(ctx) = repaint.get() {
            ctx.request_repaint();
        }
    });

    let control = Arc::clone(&dictation);
    let _server = match ipc::Server::start(&paths::socket(), move |command| {
        if command == ipc::TOGGLE {
            control.toggle();
            ipc::OK.into()
        } else {
            ipc::UNKNOWN.into()
        }
    }) {
        Ok(server) => server,
        Err(ipc::StartError::AlreadyRunning) => {
            eprintln!("ai_voice_dictation is already running");
            return ExitCode::FAILURE;
        }
        Err(ipc::StartError::Io(e)) => {
            eprintln!("cannot open {}: {e}", paths::socket().display());
            return ExitCode::FAILURE;
        }
    };

    bind_hotkey(&dictation);
    load_model(Arc::clone(&dictation));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AI Voice Dictation")
            // Matches the .desktop file name, so the dock shows the launcher's
            // icon for this window instead of a second, generic one.
            .with_app_id("ai_voice_dictation")
            .with_inner_size([760.0, 460.0])
            .with_min_inner_size([420.0, 240.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "ai_voice_dictation",
        options,
        Box::new(move |cc| {
            ui::install_fonts(&cc.egui_ctx);
            window.set(cc.egui_ctx.clone()).ok();
            Ok(Box::new(ui::Window::new(dictation, updater)))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("window: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Point the GNOME shortcut at this binary, wherever it lives now.
fn bind_hotkey(dictation: &Dictation) {
    let run: hotkey::Run = &hotkey::gsettings;
    if !hotkey::available(run) {
        dictation.session.note("No global shortcut on this desktop");
        return;
    }
    let command = match std::env::current_exe() {
        Ok(exe) => format!("{} --toggle", exe.display()),
        Err(e) => return dictation.session.note(format!("Shortcut not set: {e}")),
    };
    if let Err(e) = hotkey::register(run, &command) {
        dictation.session.note(format!("Shortcut not set: {e:#}"));
    }
}

fn load_model(dictation: Arc<Dictation>) {
    thread::spawn(move || {
        let progress = |done: u64, total: Option<u64>| {
            let share = match total {
                Some(t) if t > 0 => format!("{}%", done * 100 / t),
                _ => format!("{} MB", done >> 20),
            };
            dictation.session.progress(format!("Downloading model {share}"));
        };
        let loaded = models::ensure(&paths::models_dir(), &progress).and_then(|path| {
            dictation.session.progress("Loading model");
            engine::Engine::load(&path)
        });
        match loaded {
            Ok(mut engine) => {
                dictation.set_device(engine::device());
                dictation.session.set_transcriber(Box::new(move |burst: &session::Burst| {
                    // Whisper invents text for silence; see vad.rs.
                    if !vad::has_speech(&burst.samples) {
                        return Ok(String::new());
                    }
                    engine.transcribe(&burst.samples, burst.language)
                }));
            }
            Err(e) => dictation.session.fail(format!("Model failed to load: {e:#}")),
        }
    });
}
