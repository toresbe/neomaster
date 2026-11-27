//! Finding, loading and atomically saving the settings file.
//!
//! The path is `--config PATH`, else `$NEOMASTER_CONFIG`, else `/etc/neomaster/settings.toml`
//! when running as root (the BM5), else `$XDG_CONFIG_HOME/neomaster/settings.toml` (the
//! simulator on a desktop).
//!
//! Loading never fails hard: a missing file is the defaults; a file that does not parse is
//! moved aside to `settings.toml.bad` and the defaults are used, so a broken hand edit cannot
//! keep the panel from starting. A file from a newer neomaster is read as far as it goes but
//! never overwritten, so a downgrade does not destroy it.
//!
//! Saving writes `settings.toml.tmp` in the same directory, fsyncs it, renames it over the old
//! file and fsyncs the directory: after a power cut the file is either the old or the new one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use log::{info, warn};

use crate::{Settings, VERSION};

/// The settings file's path for this process (see the module docs).
pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("NEOMASTER_CONFIG") {
        return PathBuf::from(p);
    }
    // SAFETY-free check for root: /proc/self/status would do too, but the uid is enough.
    if is_root() {
        return PathBuf::from("/etc/neomaster/settings.toml");
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("neomaster/settings.toml")
}

fn is_root() -> bool {
    // The owner of /proc/self is our effective uid.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata("/proc/self").map(|m| m.uid() == 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    false
}

/// What [`load`] found.
#[derive(Debug)]
pub struct Loaded {
    pub settings: Settings,
    /// False when the file came from a newer neomaster: [`Store`] will not overwrite it.
    pub writable: bool,
    /// Human-readable notes for the log (migrations, repairs, a file moved aside).
    pub notes: Vec<String>,
}

/// Parse a settings file's text: migrate, then repair out-of-range values.
pub fn parse(text: &str) -> Result<Loaded, toml::de::Error> {
    let mut value: toml::Table = toml::from_str(text)?;
    let mut notes = Vec::new();
    let version = value.get("version").and_then(|v| v.as_integer()).unwrap_or(0) as u32;
    let writable = version <= VERSION;
    if !writable {
        notes.push(format!("settings file is version {version}, newer than this neomaster ({VERSION}); not saving over it"));
    }
    migrate(&mut value, version, &mut notes);
    let mut settings: Settings = value.try_into()?;
    settings.version = VERSION;
    notes.extend(settings.normalise().into_iter().map(|n| format!("repaired {n}")));
    Ok(Loaded { settings, writable, notes })
}

/// Bring an older file's table up to [`VERSION`]. Version 0 is a file without a version key
/// (hand-written before the format existed); it has the same layout as version 1.
fn migrate(_value: &mut toml::Table, from: u32, notes: &mut Vec<String>) {
    if from < VERSION {
        notes.push(format!("settings file version {from} read as version {VERSION}"));
    }
}

/// Load the file at `path` (see the module docs for what happens on errors).
pub fn load(path: &Path) -> Loaded {
    match fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(l) => l,
            Err(e) => {
                let bad = path.with_extension("toml.bad");
                let moved = fs::rename(path, &bad).is_ok();
                let mut notes = vec![format!("{} does not parse ({e}); using the defaults", path.display())];
                if moved {
                    notes.push(format!("the broken file is kept as {}", bad.display()));
                }
                Loaded { settings: Settings::default(), writable: moved, notes }
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Loaded {
            settings: Settings::default(),
            writable: true,
            notes: vec![format!("no {}; using the defaults", path.display())],
        },
        Err(e) => Loaded {
            settings: Settings::default(),
            writable: false,
            notes: vec![format!("cannot read {} ({e}); using the defaults and not saving", path.display())],
        },
    }
}

/// Write `text` to `path` atomically.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    // Make the rename itself durable.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The settings file: where it is and whether it may be written.
#[derive(Debug, Clone)]
pub struct Store {
    pub path: PathBuf,
    writable: bool,
    last_saved: Option<String>,
}

impl Store {
    /// Load from `path`, logging the notes.
    pub fn open(path: PathBuf) -> (Store, Settings) {
        let loaded = load(&path);
        for n in &loaded.notes {
            if n.starts_with("no ") || n.contains("read as version") {
                info!("{n}");
            } else {
                warn!("{n}");
            }
        }
        info!("settings: {}", path.display());
        (Store { path, writable: loaded.writable, last_saved: None }, loaded.settings)
    }

    pub fn writable(&self) -> bool {
        self.writable
    }

    /// Save if anything changed since the last save. Does nothing for a read-only store.
    pub fn save(&mut self, settings: &Settings) -> io::Result<bool> {
        if !self.writable {
            return Ok(false);
        }
        let text = settings.to_toml();
        if self.last_saved.as_deref() == Some(text.as_str()) {
            return Ok(false);
        }
        write_atomic(&self.path, &text)?;
        self.last_saved = Some(text);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("neomaster-settings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_file_gives_defaults_and_saves() {
        let d = tmpdir("missing");
        let p = d.join("sub/settings.toml");
        let (mut store, mut s) = Store::open(p.clone());
        assert_eq!(s, Settings::default());
        s.system.ir_timeout = 9;
        assert!(store.save(&s).unwrap());
        assert!(!store.save(&s).unwrap(), "unchanged settings are not rewritten");
        assert!(!p.with_extension("toml.tmp").exists());
        assert_eq!(load(&p).settings.system.ir_timeout, 9);
    }

    #[test]
    fn broken_file_is_moved_aside() {
        let d = tmpdir("broken");
        let p = d.join("settings.toml");
        fs::write(&p, "this is [not toml").unwrap();
        let l = load(&p);
        assert_eq!(l.settings, Settings::default());
        assert!(l.writable);
        assert!(d.join("settings.toml.bad").exists());
    }

    #[test]
    fn newer_file_is_read_but_not_overwritten() {
        let d = tmpdir("newer");
        let p = d.join("settings.toml");
        let text = "version = 99\n[system]\nir_timeout = 3\n";
        fs::write(&p, text).unwrap();
        let (mut store, s) = Store::open(p.clone());
        assert_eq!(s.system.ir_timeout, 3);
        assert!(!store.writable());
        assert!(!store.save(&s).unwrap());
        assert_eq!(fs::read_to_string(&p).unwrap(), text);
    }

    #[test]
    fn unversioned_file_migrates() {
        let l = parse("[aux]\ncard = \"CODEC\"\n").unwrap();
        assert_eq!(l.settings.version, VERSION);
        assert_eq!(l.settings.aux.card, "CODEC");
        assert!(l.writable);
    }
}
