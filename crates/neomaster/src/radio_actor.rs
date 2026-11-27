//! The N.RADIO actor: plays the presets the core tunes to through Music Assistant, and saves the
//! preset file. The core stays sans-IO; it sends [`RadioCmd`]s here and hears back through
//! [`Event::RadioResolved`] when a preset's name has been matched to an MA station.
//!
//! Configuration (until the settings file lands): `MA_URL`, `MA_TOKEN` and `NEOMASTER_MA_PLAYER`
//! (the BM5's player / queue id in MA), the names the settings layer reads too. Without them
//! N.RADIO tunes and logs but plays nothing.

use std::path::PathBuf;
use std::time::Duration;

use log::{info, warn};
use radio::Preset;
use tokio::sync::mpsc;

use crate::core::Event;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RadioCmd {
    /// Play this preset (resolve its name first if it has no URI).
    Play(Preset),
    /// Stop the BM5's MA queue: N.RADIO is no longer the source.
    Stop,
    /// Save the preset list (the last-played number or a resolved URI changed).
    Save(radio::Presets),
}

/// Where the core sends N.RADIO commands.
pub trait RadioOut: Send {
    fn send(&mut self, cmd: RadioCmd);
}

impl RadioOut for mpsc::UnboundedSender<RadioCmd> {
    fn send(&mut self, cmd: RadioCmd) {
        let _ = mpsc::UnboundedSender::send(self, cmd);
    }
}

/// No actor (tests, or a run without Music Assistant): commands are dropped.
pub struct NoRadio;
impl RadioOut for NoRadio {
    fn send(&mut self, _: RadioCmd) {}
}

/// Where Music Assistant is and which queue is the BM5's.
#[derive(Clone)]
pub struct MaConfig {
    pub url: String,
    pub token: String,
    pub queue_id: String,
}

impl MaConfig {
    /// From `MA_URL`, `MA_TOKEN` and `NEOMASTER_MA_PLAYER`; `None` (with the reason) if one is
    /// missing.
    pub fn from_env() -> Result<Self, String> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty()).ok_or(k.to_string());
        let missing: Vec<String> = ["MA_URL", "MA_TOKEN", "NEOMASTER_MA_PLAYER"].iter().filter_map(|k| get(k).err()).collect();
        if !missing.is_empty() {
            return Err(format!("{} not set", missing.join(", ")));
        }
        Ok(MaConfig { url: get("MA_URL")?, token: get("MA_TOKEN")?, queue_id: get("NEOMASTER_MA_PLAYER")? })
    }
}

/// How long a command waits for the MA connection (a wake-up timer can fire right after boot).
const CONNECT_WAIT: Duration = Duration::from_secs(20);

/// Run the actor until the core drops its sender.
pub async fn run(
    ma: Option<MaConfig>,
    presets_path: PathBuf,
    mut cmds: mpsc::UnboundedReceiver<RadioCmd>,
    events: mpsc::UnboundedSender<Event>,
) {
    let client = ma.as_ref().map(|c| ma_client::Client::connect(ma_client::Config::new(&c.url, &c.token)).0);
    let queue = ma.map(|c| c.queue_id).unwrap_or_default();
    while let Some(first) = cmds.recv().await {
        // Several presses in a row (▲ ▲ ▲): only the newest station is worth starting. Saves
        // are done as they come.
        let mut cmd = None;
        for c in std::iter::once(first).chain(std::iter::from_fn(|| cmds.try_recv().ok())) {
            match c {
                RadioCmd::Save(p) => save(&presets_path, &p),
                c => cmd = Some(c),
            }
        }
        let Some(cmd) = cmd else { continue };
        match cmd {
            RadioCmd::Save(_) => {}
            RadioCmd::Stop => {
                let Some(client) = &client else { continue };
                if connected(client).await {
                    if let Err(e) = client.stop(&queue).await {
                        warn!("N.RADIO: stopping the Music Assistant queue: {e}");
                    }
                }
            }
            RadioCmd::Play(mut preset) => {
                let Some(client) = &client else {
                    info!(
                        "N.RADIO: would play preset {} ({}), but Music Assistant is not configured",
                        preset.number,
                        preset.label()
                    );
                    continue;
                };
                if !connected(client).await {
                    warn!("N.RADIO: Music Assistant unreachable; preset {} not played", preset.number);
                    continue;
                }
                if preset.uri.is_empty() {
                    match radio::ma::resolve(client, &preset.name).await {
                        Ok(r) => {
                            info!("N.RADIO: preset {} {:?} is {} ({})", preset.number, preset.name, r.name, r.uri);
                            preset.uri = r.uri.clone();
                            let _ = events.send(Event::RadioResolved { number: preset.number, uri: r.uri });
                        }
                        Err(e) => {
                            warn!("N.RADIO: preset {}: {e}", preset.number);
                            continue;
                        }
                    }
                }
                match radio::ma::play(client, &queue, &preset).await {
                    Ok(()) => info!("N.RADIO: playing preset {} ({}) on {queue}", preset.number, preset.uri),
                    Err(e) => warn!("N.RADIO: playing preset {} ({}): {e}", preset.number, preset.uri),
                }
            }
        }
    }
}

async fn connected(client: &ma_client::Client) -> bool {
    tokio::time::timeout(CONNECT_WAIT, client.wait_connected()).await.is_ok()
}

fn save(path: &std::path::Path, presets: &radio::Presets) {
    if let Err(e) = radio::store::save(path, presets) {
        warn!("N.RADIO: saving {}: {e}", path.display());
    }
}

/// Load the presets and start the actor: returns the tuner and the core's command sender.
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    events: mpsc::UnboundedSender<Event>,
) -> (radio::Tuner, mpsc::UnboundedSender<RadioCmd>) {
    let path = radio::store::default_path();
    let presets = radio::store::load(&path);
    info!("N.RADIO: {} preset(s) from {}", presets.presets.len(), path.display());
    let ma = match MaConfig::from_env() {
        Ok(c) => Some(c),
        Err(why) => {
            info!("N.RADIO: Music Assistant not configured ({why}); presets tune but do not play");
            None
        }
    };
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(run(ma, path, rx, events));
    (radio::Tuner::new(presets), tx)
}
