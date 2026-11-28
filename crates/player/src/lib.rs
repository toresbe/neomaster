//! Makes the BeoMaster 5 a Music Assistant playback device.
//!
//! The BM5 joins MA as a [Sendspin](https://github.com/Sendspin/spec)
//! player, MA's own clock-synchronised streaming protocol (the `sendspin`
//! provider is enabled on the home server). This crate runs the client as
//! an actor inside neomaster: it connects to MA's Sendspin endpoint
//! (`ws://<ma>:8927/sendspin`), reconnects on its own, plays the stream
//! through ALSA, and reports stream start and end so the core can switch
//! the PC2's routing and speakers.
//!
//! MA wraps every Sendspin client in a "universal" player whose id (such
//! as `up6e0de696`) is the queue id for `play_media`; find it with
//! `ma_client::Client::player_for_output(client_id)`.
//!
//! Volume stays MA-side at 100 %: the real volume is the PC2's (audio
//! master) or the room master's (link source), so MA volume commands are
//! accepted but only scale the stream digitally.
//!
//! Measured on the home server against MA 2.10.13 with
//! `examples/latency_probe.rs` (silent output, LAN WAV source): with a
//! 200 ms lead, sound is scheduled ~290 ms after `play_media`; after
//! `next`, ~1.26 s, of which 1.00 s is a fixed wait in MA's `next` and
//! `previous` (neomaster skips with `play_index` instead). On the BM5 the
//! ALSA ring was ~1.5 s with the device's default period, hence
//! [`PlayerConfig::buffer_frames`].

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use sendspin::audio::decode::{Decoder, StreamDecoder};
use sendspin::audio::{AudioBuffer, AudioFormat, CommitRejected, PlayerEvent, PlayerSession, SyncedPlayer, SyncedPlayerConfig};
use sendspin::protocol::messages::{AudioFormatSpec, PlayerState, PlayerStateCommand, PlayerV1Support};
use sendspin::sync::ClockSync;
use sendspin::ProtocolClientBuilder;
use tokio::sync::{mpsc, watch};

/// Default ALSA period, see [`PlayerConfig::buffer_frames`].
pub const DEFAULT_BUFFER_FRAMES: u32 = 2048;

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    /// MA's Sendspin endpoint, e.g. `ws://192.168.2.238:8927/sendspin`.
    pub server_url: String,
    /// Stable id; MA keys the player (and its queue and settings) on it.
    pub client_id: String,
    /// Name shown in MA.
    pub name: String,
    /// ALSA output device name as cpal lists it (e.g. `default`,
    /// `hw:CARD=...`); `None` for the system default.
    pub device: Option<String>,
    /// How far ahead MA schedules audio. Lower starts sooner but leaves
    /// less margin for a busy C7; 200 ms worked on the LAN.
    pub lead_ms: u32,
    /// ALSA period in frames (the ring buffer is two periods). `None` lets
    /// the device choose, which on the BM5's VT82xx is 32768 frames, a
    /// ~1.5 s ring: everything already in it still plays after a skip.
    /// 2048 frames is ~93 ms of ring at 44.1 kHz, plenty on a wired LAN.
    /// If the device refuses the size, the player falls back to its default.
    pub buffer_frames: Option<u32>,
    /// Formats offered to MA, most preferred first. PCM only by default:
    /// no decoding on the C7, ~1.4 Mbit/s on the LAN.
    pub formats: Vec<AudioFormatSpec>,
}

impl PlayerConfig {
    pub fn new(server_url: impl Into<String>, client_id: impl Into<String>, name: impl Into<String>) -> Self {
        let pcm = |rate| AudioFormatSpec { codec: "pcm".into(), channels: 2, sample_rate: rate, bit_depth: 16 };
        PlayerConfig {
            server_url: server_url.into(),
            client_id: client_id.into(),
            name: name.into(),
            device: None,
            lead_ms: 200,
            buffer_frames: Some(DEFAULT_BUFFER_FRAMES),
            formats: vec![pcm(44_100), pcm(48_000)],
        }
    }
}

/// What the player reports to the core.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Connected,
    Disconnected(String),
    /// MA started a stream: switch the PC2 to this source, speakers on.
    StreamStarted {
        sample_rate: u32,
        channels: u8,
        bit_depth: u8,
    },
    /// The stream ended (stop, pause, or queue end).
    StreamEnded,
    /// The audio device failed; the player retries on the next stream.
    OutputError(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Idle,
    Playing,
}

/// Handle to the running player actor.
pub struct Player {
    state: watch::Receiver<State>,
    task: tokio::task::JoinHandle<()>,
}

impl Player {
    /// Start the player. Must be called inside a tokio runtime.
    pub fn spawn(cfg: PlayerConfig) -> (Player, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (st_tx, st_rx) = watch::channel(State::Disconnected);
        let task = tokio::spawn(supervise(cfg, tx, st_tx));
        (Player { state: st_rx, task }, rx)
    }

    pub fn state(&self) -> State {
        *self.state.borrow()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn supervise(cfg: PlayerConfig, tx: mpsc::UnboundedSender<Event>, st: watch::Sender<State>) {
    let mut backoff = Duration::from_millis(500);
    loop {
        let started = tokio::time::Instant::now();
        let r = session(&cfg, &tx, &st).await;
        let _ = st.send(State::Disconnected);
        let why = match r {
            Ok(()) => "closed by server".to_string(),
            Err(e) => e,
        };
        log::warn!("sendspin: {why}");
        let _ = tx.send(Event::Disconnected(why));
        if tx.is_closed() {
            return;
        }
        if started.elapsed() > Duration::from_secs(30) {
            backoff = Duration::from_millis(500);
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

fn find_device(name: &str) -> Result<cpal::Device, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut seen = Vec::new();
    for d in host.output_devices().map_err(|e| e.to_string())? {
        let id = d.id().map(|i| i.to_string()).unwrap_or_default();
        let desc = d.description().map(|n| n.name().to_owned()).unwrap_or_default();
        // cpal ids look like "alsa:hw:CARD=X,DEV=0"; accept the bare ALSA name too.
        if id == name || desc == name || id.split_once(':').is_some_and(|(_, alsa)| alsa == name) {
            return Ok(d);
        }
        seen.push(id);
    }
    Err(format!("no output device {name:?}; have {}", seen.join(", ")))
}

fn open_output(
    cfg: &PlayerConfig,
    format: &AudioFormat,
    clock: &Arc<Mutex<ClockSync>>,
    session: &mut PlayerSession,
    output: &mut Option<SyncedPlayer>,
) -> Result<(), String> {
    let _ = session.release_output();
    drop(output.take());
    let new_config = || -> Result<SyncedPlayerConfig, String> {
        let mut pc = SyncedPlayerConfig::new();
        if let Some(name) = &cfg.device {
            pc.device = Some(find_device(name)?);
        }
        Ok(pc)
    };
    let mut pc = new_config()?;
    pc.buffer_size = cfg.buffer_frames;
    let player = match SyncedPlayer::new(format.clone(), clock.clone(), pc) {
        Ok(p) => p,
        Err(e) if cfg.buffer_frames.is_some() => {
            log::warn!("sendspin: a {}-frame ALSA period failed ({e}); using the device default", cfg.buffer_frames.unwrap_or(0));
            SyncedPlayer::new(format.clone(), clock.clone(), new_config()?).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    session.begin_output_install().and_then(|permit| permit.install(player.control())).map_err(|e| e.to_string())?;
    *output = Some(player);
    Ok(())
}

async fn session(cfg: &PlayerConfig, tx: &mpsc::UnboundedSender<Event>, st: &watch::Sender<State>) -> Result<(), String> {
    let client = ProtocolClientBuilder::builder()
        .client_id(cfg.client_id.clone())
        .name(cfg.name.clone())
        .player_v1_support(PlayerV1Support {
            supported_formats: cfg.formats.clone(),
            buffer_capacity: 8 * 1024 * 1024,
            supported_commands: vec!["volume".into(), "mute".into()],
        })
        .initial_player_state(PlayerState {
            volume: Some(100),
            muted: Some(false),
            static_delay_ms: Some(0),
            required_lead_time_ms: Some(cfg.lead_ms),
            min_buffer_ms: Some(cfg.lead_ms),
            supported_commands: Some(vec![PlayerStateCommand::SetStaticDelay]),
        })
        .build()
        .connect(&cfg.server_url)
        .await
        .map_err(|e| format!("connect {}: {e}", cfg.server_url))?;
    let conn = client.split();
    let clock = conn.clock_sync.clone();
    let (_artwork, _visualizer, _messages, _guard) = (conn.artwork, conn.visualizer, conn.messages, conn.guard);
    let mut session: PlayerSession = conn.player.attach_empty();
    let mut output: Option<SyncedPlayer> = None;
    let mut decoder: Option<StreamDecoder> = None;
    log::info!("sendspin: connected to {} as {:?}", cfg.server_url, cfg.name);
    let _ = st.send(State::Idle);
    let _ = tx.send(Event::Connected);

    while let Some(ev) = session.recv().await {
        match ev {
            PlayerEvent::Start(config) => {
                let d = StreamDecoder::new(&config).map_err(|e| e.to_string())?;
                let f = d.format().clone();
                let reusable =
                    output.as_ref().is_some_and(|p| p.format().sample_rate == f.sample_rate && p.format().channels == f.channels);
                if !reusable {
                    if let Err(e) = open_output(cfg, &f, &clock, &mut session, &mut output) {
                        log::warn!("sendspin: audio output: {e}");
                        let _ = tx.send(Event::OutputError(e));
                    }
                }
                let _ = st.send(State::Playing);
                let _ =
                    tx.send(Event::StreamStarted { sample_rate: f.sample_rate, channels: f.channels, bit_depth: f.bit_depth });
                decoder = Some(d);
            }
            PlayerEvent::Audio(work) => {
                let Some(d) = decoder.as_ref() else { continue };
                match d.decode(work.data()) {
                    Ok(samples) => {
                        let buffer = AudioBuffer { timestamp: work.timestamp(), samples, format: d.format().clone() };
                        if let Err(CommitRejected::Output(e)) = work.commit(buffer) {
                            log::warn!("sendspin: output rejected a buffer: {e}");
                        }
                    }
                    Err(e) => log::warn!("sendspin: undecodable audio: {e}"),
                }
            }
            PlayerEvent::OutputLost(e) => {
                log::warn!("sendspin: output lost: {e}");
                let _ = tx.send(Event::OutputError(e.to_string()));
                drop(output.take());
                if let Some(d) = decoder.as_ref() {
                    let f = d.format().clone();
                    if let Err(e) = open_output(cfg, &f, &clock, &mut session, &mut output) {
                        let _ = tx.send(Event::OutputError(e));
                    }
                }
            }
            PlayerEvent::Clear => {}
            PlayerEvent::End => {
                let _ = session.release_output();
                drop(output.take());
                decoder = None;
                let _ = st.send(State::Idle);
                let _ = tx.send(Event::StreamEnded);
            }
        }
    }
    session.shutdown().await;
    Ok(())
}
