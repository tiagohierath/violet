//! Keys to commands. The one keybind file is `~/.config/violet/keys.toml`;
//! `assets/keys.toml` is the template it is created from. Not modal: the pen
//! always paints, the left hand taps single keys.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{Key, Modifiers};
use serde::Deserialize;

use crate::commands::Invocation;

const TEMPLATE: &str = include_str!("../assets/keys.toml");

#[derive(Deserialize)]
struct File {
    keys: BTreeMap<String, String>,
}

/// A parsed `"ctrl+shift+z"`.
#[derive(PartialEq)]
struct Chord {
    ctrl: bool,
    shift: bool,
    alt: bool,
    key: Key,
}

/// Chords to command invocations. The first matching chord wins.
pub struct Keymap {
    binds: Vec<(Chord, Invocation)>,
    lines: Vec<(String, String)>,
    path: PathBuf,
    modified: Option<SystemTime>,
}

impl Keymap {
    /// The keys file at `~/.config/violet/keys.toml`, created from the
    /// template when it does not exist yet.
    pub fn load() -> Self {
        let path = config_file("keys.toml");
        if !path.exists() {
            let created = path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::write(&path, TEMPLATE));
            if let Err(err) = created {
                eprintln!("keys: cannot create {}: {err}", path.display());
            }
        }
        let mut keymap = Self::parse(TEMPLATE);
        keymap.path = path;
        keymap.reload_if_changed();
        keymap
    }

    /// Re-reads the keys file when it changed on disk, so an edit applies
    /// without a restart. Returns true when the bindings were replaced.
    pub fn reload_if_changed(&mut self) -> bool {
        let modified = fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if modified == self.modified {
            return false;
        }
        self.modified = modified;
        let text = fs::read_to_string(&self.path).unwrap_or_else(|_| TEMPLATE.to_owned());
        let fresh = Self::parse(&text);
        self.binds = fresh.binds;
        self.lines = fresh.lines;
        true
    }

    /// Bindings from keys-file text. A file that is not valid TOML falls back
    /// to the template; a bad line is reported and skipped.
    pub fn parse(text: &str) -> Self {
        let file: File = toml::from_str(text).unwrap_or_else(|err| {
            eprintln!("keys: {err}; using the template bindings");
            toml::from_str(TEMPLATE).expect("template parses")
        });
        let mut keymap = Self { binds: Vec::new(), lines: Vec::new(), path: PathBuf::new(), modified: None };
        for (chord, line) in file.keys {
            match (parse_chord(&chord), Invocation::parse(&line)) {
                (Some(parsed), Some(invocation)) => {
                    if keymap.binds.iter().any(|(bound, _)| *bound == parsed) {
                        eprintln!("keys: \"{chord}\" is bound twice; the first line wins");
                    }
                    keymap.binds.push((parsed, invocation));
                    keymap.lines.push((chord, line));
                }
                _ => eprintln!("keys: skipped \"{chord}\" = \"{line}\""),
            }
        }
        keymap
    }

    pub fn lookup(&self, key: Key, mods: Modifiers) -> Option<&Invocation> {
        // Symbols like + need shift on most layouts; a chord that does not
        // mention shift should not care about it for them.
        let symbol = matches!(key, Key::Plus | Key::Minus | Key::Equals | Key::OpenBracket | Key::CloseBracket);
        self.binds
            .iter()
            .find(|(chord, _)| {
                chord.key == key
                    && chord.ctrl == mods.ctrl
                    && chord.alt == mods.alt
                    && (chord.shift == mods.shift || (symbol && !chord.shift))
            })
            .map(|(_, invocation)| invocation)
    }

    /// Every binding as `(chord, command line)`, for the help listing.
    pub fn describe(&self) -> impl Iterator<Item = (&str, &str)> {
        self.lines.iter().map(|(chord, line)| (chord.as_str(), line.as_str()))
    }
}

/// `~/.config/violet/<name>`, following `$XDG_CONFIG_HOME` when set.
pub fn config_file(name: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("violet").join(name)
}

/// `"ctrl+shift+z"` to a `Chord`; `None` for an unknown key or modifier.
fn parse_chord(s: &str) -> Option<Chord> {
    let (mods, key) = match s.rsplit_once('+') {
        // A trailing "+" is the plus key itself: "ctrl++".
        Some((mods, "")) => (mods.trim_end_matches('+'), "+"),
        Some((mods, key)) => (mods, key),
        None => ("", s),
    };
    let mut chord = Chord { ctrl: false, shift: false, alt: false, key: key_from(key)? };
    for modifier in mods.split('+').filter(|m| !m.is_empty()) {
        match modifier {
            "ctrl" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" => chord.alt = true,
            _ => return None,
        }
    }
    Some(chord)
}

/// egui's key by name, also accepting lowercase (`"tab"` for `Tab`).
fn key_from(name: &str) -> Option<Key> {
    Key::from_name(name).or_else(|| {
        let mut chars = name.chars();
        let first = chars.next()?.to_ascii_uppercase();
        Key::from_name(&format!("{first}{}", chars.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_binding_parses() {
        let template: File = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(Keymap::parse(TEMPLATE).binds.len(), template.keys.len());
    }

    #[test]
    fn redo_needs_shift_undo_does_not() {
        let keymap = Keymap::parse(TEMPLATE);
        let name = |key, mods| keymap.lookup(key, mods).map(|i| i.name.as_str());
        let ctrl = Modifiers { ctrl: true, command: true, ..Default::default() };
        let ctrl_shift = Modifiers { shift: true, ..ctrl };
        assert_eq!(name(Key::Z, ctrl), Some("history.undo"));
        assert_eq!(name(Key::Z, ctrl_shift), Some("history.redo"));
        let shift = Modifiers { shift: true, ..Default::default() };
        assert_eq!(name(Key::Plus, shift), Some("view.zoom-in"));
    }

    #[test]
    fn edited_file_applies_without_restart() {
        let dir = std::env::temp_dir().join(format!("violet-keys-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.toml");
        fs::write(&path, "[keys]\n\"q\" = \"file.new\"\n").unwrap();
        let mut keymap = Keymap::parse(TEMPLATE);
        keymap.path = path.clone();
        assert!(keymap.reload_if_changed());
        let q = Modifiers::default();
        assert_eq!(keymap.lookup(Key::Q, q).map(|i| i.name.as_str()), Some("file.new"));
        assert!(!keymap.reload_if_changed());
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&path, "[keys]\n\"q\" = \"app.quit\"\n").unwrap();
        assert!(keymap.reload_if_changed());
        assert_eq!(keymap.lookup(Key::Q, q).map(|i| i.name.as_str()), Some("app.quit"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
