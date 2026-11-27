//! The settings at run time: loading them at start (file, then environment, then the command
//! line), saving the panel's changes off the core task, the machine facts the SETTINGS menu's
//! info sections show, and the log level switch.

use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;

use log::{info, warn};
use settings::env::Overrides;
use settings::menu::Facts;
use settings::store::Store;
use settings::Settings;

/// Where the core sends changed settings to be saved.
pub trait SettingsOut: Send {
    fn save(&mut self, settings: &Settings);
}

/// Saves on its own thread so the core never waits for the disk. Values the environment or the
/// command line put over the file are not saved unless the panel changed them.
pub struct Persister {
    tx: std_mpsc::Sender<Settings>,
}

impl Persister {
    pub fn spawn(mut store: Store, overrides: Overrides, at_start: Settings) -> Persister {
        let (tx, rx) = std_mpsc::channel::<Settings>();
        let spawned = std::thread::Builder::new().name("settings".into()).spawn(move || {
            while let Ok(mut s) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    s = newer;
                }
                let s = overrides.for_saving(&s, &at_start);
                match store.save(&s) {
                    Ok(true) => info!("settings saved to {}", store.path.display()),
                    Ok(false) if !store.writable() => warn!("settings not saved: {} is not ours to write", store.path.display()),
                    Ok(false) => {}
                    Err(e) => warn!("settings not saved to {}: {e}", store.path.display()),
                }
            }
        });
        if let Err(e) = spawned {
            warn!("no settings thread ({e}); changes will not be saved");
        }
        Persister { tx }
    }
}

impl SettingsOut for Persister {
    fn save(&mut self, settings: &Settings) {
        let _ = self.tx.send(settings.clone());
    }
}

/// The settings for this run: the file at `path`, the environment over it, then `--role`.
pub fn load(path: PathBuf, role: Option<bool>) -> (Settings, Persister) {
    let (store, mut s) = Store::open(path);
    let mut overrides = Overrides::from_env(&mut s, |k| std::env::var(k).ok());
    if let Some(am) = role {
        overrides.role(&mut s, am);
    }
    if !overrides.applied.is_empty() {
        info!("settings overridden for this run by {}", overrides.applied.join(", "));
    }
    set_debug_logging(s.system.debug_logging);
    let persister = Persister::spawn(store, overrides, s.clone());
    (s, persister)
}

/// DEBUG LOGGING. Without `RUST_LOG` the logger is built at debug level and capped here; with
/// `RUST_LOG` that wins and this does nothing.
pub fn set_debug_logging(on: bool) {
    if std::env::var_os("RUST_LOG").is_some() {
        return;
    }
    log::set_max_level(if on { log::LevelFilter::Debug } else { log::LevelFilter::Info });
}

/// The A.AUX loop's configuration from the settings.
pub fn loopback_config(s: &settings::Aux) -> a_aux::loopback::LoopbackConfig {
    a_aux::loopback::LoopbackConfig {
        playback: s.playback.clone(),
        latency_us: s.latency_us,
        ..a_aux::loopback::LoopbackConfig::default()
    }
}

fn read_trimmed(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn uptime() -> Option<String> {
    let secs: f64 = read_trimmed("/proc/uptime")?.split_whitespace().next()?.parse().ok()?;
    let m = (secs / 60.0) as u64;
    Some(format!("{}d {}h {}m", m / 1440, m / 60 % 24, m % 60))
}

fn available_mb(path: &str) -> Option<u64> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: statvfs only writes the struct we own.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64 / 1_000_000)
}

/// The address the host would use to reach the internet. Connecting a UDP socket sends nothing.
fn ip_address() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

fn mac_address() -> Option<String> {
    let mut ifs: Vec<_> = std::fs::read_dir("/sys/class/net").ok()?.flatten().map(|e| e.path()).collect();
    ifs.sort();
    ifs.iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "lo"))
        .filter(|p| read_trimmed(p.join("operstate")).as_deref() == Some("up"))
        .find_map(|p| read_trimmed(p.join("address")))
        .map(|a| a.to_uppercase().replace(':', "-"))
}

/// The B&O USB CD drive (USB 13fd:0940, from MLConfig.dll's check).
fn cd_drive() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/bus/usb/devices") else { return false };
    dir.flatten().any(|e| {
        read_trimmed(e.path().join("idVendor")).as_deref() == Some("13fd")
            && read_trimmed(e.path().join("idProduct")).as_deref() == Some("0940")
    })
}

fn os_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    text.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string())
}

fn temp_c(path: &str) -> Option<String> {
    let milli: i64 = read_trimmed(path)?.parse().ok()?;
    Some(format!("{} °C", milli / 1000))
}

/// Gather the facts (reads /proc and /sys; run it off the core task).
pub fn gather_facts(s: &Settings) -> Facts {
    let na = || "NOT AVAILABLE".to_string();
    let t = crate::link::local_time();
    let mut system_info = Vec::new();
    if let Some(c) = temp_c("/sys/class/thermal/thermal_zone0/temp") {
        system_info.push(("CPU TEMP".into(), c));
    }
    system_info.push(("BM5 UPTIME".into(), uptime().unwrap_or_else(na)));
    system_info.push(("AVAILABLE STORAGE".into(), available_mb("/").map(|m| format!("{m} MB")).unwrap_or_else(na)));
    let cd = cd_drive();
    let aux = a_aux::device::scan_for(Some(s.aux.card.as_str()).filter(|c| !c.is_empty()));
    let product_info = vec![
        ("NEOMASTER".into(), env!("CARGO_PKG_VERSION").into()),
        ("OS".into(), os_name().unwrap_or_else(na)),
        ("KERNEL".into(), read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_else(na)),
        ("SYSTEM CONFIGURATION".into(), s.roles.system_configuration().to_string()),
        ("A.AUX INPUT".into(), aux.map(|c| format!("{} [{}]", c.name, c.id)).unwrap_or_else(|| "NOT DETECTED".into())),
        ("CD DRIVE".into(), if cd { "DETECTED".into() } else { "NOT DETECTED".into() }),
    ];
    let network_info = vec![
        ("IP ADDRESS".into(), ip_address().unwrap_or_else(na)),
        ("MAC ADDRESS".into(), mac_address().unwrap_or_else(na)),
        ("PLAYER ID".into(), if s.music_assistant.player_id.is_empty() { na() } else { s.music_assistant.player_id.clone() }),
    ];
    Facts {
        system_info,
        product_info,
        network_info,
        cd_drive: cd,
        time: format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second),
        host_name: read_trimmed("/proc/sys/kernel/hostname").unwrap_or_default(),
    }
}
