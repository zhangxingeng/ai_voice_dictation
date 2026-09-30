//! The window: a status line with a level meter, the editable transcript, and
//! a copy button.
//!
//! egui because it is the least code for this: immediate mode means the
//! window is a function of the current state, redrawn when something changes,
//! with no widget tree to keep in sync.
//!
//! The transcript is editable on purpose -- Whisper gets technical words wrong
//! often enough that fixing them in place beats re-dictating -- so the window
//! owns the text and finished bursts are only ever appended to it. Appending
//! at the end never moves the caret, so a burst landing mid-correction does
//! not disturb the edit.

use crate::control::Dictation;
use crate::language::{self, Language};
use crate::session::Phase;
use crate::update::{self, Updater};
use eframe::egui::{self, Color32, Key, RichText, Sense, Vec2, ViewportCommand};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HINT: &str = "Super+Shift+D  record / stop      Esc  quit";

/// Fast enough for the meter to look live, slow enough to cost nothing.
const LIVE: Duration = Duration::from_millis(60);

const GREY: Color32 = Color32::from_rgb(0x5a, 0x5f, 0x6a);
const RED: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
const AMBER: Color32 = Color32::from_rgb(0xf5, 0xa5, 0x24);
const GREEN: Color32 = Color32::from_rgb(0x46, 0xa7, 0x58);

pub struct Window {
    dictation: Arc<Dictation>,
    updater: Updater,
    text: String,
    copied: Option<Instant>,
    pinned: bool,
}

impl Window {
    pub fn new(dictation: Arc<Dictation>, updater: Updater) -> Self {
        Self { dictation, updater, text: String::new(), copied: None, pinned: false }
    }

    fn append(&mut self, fragment: &str) {
        self.text.push_str(language::separator(&self.text, fragment));
        self.text.push_str(fragment);
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for fragment in self.dictation.session.take_finished() {
            self.append(&fragment);
        }
        let status = self.dictation.session.status();
        let ctx = ui.ctx().clone();

        // Always-on-top requested at creation arrives before the window is
        // mapped, and GNOME drops it. Asking again once it is on screen works.
        if !self.pinned {
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            self.pinned = true;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if matches!(status.phase, Phase::Recording | Phase::Transcribing | Phase::Loading)
            || self.copied.is_some()
        {
            ctx.request_repaint_after(LIVE);
        }

        egui::Panel::top("status").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (colour, caption) = cue(status.phase);
                dot(ui, colour);

                let caption = match (&status.message, status.pending) {
                    (m, _) if !m.is_empty() => m.clone(),
                    (_, 0) => caption.to_owned(),
                    (_, n) => format!("{caption} ({n})"),
                };
                let caption =
                    if self.copied.is_some_and(|t| t.elapsed() < Duration::from_millis(900)) {
                        "Copied".to_owned()
                    } else {
                        self.copied = None;
                        caption
                    };
                ui.label(RichText::new(caption).strong().size(15.0));

                if status.phase == Phase::Recording {
                    meter(ui, self.dictation.level());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(self.dictation.device()).weak());
                    update_control(ui, &self.updater);
                });
            });
            ui.add_space(6.0);
        });

        egui::Panel::bottom("actions").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(HINT).weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Copy").clicked() && !self.text.trim().is_empty() {
                        ctx.copy_text(self.text.trim().to_owned());
                        self.copied = Some(Instant::now());
                    }
                    ui.label(
                        RichText::new(format!("{} chars", self.text.trim().chars().count())).weak(),
                    );
                    language_picker(ui, &self.dictation);
                });
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut self.text)
                        .font(egui::FontId::proportional(16.0))
                        .hint_text("Press Super+Shift+D and speak."),
                );
            });
        });
    }
}

fn update_control(ui: &mut egui::Ui, updater: &Updater) {
    match updater.state() {
        update::State::Current => {}
        update::State::Available { version, .. } => {
            if ui.button(format!("Update to v{version}")).clicked() {
                updater.install();
            }
        }
        update::State::Installing { version } => {
            ui.label(RichText::new(format!("Installing v{version}…")).weak());
        }
        update::State::Installed { version } => {
            ui.label(
                RichText::new(format!("v{version} installed, restart to use it")).color(GREEN),
            );
        }
        update::State::Failed(message) => {
            ui.label(RichText::new(message).color(RED));
        }
    }
}

fn language_picker(ui: &mut egui::Ui, dictation: &Dictation) {
    let mut chosen = dictation.language();
    egui::ComboBox::from_id_salt("language").selected_text(chosen.label()).show_ui(ui, |ui| {
        for language in Language::ALL {
            ui.selectable_value(&mut chosen, language, language.label());
        }
    });
    if chosen != dictation.language() {
        dictation.set_language(chosen);
    }
}

/// Add the system's Chinese font as a fallback, so Chinese transcripts and
/// the picker's own label render instead of showing boxes.
///
/// Loaded from the system rather than embedded: a CJK font is ~20 MB, more
/// than the rest of the app. fontconfig picks the face -- on Ubuntu, Noto
/// Sans CJK SC from the `fonts-noto-cjk` package the .deb recommends. Without
/// one the app still works; Chinese text just shows as boxes.
pub fn install_fonts(ctx: &egui::Context) {
    let Ok(out) = std::process::Command::new("fc-match")
        .args(["--format=%{file}\n%{index}", "sans-serif:lang=zh-cn"])
        .output()
    else {
        return;
    };
    let out = String::from_utf8_lossy(&out.stdout);
    let mut lines = out.lines();
    let (Some(file), index) = (lines.next(), lines.next().and_then(|i| i.parse().ok())) else {
        return;
    };
    let Ok(bytes) = std::fs::read(file) else { return };
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index.unwrap_or(0);
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    let fallback = |family| InsertFontFamily { family, priority: FontPriority::Lowest };
    ctx.add_font(FontInsert::new(
        "chinese",
        data,
        vec![fallback(egui::FontFamily::Proportional), fallback(egui::FontFamily::Monospace)],
    ));
}

fn cue(phase: Phase) -> (Color32, &'static str) {
    match phase {
        Phase::Loading => (GREY, "Loading model"),
        Phase::Idle => (GREY, "Ready"),
        Phase::Recording => (RED, "Recording"),
        Phase::Transcribing => (AMBER, "Transcribing"),
        Phase::Done => (GREEN, "Done"),
        Phase::Error => (RED, "Error"),
    }
}

fn dot(ui: &mut egui::Ui, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 6.0, colour);
}

fn meter(ui: &mut egui::Ui, level: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(180.0, 10.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let mut bar = rect;
    // No minimum width: silence has to read as exactly empty. See meter.rs.
    bar.set_width(rect.width() * level.clamp(0.0, 1.0));
    painter.rect_filled(bar, 2.0, GREEN);
}
