//! How the hotkey reaches the running app.
//!
//! Wayland gives ordinary clients no global hotkey. What works on GNOME
//! without root or an extension is a custom keybinding that runs a command --
//! here `ai_voice_dictation --toggle` -- which pokes the running app over this socket.
//!
//! A socket rather than a signal because the reply matters: the command has to
//! know whether anyone was listening, so it can say the app is not running
//! instead of silently doing nothing.
//!
//! Protocol: one line in, one line out. Both ends ship in the same binary, so
//! there is nothing to negotiate.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub const TOGGLE: &str = "toggle";
pub const OK: &str = "ok";
pub const UNKNOWN: &str = "unknown";

const TIMEOUT: Duration = Duration::from_secs(2);

/// Owns the socket file; removes it when dropped.
pub struct Server {
    path: PathBuf,
}

#[derive(Debug)]
pub enum StartError {
    AlreadyRunning,
    Io(std::io::Error),
}

impl Server {
    /// Bind and serve on a background thread. `handle` maps a command to a
    /// reply and runs on that thread.
    ///
    /// A socket file left by a crash must not block startup: if nothing
    /// answers it is stale and gets replaced. If something does answer,
    /// another instance owns it and this refuses rather than steal it.
    pub fn start(
        path: &Path,
        handle: impl Fn(&str) -> String + Send + 'static,
    ) -> Result<Self, StartError> {
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                return Err(StartError::AlreadyRunning);
            }
            std::fs::remove_file(path).map_err(StartError::Io)?;
        }
        let listener = UnixListener::bind(path).map_err(StartError::Io)?;

        // Not joined on shutdown: it blocks in accept() and dies with the
        // process, which is exactly when it should stop.
        thread::Builder::new()
            .name("ipc".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    // One bad client must never take the hotkey down.
                    let _ = serve(stream, &handle);
                }
            })
            .map_err(StartError::Io)?;

        Ok(Self { path: path.to_owned() })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn serve(stream: UnixStream, handle: &impl Fn(&str) -> String) -> std::io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let reply = handle(line.trim());
    (&stream).write_all(format!("{reply}\n").as_bytes())
}

/// Send one command. `None` means nothing is listening -- the app is not
/// running, which is an ordinary outcome rather than an error.
pub fn send(path: &Path, command: &str) -> Option<String> {
    let stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    (&stream).write_all(format!("{command}\n").as_bytes()).ok()?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply).ok()?;
    Some(reply.trim().to_owned()).filter(|r| !r.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_socket(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("ai_voice_dictation-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("s.sock")
    }

    fn echo(command: &str) -> String {
        if command == TOGGLE { OK.into() } else { UNKNOWN.into() }
    }

    #[test]
    fn round_trip() {
        let path = temp_socket("round");
        let _server = Server::start(&path, echo).unwrap();
        assert_eq!(send(&path, TOGGLE).as_deref(), Some(OK));
        assert_eq!(send(&path, "nonsense").as_deref(), Some(UNKNOWN));
    }

    #[test]
    fn every_command_reaches_the_handler() {
        let path = temp_socket("count");
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        let _server = Server::start(&path, move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            OK.into()
        })
        .unwrap();
        for _ in 0..5 {
            send(&path, TOGGLE).unwrap();
        }
        assert_eq!(seen.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn nothing_listening_is_none_not_an_error() {
        assert_eq!(send(&temp_socket("absent"), TOGGLE), None);
    }

    #[test]
    fn a_second_instance_is_refused() {
        let path = temp_socket("twice");
        let _first = Server::start(&path, echo).unwrap();
        assert!(matches!(Server::start(&path, echo), Err(StartError::AlreadyRunning)));
        assert_eq!(send(&path, TOGGLE).as_deref(), Some(OK), "the first keeps working");
    }

    #[test]
    fn a_stale_socket_from_a_crash_is_replaced() {
        let path = temp_socket("stale");
        drop(UnixListener::bind(&path).unwrap()); // leaves the file, nobody listening
        assert!(path.exists());
        let _server = Server::start(&path, echo).unwrap();
        assert_eq!(send(&path, TOGGLE).as_deref(), Some(OK));
    }

    #[test]
    fn dropping_the_server_removes_the_socket() {
        let path = temp_socket("drop");
        drop(Server::start(&path, echo).unwrap());
        assert!(!path.exists());
    }
}
