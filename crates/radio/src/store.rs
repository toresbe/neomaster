//! The presets' own file, until they move into the settings file.
//!
//! The path is `$NEOMASTER_RADIO_PRESETS`, else `/etc/neomaster/radio-presets.toml` when running
//! as root (the BM5), else `$XDG_CONFIG_HOME/neomaster/radio-presets.toml`: next to where the
//! settings file goes. Loading never fails hard (a missing or broken file is an empty list, a
//! broken one is moved aside to `.bad`); saving is atomic (temporary file, fsync, rename).

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use log::warn;

use crate::Presets;

pub const FILE_NAME: &str = "radio-presets.toml";

pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("NEOMASTER_RADIO_PRESETS") {
        return PathBuf::from(p);
    }
    if is_root() {
        return Path::new("/etc/neomaster").join(FILE_NAME);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("neomaster").join(FILE_NAME)
}

fn is_root() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata("/proc/self").map(|m| m.uid() == 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    false
}

/// Parse a presets file's text, normalised (see [`Presets::normalise`]).
pub fn parse(text: &str) -> Result<Presets, toml::de::Error> {
    let mut p: Presets = toml::from_str(text)?;
    for d in p.normalise() {
        warn!("N.RADIO presets: dropped {d} (number out of range or repeated)");
    }
    Ok(p)
}

/// Load the presets; an absent file is an empty list.
pub fn load(path: &Path) -> Presets {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Presets::default(),
        Err(e) => {
            warn!("N.RADIO presets: {}: {e}", path.display());
            return Presets::default();
        }
    };
    match parse(&text) {
        Ok(p) => p,
        Err(e) => {
            let bad = path.with_extension("toml.bad");
            warn!("N.RADIO presets: {} does not parse ({e}); moved to {}", path.display(), bad.display());
            let _ = fs::rename(path, bad);
            Presets::default()
        }
    }
}

/// Save atomically: after a power cut the file is the old one or the new one.
pub fn save(path: &Path, presets: &Presets) -> io::Result<()> {
    let text = toml::to_string(presets).map_err(io::Error::other)?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(b"# N.RADIO presets: Beo4 / link channel number -> Music Assistant station.\n")?;
        f.write_all(b"# An entry with a name and no uri is looked up in Music Assistant by name.\n\n")?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Preset;

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(FILE_NAME);
        assert!(load(&path).is_empty());
        let mut p = Presets::default();
        p.set(Preset::new(8, "Deutschlandfunk", "library://radio/3"));
        p.last = 8;
        save(&path, &p).unwrap();
        assert_eq!(load(&path), p);
    }

    #[test]
    fn a_broken_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, "[[preset]\nnumber = ").unwrap();
        assert!(load(&path).is_empty());
        assert!(!path.exists());
        assert!(dir.path().join("radio-presets.toml.bad").exists());
    }
}
