//! Runs shairport-sync for neomaster and reads its metadata pipe.
//!
//! [`Receiver::start`] writes a configuration file and the metadata FIFO into a runtime
//! directory, starts shairport-sync on it, and keeps it running: a supervisor thread restarts it
//! when it exits (at once when [`Receiver::drop_session`] asked for it, with a back-off when it
//! died). A reader thread parses the pipe and passes each [`Item`] on.
//!
//! Running it ourselves, rather than as a system service, is what lets neomaster end a session
//! when another source is picked: AirPlay 1 has no "stop" a receiver can send, short of the
//! DACP remote-control back channel (not every sender offers one), so the receiver is restarted
//! and the sender sees the connection drop.

use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use log::{debug, info, warn};

use crate::metadata::{Item, Parser};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverConfig {
    /// The program, normally "shairport-sync" (anything that takes `-c <file>` will do, e.g. a
    /// wrapper that runs it in a container).
    pub program: String,
    /// The name senders show.
    pub name: String,
    /// shairport-sync's output backend: "alsa" on the BM5; "dummy" to test without sound.
    pub backend: String,
    /// The ALSA device to play to (the output the PC2 hears).
    pub device: String,
    /// The RTSP port (AirPlay 1's is 5000).
    pub port: u16,
    /// Where the configuration file and the metadata pipe go.
    pub dir: PathBuf,
}

impl Default for ReceiverConfig {
    fn default() -> Self {
        ReceiverConfig {
            program: "shairport-sync".into(),
            name: "BeoMaster 5".into(),
            backend: "alsa".into(),
            device: "default".into(),
            port: 5000,
            dir: default_dir(),
        }
    }
}

/// `$XDG_RUNTIME_DIR/neomaster`, else `/tmp/neomaster-<uid>`.
pub fn default_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("neomaster"),
        _ => PathBuf::from(format!("/tmp/neomaster-{}", unsafe { libc::getuid() })),
    }
}

impl ReceiverConfig {
    /// The defaults with the environment applied. `None` if `NEOMASTER_AIRPLAY` is 0, off or
    /// no.
    ///
    /// | Variable | Default |
    /// |---|---|
    /// | `NEOMASTER_AIRPLAY_COMMAND` | `shairport-sync` |
    /// | `NEOMASTER_AIRPLAY_NAME` | `BeoMaster 5` |
    /// | `NEOMASTER_AIRPLAY_BACKEND` | `alsa` |
    /// | `NEOMASTER_AIRPLAY_DEVICE` | `default` (neomaster passes A.AUX's playback device) |
    /// | `NEOMASTER_AIRPLAY_PORT` | 5000 |
    /// | `NEOMASTER_AIRPLAY_DIR` | `$XDG_RUNTIME_DIR/neomaster` |
    pub fn from_env() -> Option<Self> {
        Self::from_env_with_device("default")
    }

    /// [`ReceiverConfig::from_env`], with `device` as the output unless
    /// `NEOMASTER_AIRPLAY_DEVICE` says otherwise.
    pub fn from_env_with_device(device: &str) -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        if var("NEOMASTER_AIRPLAY").is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "0" | "off" | "no" | "false")) {
            return None;
        }
        let mut c = Self { device: device.into(), ..Self::default() };
        if let Some(v) = var("NEOMASTER_AIRPLAY_COMMAND") {
            c.program = v;
        }
        if let Some(v) = var("NEOMASTER_AIRPLAY_NAME") {
            c.name = v;
        }
        if let Some(v) = var("NEOMASTER_AIRPLAY_BACKEND") {
            c.backend = v;
        }
        if let Some(v) = var("NEOMASTER_AIRPLAY_DEVICE") {
            c.device = v;
        }
        if let Some(v) = var("NEOMASTER_AIRPLAY_PORT").and_then(|p| p.parse().ok()) {
            c.port = v;
        }
        if let Some(v) = var("NEOMASTER_AIRPLAY_DIR") {
            c.dir = v.into();
        }
        Some(c)
    }

    pub fn conf_path(&self) -> PathBuf {
        self.dir.join("shairport-sync.conf")
    }

    pub fn pipe_path(&self) -> PathBuf {
        self.dir.join("shairport-sync-metadata")
    }

    /// shairport-sync's configuration (libconfig syntax; options as in 3.3.x and 4.x).
    pub fn conf_text(&self) -> String {
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            r#"// Written by neomaster; changes are overwritten at start-up.
general = {{
  name = "{name}";
  output_backend = "{backend}";
  port = {port};
  // The volume is the PC2's (audio master) or the room master's (link source): play at full
  // scale and leave the sender's volume slider out of it.
  ignore_volume_control = "yes";
}};
alsa = {{
  output_device = "{device}";
}};
metadata = {{
  enabled = "yes";
  include_cover_art = "yes";
  pipe_name = "{pipe}";
  pipe_timeout = 5000;
}};
sessioncontrol = {{
  // A second device may take over from the first, as on Apple's own receivers.
  allow_session_interruption = "yes";
  session_timeout = 20;
}};
"#,
            name = q(&self.name),
            backend = q(&self.backend),
            port = self.port,
            device = q(&self.device),
            pipe = q(&self.pipe_path().to_string_lossy()),
        )
    }
}

/// What the receiver reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// An item from the metadata pipe.
    Item(Item),
    /// shairport-sync exited and is being started again: any session is gone.
    Restarted,
}

struct Shared {
    child: Mutex<Option<Child>>,
    /// When SIGTERM went to the child; SIGKILL follows after [`TERM_TIMEOUT`].
    terminated_at: Mutex<Option<Instant>>,
    drop_requested: AtomicBool,
    shutdown: AtomicBool,
}

/// shairport-sync, kept running; see the module docs.
pub struct Receiver {
    shared: Arc<Shared>,
    config: ReceiverConfig,
}

/// Restart back-off after shairport-sync died by itself.
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// How long shairport-sync gets to exit on SIGTERM before it is killed.
const TERM_TIMEOUT: Duration = Duration::from_secs(3);

impl Receiver {
    /// Write the configuration and the pipe, start shairport-sync, and pass what it reports to
    /// `report` (from the receiver's own threads).
    pub fn start(config: ReceiverConfig, report: impl Fn(Report) + Send + Sync + 'static) -> io::Result<Receiver> {
        fs::create_dir_all(&config.dir)?;
        fs::write(config.conf_path(), config.conf_text())?;
        make_fifo(&config.pipe_path())?;
        let report = Arc::new(report);

        // O_RDWR: never blocks waiting for a writer and never sees EOF when shairport-sync
        // restarts (Linux allows it on a FIFO), so the writer always finds a reader.
        let pipe = fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open(config.pipe_path())?;
        let r = report.clone();
        thread::Builder::new().name("airplay-pipe".into()).spawn(move || read_pipe(pipe, &*r))?;

        let shared = Arc::new(Shared {
            child: Mutex::new(None),
            terminated_at: Mutex::new(None),
            drop_requested: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        });
        let (s, c) = (shared.clone(), config.clone());
        thread::Builder::new().name("airplay-supervisor".into()).spawn(move || supervise(&c, &s, &*report))?;
        info!("AirPlay: receiving as \"{}\" ({} on port {})", config.name, config.program, config.port);
        Ok(Receiver { shared, config })
    }

    pub fn config(&self) -> &ReceiverConfig {
        &self.config
    }

    /// End the current session: restart shairport-sync. A [`Report::Restarted`] follows.
    pub fn drop_session(&self) {
        self.shared.drop_requested.store(true, Ordering::SeqCst);
        if let Some(child) = self.shared.child.lock().unwrap().as_ref() {
            info!("AirPlay: ending the session");
            terminate(child);
            *self.shared.terminated_at.lock().unwrap() = Some(Instant::now());
        }
    }

    pub fn apply(&self, effect: crate::Effect) {
        match effect {
            crate::Effect::Drop => self.drop_session(),
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        if let Some(child) = self.shared.child.lock().unwrap().as_ref() {
            terminate(child);
        }
    }
}

fn terminate(child: &Child) {
    // SIGTERM lets shairport-sync withdraw its mDNS record, which a kill would not.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
}

fn make_fifo(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(m) if m.file_type().is_fifo() => return Ok(()),
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut c = path.as_os_str().as_bytes().to_vec();
    c.push(0);
    if unsafe { libc::mkfifo(c.as_ptr() as *const libc::c_char, 0o600) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_pipe(mut pipe: fs::File, report: &dyn Fn(Report)) {
    let mut parser = Parser::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => thread::sleep(Duration::from_millis(100)),
            Ok(n) => {
                let (items, bad) = parser.feed(&buf[..n]);
                for b in bad {
                    debug!("AirPlay: skipped unreadable metadata: {}", b.0);
                }
                for item in items {
                    report(Report::Item(item));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                warn!("AirPlay: metadata pipe: {e}");
                return;
            }
        }
    }
}

fn supervise(config: &ReceiverConfig, shared: &Shared, report: &dyn Fn(Report)) {
    let mut backoff = BACKOFF_MIN;
    let mut failed_to_start = false;
    while !shared.shutdown.load(Ordering::SeqCst) {
        let started = Instant::now();
        let mut cmd = Command::new(&config.program);
        cmd.arg("-c").arg(config.conf_path()).stdin(Stdio::null()).stdout(Stdio::null());
        // If neomaster dies without cleaning up, shairport-sync goes with it (this thread lives
        // as long as the process, which is what PR_SET_PDEATHSIG watches).
        // SAFETY: prctl is async-signal-safe and touches no memory of ours.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let spawned = cmd.spawn();
        let child = match spawned {
            Ok(c) => c,
            Err(e) => {
                if !failed_to_start {
                    warn!("AirPlay: could not start {}: {e} (AirPlay is off until it can)", config.program);
                    failed_to_start = true;
                }
                thread::sleep(BACKOFF_MAX);
                continue;
            }
        };
        failed_to_start = false;
        let pid = child.id();
        *shared.child.lock().unwrap() = Some(child);
        // Poll rather than wait(): wait() would hold the lock drop_session needs.
        let status = loop {
            let mut guard = shared.child.lock().unwrap();
            match guard.as_mut().map(|c| c.try_wait()) {
                Some(Ok(Some(status))) => {
                    *guard = None;
                    break Some(status);
                }
                Some(Ok(None)) => {
                    let late = shared.terminated_at.lock().unwrap().is_some_and(|t| t.elapsed() > TERM_TIMEOUT);
                    if late {
                        warn!("AirPlay: {} ignored SIGTERM; killing it", config.program);
                        let _ = guard.as_mut().map(|c| c.kill());
                        *shared.terminated_at.lock().unwrap() = None;
                    }
                }
                Some(Err(e)) => {
                    warn!("AirPlay: {e}");
                    *guard = None;
                    break None;
                }
                None => break None,
            }
            drop(guard);
            thread::sleep(Duration::from_millis(100));
        };
        *shared.terminated_at.lock().unwrap() = None;
        if shared.shutdown.load(Ordering::SeqCst) {
            return;
        }
        report(Report::Restarted);
        if shared.drop_requested.swap(false, Ordering::SeqCst) {
            debug!("AirPlay: {} (pid {pid}) stopped to end a session; starting it again", config.program);
            backoff = BACKOFF_MIN;
            continue;
        }
        if started.elapsed() > BACKOFF_MAX {
            backoff = BACKOFF_MIN;
        }
        warn!(
            "AirPlay: {} exited ({}); starting it again in {} s",
            config.program,
            status.map(|s| s.to_string()).unwrap_or_else(|| "unknown status".into()),
            backoff.as_secs()
        );
        thread::sleep(backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_quotes_the_name_and_points_at_the_pipe() {
        let c = ReceiverConfig { name: "Tore's \"BM5\"".into(), dir: "/run/x".into(), ..Default::default() };
        let t = c.conf_text();
        assert!(t.contains(r#"name = "Tore's \"BM5\"";"#), "{t}");
        assert!(t.contains(r#"pipe_name = "/run/x/shairport-sync-metadata";"#));
        assert!(t.contains(r#"output_backend = "alsa";"#));
        assert!(t.contains(r#"ignore_volume_control = "yes";"#));
    }
}
