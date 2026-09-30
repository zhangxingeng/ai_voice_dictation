//! Updating from GitHub Releases.
//!
//! A .deb cannot update itself -- installing one needs root -- and an apt
//! repository is more machinery than a personal tool deserves. So the app
//! checks the latest release at launch, and if it is newer, offers a button
//! that downloads the .deb and installs it through `pkexec`: the same
//! graphical password prompt Ubuntu's own updater uses.
//!
//! Every failure here is silent or confined to the status line. Being offline
//! must never get in the way of dictating.

use anyhow::{Context, Result, anyhow, bail};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

const LATEST: &str = "https://api.github.com/repos/zhangxingeng/ai_voice_dictation/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Up to date, not checked yet, or the check failed.
    Current,
    Available {
        version: String,
        url: String,
    },
    Installing {
        version: String,
    },
    /// On disk; takes effect when the app is restarted.
    Installed {
        version: String,
    },
    Failed(String),
}

#[derive(Clone)]
pub struct Updater {
    state: Arc<Mutex<State>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Updater {
    /// Start checking in the background.
    pub fn check(notify: impl Fn() + Send + Sync + 'static) -> Self {
        let updater =
            Self { state: Arc::new(Mutex::new(State::Current)), notify: Arc::new(notify) };
        let checker = updater.clone();
        thread::spawn(move || {
            if let Ok(Some((version, url))) = latest(env!("CARGO_PKG_VERSION")) {
                checker.set(State::Available { version, url });
            }
        });
        updater
    }

    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Download and install the available version. Does nothing otherwise.
    pub fn install(&self) {
        let State::Available { version, url } = self.state() else { return };
        self.set(State::Installing { version: version.clone() });
        let updater = self.clone();
        thread::spawn(move || {
            updater.set(match download_and_install(&url) {
                Ok(()) => State::Installed { version },
                Err(e) => State::Failed(format!("Update failed: {e:#}")),
            });
        });
    }

    fn set(&self, state: State) {
        *self.state.lock().unwrap() = state;
        (self.notify)();
    }
}

/// The newest release and its .deb, if it is newer than `current`.
fn latest(current: &str) -> Result<Option<(String, String)>> {
    let release: serde_json::Value = ureq::get(LATEST).call()?.body_mut().read_json()?;
    newer_deb(&release, current)
}

fn newer_deb(release: &serde_json::Value, current: &str) -> Result<Option<(String, String)>> {
    let tag = release["tag_name"].as_str().context("release has no tag")?;
    let version = tag.trim_start_matches('v');
    if parse(version)? <= parse(current)? {
        return Ok(None);
    }
    let url = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["browser_download_url"].as_str())
        .find(|u| u.ends_with("_amd64.deb"))
        .context("release has no .deb")?;
    Ok(Some((version.to_owned(), url.to_owned())))
}

fn parse(version: &str) -> Result<(u32, u32, u32)> {
    let mut parts = version.split('.').map(str::parse::<u32>);
    let mut next = || parts.next().unwrap_or(Ok(0)).map_err(|_| anyhow!("bad version {version}"));
    Ok((next()?, next()?, next()?))
}

fn download_and_install(url: &str) -> Result<()> {
    let dir =
        std::env::temp_dir().join(format!("ai_voice_dictation-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let deb = dir.join("update.deb");
    let mut body = ureq::get(url).call().context("download")?.into_body().into_reader();
    std::io::copy(&mut body, &mut std::fs::File::create(&deb)?).context("download")?;

    // apt-get rather than apt: apt warns that its CLI is not stable for
    // scripts. The path must be absolute for apt to treat it as a file.
    let status = Command::new("pkexec")
        .args(["apt-get", "install", "-y"])
        .arg(&deb)
        .status()
        .context("run pkexec")?;
    let _ = std::fs::remove_dir_all(&dir);
    match status.code() {
        Some(0) => Ok(()),
        // pkexec's codes for a dismissed or refused password prompt.
        Some(126 | 127) => bail!("not authorised"),
        _ => bail!("apt-get exited with {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str) -> serde_json::Value {
        json!({
            "tag_name": tag,
            "assets": [
                { "browser_download_url": "https://example.com/notes.txt" },
                { "browser_download_url": format!("https://example.com/ai-voice-dictation_{}-1_amd64.deb", tag.trim_start_matches('v')) },
            ]
        })
    }

    #[test]
    fn a_newer_release_is_offered_with_its_deb() {
        let (version, url) = newer_deb(&release("v0.2.0"), "0.1.0").unwrap().unwrap();
        assert_eq!(version, "0.2.0");
        assert!(url.ends_with("0.2.0-1_amd64.deb"));
    }

    #[test]
    fn the_same_or_an_older_release_is_not() {
        assert_eq!(newer_deb(&release("v0.1.0"), "0.1.0").unwrap(), None);
        assert_eq!(newer_deb(&release("v0.0.9"), "0.1.0").unwrap(), None);
    }

    #[test]
    fn versions_compare_numerically_not_as_text() {
        assert!(newer_deb(&release("v0.10.0"), "0.9.0").unwrap().is_some());
    }

    /// Hits the real GitHub API: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_live_latest_release_is_offered_to_an_older_version() {
        let (version, url) = latest("0.0.1").unwrap().expect("a release newer than 0.0.1");
        eprintln!("v{version}: {url}");
        assert!(url.ends_with("_amd64.deb"));
    }

    #[test]
    fn a_release_without_a_deb_is_an_error_not_an_offer() {
        let bare = json!({ "tag_name": "v9.0.0", "assets": [] });
        assert!(newer_deb(&bare, "0.1.0").is_err());
    }
}
