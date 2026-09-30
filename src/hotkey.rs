//! Registering the global hotkey as a GNOME custom keybinding.
//!
//! GNOME owns the grab and runs `ai_voice_dictation --toggle` when the key fires. No
//! root, no extension, nothing installed.
//!
//! Custom keybindings are an array of dconf paths (`.../customN/`), each
//! holding name/binding/command under a relocatable schema. Registering means:
//! find ours by name, or append a new path in the lowest free slot. Writing
//! `custom0` blindly would destroy whatever the user already had there.
//!
//! The key combination is written only when the entry is created. After that
//! it belongs to the user -- it shows up in Settings → Keyboard → Custom
//! Shortcuts, which is where to change it -- and an app that reasserts its
//! default on every launch silently undoes their choice.

use anyhow::{Context, Result, bail};
use std::process::Command;

/// Super is the desktop's modifier by convention; apps don't bind it, so a
/// global grab here shadows nothing. Every Ctrl/Alt/Shift combo belongs to the
/// focused app -- Ctrl+Shift+D was tried first and stole "bookmark all tabs"
/// from browsers.
pub const BINDING: &str = "<Super><Shift>d";
pub const NAME: &str = "AI voice dictation";

const PARENT: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CHILD: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const PREFIX: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";

/// Runs `gsettings <args>` and returns stdout. Injected so tests never touch
/// the real dconf database: a test that rewrote the user's shortcuts would be
/// unforgivable.
pub type Run<'a> = &'a dyn Fn(&[&str]) -> Result<String>;

pub fn gsettings(args: &[&str]) -> Result<String> {
    let out = Command::new("gsettings").args(args).output().context("run gsettings")?;
    if !out.status.success() {
        bail!("gsettings {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether this is a GNOME session we can register with. False is not an
/// error: the app still works, just without a global key.
pub fn available(run: Run) -> bool {
    run(&["get", PARENT, "custom-keybindings"]).is_ok()
}

/// Make sure our binding exists and runs `command`. Returns its dconf path.
pub fn register(run: Run, command: &str) -> Result<String> {
    if let Some(path) = find(run)? {
        set(run, &path, "name", NAME)?;
        set(run, &path, "command", command)?;
        return Ok(path);
    }

    let mut paths = list(run)?;
    let used: Vec<u32> = paths
        .iter()
        .filter_map(|p| {
            p.trim_end_matches('/').rsplit('/').next()?.strip_prefix("custom")?.parse().ok()
        })
        .collect();
    let slot = (0..).find(|n| !used.contains(n)).unwrap();
    let path = format!("{PREFIX}custom{slot}/");

    paths.push(path.clone());
    run(&["set", PARENT, "custom-keybindings", &to_gvariant(&paths)])?;
    set(run, &path, "name", NAME)?;
    set(run, &path, "binding", BINDING)?;
    set(run, &path, "command", command)?;
    Ok(path)
}

/// Remove our binding, leaving every other entry in its original order.
/// True if there was one.
pub fn unregister(run: Run) -> Result<bool> {
    let Some(ours) = find(run)? else { return Ok(false) };
    let rest: Vec<String> = list(run)?.into_iter().filter(|p| *p != ours).collect();
    run(&["set", PARENT, "custom-keybindings", &to_gvariant(&rest)])?;
    Ok(true)
}

fn find(run: Run) -> Result<Option<String>> {
    for path in list(run)? {
        // An unreadable entry is someone else's problem; skip it.
        if let Ok(name) = run(&["get", &format!("{CHILD}:{path}"), "name"])
            && unquote(&name) == NAME
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn list(run: Run) -> Result<Vec<String>> {
    Ok(parse_gvariant(&run(&["get", PARENT, "custom-keybindings"])?))
}

fn set(run: Run, path: &str, key: &str, value: &str) -> Result<()> {
    run(&["set", &format!("{CHILD}:{path}"), key, value]).map(drop)
}

/// `@as []`, `[]`, or `['/a/', '/b/']`. Paths never contain quotes or commas.
fn parse_gvariant(value: &str) -> Vec<String> {
    let value = value.trim();
    let value = value.strip_prefix("@as").unwrap_or(value).trim();
    let inner = value.trim_start_matches('[').trim_end_matches(']');
    inner.split(',').map(unquote).filter(|s| !s.is_empty()).collect()
}

fn to_gvariant(paths: &[String]) -> String {
    let quoted: Vec<String> = paths.iter().map(|p| format!("'{p}'")).collect();
    format!("[{}]", quoted.join(", "))
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches('\'').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// A pretend dconf: `schema[:path] key` -> value, as gsettings prints it.
    struct Fake(RefCell<BTreeMap<String, String>>);

    impl Fake {
        fn with(array: &str) -> Self {
            let fake = Fake(RefCell::new(BTreeMap::new()));
            fake.put(PARENT, "custom-keybindings", array);
            fake
        }

        fn put(&self, schema: &str, key: &str, value: &str) {
            self.0.borrow_mut().insert(format!("{schema} {key}"), value.to_owned());
        }

        fn get(&self, schema: &str, key: &str) -> Option<String> {
            self.0.borrow().get(&format!("{schema} {key}")).cloned()
        }

        fn child(&self, path: &str, key: &str) -> Option<String> {
            self.get(&format!("{CHILD}:{path}"), key).map(|v| unquote(&v))
        }

        fn array(&self) -> Vec<String> {
            parse_gvariant(&self.get(PARENT, "custom-keybindings").unwrap())
        }

        fn run(&self, args: &[&str]) -> Result<String> {
            match args {
                ["get", schema, key] => {
                    // Unset child keys read as '' in real gsettings.
                    Ok(self.get(schema, key).unwrap_or_else(|| "''".into()))
                }
                ["set", schema, key, value] => {
                    self.put(schema, key, value);
                    Ok(String::new())
                }
                _ => bail!("unexpected {args:?}"),
            }
        }
    }

    const SOMEONE: &str =
        "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/";

    #[test]
    fn parses_every_form_gsettings_prints() {
        assert!(parse_gvariant("@as []\n").is_empty());
        assert!(parse_gvariant("[]").is_empty());
        assert_eq!(parse_gvariant("['/a/', '/b/']\n"), ["/a/", "/b/"]);
    }

    #[test]
    fn registers_into_an_empty_list() {
        let fake = Fake::with("@as []");
        let path = register(&|a| fake.run(a), "/usr/bin/ai_voice_dictation --toggle").unwrap();
        assert!(path.ends_with("/custom0/"));
        assert_eq!(fake.array(), std::slice::from_ref(&path));
        assert_eq!(fake.child(&path, "binding").unwrap(), BINDING);
        assert_eq!(fake.child(&path, "command").unwrap(), "/usr/bin/ai_voice_dictation --toggle");
    }

    #[test]
    fn never_takes_a_slot_that_is_in_use() {
        let fake = Fake::with(&format!("['{SOMEONE}']"));
        fake.put(&format!("{CHILD}:{SOMEONE}"), "name", "'terminal'");
        let path = register(&|a| fake.run(a), "cmd").unwrap();
        assert!(path.ends_with("/custom1/"));
        assert_eq!(fake.array(), [SOMEONE.to_owned(), path]);
        assert_eq!(fake.child(SOMEONE, "name").unwrap(), "terminal", "theirs untouched");
    }

    #[test]
    fn registering_twice_updates_in_place() {
        let fake = Fake::with("@as []");
        let first = register(&|a| fake.run(a), "old").unwrap();
        let second = register(&|a| fake.run(a), "new").unwrap();
        assert_eq!(first, second);
        assert_eq!(fake.array().len(), 1);
        assert_eq!(fake.child(&first, "command").unwrap(), "new");
    }

    #[test]
    fn a_key_the_user_changed_is_left_alone() {
        let fake = Fake::with("@as []");
        let path = register(&|a| fake.run(a), "cmd").unwrap();
        fake.put(&format!("{CHILD}:{path}"), "binding", "'<Super>F9'");
        register(&|a| fake.run(a), "cmd").unwrap();
        assert_eq!(fake.child(&path, "binding").unwrap(), "<Super>F9");
    }

    #[test]
    fn unregister_removes_only_ours() {
        let fake = Fake::with(&format!("['{SOMEONE}']"));
        register(&|a| fake.run(a), "cmd").unwrap();
        assert!(unregister(&|a| fake.run(a)).unwrap());
        assert_eq!(fake.array(), [SOMEONE]);
        assert!(!unregister(&|a| fake.run(a)).unwrap(), "second time: nothing there");
    }

    #[test]
    fn unavailable_when_gsettings_fails() {
        assert!(!available(&|_| bail!("no schema")));
        assert!(available(&|_| Ok("@as []".into())));
    }
}
