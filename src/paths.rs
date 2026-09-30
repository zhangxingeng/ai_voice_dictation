//! Where things live on disk, per the XDG base directory spec.

use std::env;
use std::path::PathBuf;

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(fallback),
    }
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Downloaded models. Hundreds of megabytes, so never packaged.
pub fn models_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("ai_voice_dictation").join("models")
}

/// The socket the hotkey command talks to.
///
/// $XDG_RUNTIME_DIR is user-private, on tmpfs, and emptied at logout, so a
/// socket left behind by a crash cannot outlive the session. /tmp only when it
/// is unset, which happens over bare ssh and in containers.
pub fn socket() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("ai_voice_dictation.sock"),
        _ => env::temp_dir().join("ai_voice_dictation.sock"),
    }
}
