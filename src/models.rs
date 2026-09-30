//! Fetching the Whisper model on first run.
//!
//! large-v3-turbo quantised to q5_0: 574 MB, and measured as fast and as
//! accurate as q8_0 (874 MB) on this workload. Pinned to a commit and a
//! sha256, so the file cannot change underneath us.
//!
//! The download goes to a `.part` file beside the target and is renamed into
//! place only after the hash checks out. A rename within one directory is
//! atomic, so a download killed at any point leaves either no model or a
//! complete, verified one -- never a truncated file that looks present.

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const FILENAME: &str = "ggml-large-v3-turbo-q5_0.bin";
const URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/\
                   5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q5_0.bin";
const SHA256: &str = "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2";

/// Called with (bytes so far, total bytes if known).
pub type Progress<'a> = &'a dyn Fn(u64, Option<u64>);

/// The model's path, downloading it first if it is not there yet.
///
/// Presence is checked by existence, not by re-hashing half a gigabyte on
/// every launch: the rename above is what makes existence trustworthy.
pub fn ensure(dir: &Path, progress: Progress) -> Result<PathBuf> {
    let target = dir.join(FILENAME);
    if target.is_file() {
        return Ok(target);
    }
    let response = ureq::get(URL).call().context("download the model")?;
    let total = response.body().content_length();
    install(response.into_body().into_reader(), total, &target, SHA256, progress)?;
    Ok(target)
}

fn install(
    mut source: impl Read,
    total: Option<u64>,
    target: &Path,
    sha256: &str,
    progress: Progress,
) -> Result<()> {
    let dir = target.parent().context("model path has no directory")?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let part = target.with_extension("part");

    let result = (|| -> Result<()> {
        let mut file = File::create(&part)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0; 1 << 20];
        let mut done = 0u64;
        loop {
            let n = source.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            done += n as u64;
            progress(done, total);
        }
        file.sync_all()?;
        let actual = hex(&hasher.finalize());
        ensure!(actual == sha256, "model checksum mismatch: got {actual}");
        fs::rename(&part, target)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("ai_voice_dictation-models-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("nested").join(FILENAME)
    }

    fn sha(data: &[u8]) -> String {
        hex(&Sha256::digest(data))
    }

    #[test]
    fn installs_a_verified_file_and_reports_progress() {
        let target = scratch("ok");
        let data = vec![7u8; 3 << 20];
        let last = Cell::new(0);
        install(&data[..], Some(data.len() as u64), &target, &sha(&data), &|done, _| {
            last.set(done)
        })
        .unwrap();
        assert_eq!(fs::read(&target).unwrap(), data);
        assert_eq!(last.get(), data.len() as u64);
        assert!(!target.with_extension("part").exists());
    }

    #[test]
    fn a_corrupt_download_leaves_nothing_behind() {
        let target = scratch("bad");
        let err = install(&b"not the model"[..], None, &target, &sha(b"the model"), &|_, _| {});
        assert!(err.unwrap_err().to_string().contains("checksum"));
        assert!(!target.exists(), "must not look present");
        assert!(!target.with_extension("part").exists());
    }

    #[test]
    fn an_interrupted_download_leaves_nothing_behind() {
        struct Dies(usize);
        impl Read for Dies {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::other("connection reset"));
                }
                self.0 -= 1;
                buf[0] = 1;
                Ok(1)
            }
        }
        let target = scratch("cut");
        assert!(install(Dies(10), None, &target, "x", &|_, _| {}).is_err());
        assert!(!target.exists());
        assert!(!target.with_extension("part").exists());
    }

    #[test]
    fn an_existing_model_is_not_downloaded_again() {
        let target = scratch("present");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"already here").unwrap();
        // The progress callback panics if a download is attempted.
        let path = ensure(target.parent().unwrap(), &|_, _| panic!("downloaded")).unwrap();
        assert_eq!(path, target);
    }
}
