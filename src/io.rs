//! Files off the UI thread (ARCHITECTURE §6, §8). Saving gets a clone of
//! the document, which is cheap because tiles are shared and never
//! mutated, so painting carries on while it writes. Paths follow XDG.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use eframe::egui;

use violet::document::Document;
use violet::file;

/// Work for the file thread.
pub enum Job {
    /// `snapshot` is the silent recovery copy; `origin` is the real path
    /// that snapshot stands in for, kept beside it for recovery.
    Save {
        doc: Document,
        path: PathBuf,
        /// The document revision saved, echoed back in `Done::Saved`.
        rev: u64,
        snapshot: bool,
        origin: Option<PathBuf>,
    },
    /// `recovered` carries the original path of a recovered snapshot:
    /// `None` for an ordinary open, `Some(None)` for a snapshot of a
    /// drawing that was never saved.
    Load {
        path: PathBuf,
        recovered: Option<Option<PathBuf>>,
    },
    Export {
        doc: Document,
        path: PathBuf,
    },
}

/// A finished `Job`, reported back to the UI thread. `ms` is how long it
/// took.
pub enum Done {
    Saved { path: PathBuf, rev: u64, snapshot: bool, ms: f32 },
    Loaded { path: PathBuf, doc: Box<Document>, recovered: Option<Option<PathBuf>>, ms: f32 },
    Exported { path: PathBuf, ms: f32 },
    Failed { what: &'static str, path: PathBuf, err: String, snapshot: bool },
}

/// The file thread and its two channels.
pub struct Io {
    /// `None` until the first job starts the worker.
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
    /// To wake the UI when a job finishes.
    ctx: egui::Context,
    /// Saves queued or running, snapshots excluded.
    pub saving: usize,
    pub snapshotting: bool,
}

impl Io {
    pub fn new(ctx: egui::Context) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        Self { jobs: None, worker: None, done_tx, done_rx, ctx, saving: 0, snapshotting: false }
    }

    /// Queues a job. If the worker cannot take it, the failure arrives
    /// through `poll` like any other result.
    pub fn run(&mut self, job: Job) {
        let (what, path, snapshot) = match &job {
            Job::Save { path, snapshot, .. } => ("save", path.clone(), *snapshot),
            Job::Load { path, .. } => ("open", path.clone(), false),
            Job::Export { path, .. } => ("export", path.clone(), false),
        };
        match &job {
            Job::Save { snapshot: true, .. } => self.snapshotting = true,
            Job::Save { .. } => self.saving += 1,
            _ => {}
        }
        // One worker preserves request order: an older save must never
        // finish after a newer one and overwrite it. Start lazily.
        let result = (|| {
            if self.jobs.is_none() {
                let (jobs, requests) = mpsc::channel();
                let done_tx = self.done_tx.clone();
                let ctx = self.ctx.clone();
                let worker = std::thread::Builder::new()
                    .name("violet-io".into())
                    .spawn(move || {
                        for job in requests {
                            let _ = done_tx.send(work(job));
                            ctx.request_repaint();
                        }
                    })
                    .map_err(|e| e.to_string())?;
                self.jobs = Some(jobs);
                self.worker = Some(worker);
            }
            self.jobs
                .as_ref()
                .ok_or("File worker unavailable")?
                .send(job)
                .map_err(|e| e.to_string())
        })();
        if let Err(err) = result {
            let _ = self.done_tx.send(Done::Failed { what, path, err, snapshot });
            self.ctx.request_repaint();
        }
    }

    /// Results finished since the last poll; never blocks.
    pub fn poll(&mut self) -> Vec<Done> {
        let done: Vec<Done> = self.done_rx.try_iter().collect();
        for result in &done {
            match result {
                Done::Saved { snapshot: true, .. } | Done::Failed { snapshot: true, .. } => {
                    self.snapshotting = false
                }
                Done::Saved { .. } | Done::Failed { what: "save", .. } => {
                    self.saving = self.saving.saturating_sub(1)
                }
                _ => {}
            }
        }
        done
    }

    /// Runs a save right here, for quitting: the window is going away.
    pub fn save_now(doc: &Document, path: &Path) -> Result<(), String> {
        make_parent(path)?;
        file::save(doc, path, false)
    }

    /// Waits for every queued job. Dropping the sender ends the worker's
    /// loop once the queue is empty.
    pub fn finish(&mut self) {
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Io {
    fn drop(&mut self) {
        self.finish();
    }
}

fn make_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Runs one job on the worker thread.
fn work(job: Job) -> Done {
    let started = Instant::now();
    let ms = |since: Instant| since.elapsed().as_secs_f32() * 1000.0;
    match job {
        Job::Save { doc, path, rev, snapshot, origin } => {
            let result = make_parent(&path)
                .and_then(|()| file::save(&doc, &path, snapshot))
                .and_then(|()| {
                    if snapshot {
                        let note =
                            origin.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                        std::fs::write(path.with_extension("path"), note).map_err(|e| e.to_string())
                    } else {
                        Ok(())
                    }
                });
            match result {
                Ok(()) => Done::Saved { path, rev, snapshot, ms: ms(started) },
                Err(err) => Done::Failed { what: "save", path, err, snapshot },
            }
        }
        Job::Load { path, recovered } => match file::load(&path) {
            Ok(doc) => Done::Loaded { path, doc: Box::new(doc), recovered, ms: ms(started) },
            Err(err) => Done::Failed { what: "open", path, err, snapshot: false },
        },
        Job::Export { doc, path } => {
            match make_parent(&path).and_then(|()| file::export_png(&doc, &path)) {
                Ok(()) => Done::Exported { path, ms: ms(started) },
                Err(err) => Done::Failed { what: "export", path, err, snapshot: false },
            }
        }
    }
}

pub struct Paths {
    /// Where untitled drawings land.
    pub drawings: PathBuf,
    /// Small app state, such as the recent files list.
    pub state: PathBuf,
    /// Autosave snapshots, one per running session.
    pub recovery: PathBuf,
}

impl Paths {
    pub fn xdg() -> Self {
        let home =
            std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        // An XDG base directory, or its default under home.
        let env = |var: &str, rel: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(rel))
        };
        let pictures = std::env::var_os("XDG_PICTURES_DIR")
            .map(PathBuf::from)
            .or_else(|| user_dir(&home, "XDG_PICTURES_DIR"))
            .unwrap_or_else(|| home.join("Pictures"));
        Self {
            drawings: pictures.join("Violet"),
            state: env("XDG_STATE_HOME", ".local/state").join("violet"),
            recovery: env("XDG_DATA_HOME", ".local/share").join("violet").join("recovery"),
        }
    }

    /// This session's snapshot path, named by process id so a later
    /// session can tell whether its owner is still running.
    pub fn snapshot(&self) -> PathBuf {
        self.recovery.join(format!("{}.{}", std::process::id(), file::EXTENSION))
    }
}

/// `XDG_PICTURES_DIR="$HOME/Drawings"` from user-dirs.dirs.
fn user_dir(home: &Path, key: &str) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    let line = text.lines().find(|l| l.trim_start().starts_with(key))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"');
    let value = value.replace("$HOME", &home.to_string_lossy());
    Some(PathBuf::from(value)).filter(|p| p.is_absolute())
}

/// Expands a leading `~/`.
pub fn expand(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(s),
    }
}

/// Adds `.lily` when the path has no extension.
pub fn with_extension(p: PathBuf) -> PathBuf {
    if p.extension().is_some() { p } else { p.with_extension(file::EXTENSION) }
}

/// Flowers and creatures for untitled drawings' names: `lily-otter.lily`.
const FLOWERS: [&str; 25] = [
    "lily", "rose", "iris", "daisy", "poppy", "tulip", "violet", "aster", "clover", "lotus", "orchid", "peony", "dahlia",
    "lilac", "marigold", "pansy", "jasmine", "magnolia", "bluebell", "foxglove", "hyacinth", "thistle", "sunflower",
    "camellia", "begonia",
];
const CREATURES: [&str; 25] = [
    "otter", "fox", "hare", "owl", "wren", "moth", "finch", "badger", "heron", "newt", "lynx", "seal", "robin", "swan",
    "deer", "koi", "bee", "tern", "fawn", "dove", "gecko", "frog", "lark", "mole", "bat",
];

/// A name for a new drawing that is not in use yet: one flower and one
/// creature, with a number added if the pair is taken.
pub fn untitled(paths: &Paths) -> PathBuf {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let flower = FLOWERS[(next() % FLOWERS.len() as u64) as usize];
    let creature = CREATURES[(next() % CREATURES.len() as u64) as usize];
    let base = format!("{flower}-{creature}");
    (1..)
        .map(|n| {
            let stem = if n == 1 { base.clone() } else { format!("{base}-{n}") };
            paths.drawings.join(format!("{stem}.{}", file::EXTENSION))
        })
        .find(|p| !p.exists())
        .unwrap_or_else(|| paths.drawings.join(format!("{base}.{}", file::EXTENSION)))
}

/// The recent files list, newest first, minus files that are gone.
pub fn recent(paths: &Paths) -> Vec<PathBuf> {
    std::fs::read_to_string(paths.state.join("recent"))
        .map(|s| s.lines().map(PathBuf::from).filter(|p| p.exists()).collect())
        .unwrap_or_default()
}

/// Moves `path` to the front of the recent list (at most 20) and writes
/// the list out. Write errors are ignored: the list is a convenience.
pub fn remember(paths: &Paths, list: &mut Vec<PathBuf>, path: &Path) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(20);
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(paths.state.join("recent"), text);
}

/// The newest snapshot left behind by a session that is no longer running,
/// with the path it stood in for.
pub fn orphaned_snapshot(paths: &Paths) -> Option<(PathBuf, Option<PathBuf>)> {
    let entries = std::fs::read_dir(&paths.recovery).ok()?;
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == file::EXTENSION))
        .filter(|p| {
            // Named `<pid>.lily`; orphaned when that process is gone.
            let pid = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
            pid.is_some_and(|pid| {
                pid != std::process::id() && !Path::new(&format!("/proc/{pid}")).exists()
            })
        })
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    found.sort();
    let (_, path) = found.pop()?;
    let origin = std::fs::read_to_string(path.with_extension("path"))
        .ok()
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty());
    Some((path, origin))
}
