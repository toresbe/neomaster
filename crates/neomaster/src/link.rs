//! The Masterlink role glue: what the panel, the PC2's IR eye and the link rooms ask for, turned
//! into PC2 messages and player actions, depending on how the BM5 sits on the link.
//!
//! [`Link`] is sans-IO like the state machines it drives: the core feeds it UI intents and the
//! PC2 actor's [`pc2::Event`]s with `now`, and gets back [`Outputs`]: messages for the PC2 actor
//! and [`AppEffect`]s for the player. It owns the [`AudioMaster`] or [`SourceCenter`] and the PC2
//! [`Mixer`] state.
//!
//! The role table (architecture sketch, "PC2, Masterlink and the role model"):
//!
//! | Intent            | Audio master                                  | Link source                                      |
//! |-------------------|-----------------------------------------------|--------------------------------------------------|
//! | Volume wheel      | PC2 `eb` absolute volume                      | Beo4 VOL UP / DOWN to the room's master, 1 per 4 |
//! | Pick a source     | play it, speakers on, `set_local_source`      | its Beo4 key to the room's master; play when the master connects us |
//! | Short standby     | speakers and routing off, `local_standby`     | Beo4 STANDBY to the room's master                |
//! | Long standby      | `all_standby` (lock, STANDBY_1 to ALL)        | the same, from PC_1 ([`SourceCenter::all_standby`]) |
//! | Local IR key      | act on it (held STANDBY = long standby)       | log only: the room's master has its own eye       |
//! | Link room keys    | to the player                                 | to the player                                    |

use std::time::Duration;

use log::{debug, info, trace, warn};
use masterlink::protocol::clock::LocalTime;
use masterlink::protocol::{node, source as ml_source};
use masterlink::{
    AudioMaster, Effect as MlEffect, Effects, Host, LinkRole, LocalSource, LockMode, SourceCenter, SourceMap, Telegram, TrackInfo,
};
use pc2::{beo4, Mixer};

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = Duration;

/// What the PC2 actor is asked to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pc2Cmd {
    /// A PC2 message, without the USB framing (mixer, bus lock).
    Message(pc2::Message),
    /// An encoded Masterlink telegram (checksum and EOT included); the actor paces it.
    Telegram(Vec<u8>),
}

/// What the player side of neomaster is asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEffect {
    /// Play this local source (Music Assistant for N.MUSIC / N.RADIO, the USB card for A.AUX).
    Play(LocalSource),
    /// Stop playing: nobody, local or on the link, listens any more.
    Stop,
    /// A Beo4 key for the playing source, from a link room (SOURCE_CONTROL) or the local remote.
    SourceKey { source: LocalSource, key: u8 },
    /// A link room asked for the playing source at this channel / track number.
    SourceChannel { source: LocalSource, channel: u8 },
    /// The room or the whole system went to standby: blank the panel.
    Standby,
    /// The volume as the PC2 now has it (audio master only).
    Volume(u8),
}

/// [`Link`]'s state as the debug stream reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub started: bool,
    pub playing: Option<LocalSource>,
    pub selected: Option<LocalSource>,
    pub speakers: bool,
    pub volume: u8,
    /// Audio master only.
    pub bus: Option<masterlink::BusState>,
    pub holds_bus_lock: bool,
    /// Link source only: our source is on the link.
    pub distributing: bool,
    pub current_source: u8,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outputs {
    pub pc2: Vec<Pc2Cmd>,
    pub app: Vec<AppEffect>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub role: LinkRole,
    /// B&O's SystemConfiguration (audio master: 5, "A.OPT 0"; the F6 option byte).
    pub system_configuration: u8,
    pub lock_mode: LockMode,
    /// Where a link source sends its Beo4 keys: the room's master (V_MASTER in Tore's setup,
    /// a Beosystem 3).
    pub room_master: u8,
    /// Tone set when the audio master turns its speakers on (stock: volume 34, loudness on).
    pub start_volume: u8,
    pub bass: i8,
    pub treble: i8,
    pub loudness: bool,
    /// Local time for the clock broadcast (audio master).
    pub clock: fn() -> LocalTime,
    /// The N.MUSIC and N.RADIO slots' ML addresses (MLConfig can move them, e.g. to A.MEM
    /// 0x79 or CD 0x8d, and RADIO 0x6f).
    pub n_music: u8,
    pub n_radio: u8,
}

impl Config {
    pub fn new(role: LinkRole) -> Self {
        Config {
            role,
            system_configuration: match role {
                LinkRole::AudioMaster => AudioMaster::DEFAULT_SYSTEM_CONFIGURATION,
                LinkRole::SourceCenter => 7,
            },
            lock_mode: LockMode::Firmware,
            room_master: node::VIDEO_MASTER,
            start_volume: 34,
            // = B&O's e3 a2 ff 00 00 (old neomaster and the captures)
            bass: -1,
            treble: 0,
            loudness: true,
            clock: local_time,
            n_music: ml_source::N_MUSIC,
            n_radio: ml_source::N_RADIO,
        }
    }

    /// The PC2 start-up mode for this role.
    pub fn pc2_mode(&self) -> pc2::Mode {
        match self.role {
            LinkRole::AudioMaster => pc2::Mode::AudioMaster,
            LinkRole::SourceCenter => pc2::Mode::SourceCenter,
        }
    }
}

enum Machine {
    AudioMaster(AudioMaster),
    SourceCenter(SourceCenter),
}

/// Wheel counts (UI volume steps) per Beo4 volume key in the link source role.
pub const COUNTS_PER_VOLUME_KEY: i32 = 4;

pub struct Link {
    cfg: Config,
    map: SourceMap,
    machine: Machine,
    mixer: Mixer,
    /// What the player plays, for us or for link rooms.
    playing: Option<LocalSource>,
    /// Picked on the panel (or the local remote) and not put to standby since.
    selected: Option<LocalSource>,
    /// Audio master: our speakers are on.
    speakers: bool,
    volume: u8,
    muted_volume: Option<u8>,
    /// Link source: volume steps not yet sent as Beo4 keys.
    volume_pending: i32,
    last_ui_volume: Option<u8>,
    track: TrackInfo,
    /// A link room just started this source: announce its track after the replies.
    announce: Option<(u8, LocalSource)>,
    started: bool,
}

impl Link {
    pub fn new(cfg: Config) -> Self {
        let machine = match cfg.role {
            LinkRole::AudioMaster => {
                let mut am = AudioMaster::new(cfg.system_configuration);
                am.set_lock_mode(cfg.lock_mode);
                Machine::AudioMaster(am)
            }
            LinkRole::SourceCenter => Machine::SourceCenter(SourceCenter::new()),
        };
        let mut map = SourceMap::new(cfg.role);
        (map.n_music, map.n_radio) = (cfg.n_music, cfg.n_radio);
        Link {
            map,
            volume: cfg.start_volume,
            machine,
            mixer: Mixer::new(),
            playing: None,
            selected: None,
            speakers: false,
            muted_volume: None,
            volume_pending: 0,
            last_ui_volume: None,
            track: TrackInfo::unknown(),
            announce: None,
            started: false,
            cfg,
        }
    }

    pub fn role(&self) -> LinkRole {
        self.cfg.role
    }
    /// Something here listens locally: the audio master has its speakers on.
    pub fn speakers(&self) -> bool {
        self.speakers
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn playing(&self) -> Option<LocalSource> {
        self.playing
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn mixer(&self) -> &pc2::MixerState {
        &self.mixer.state
    }
    /// What the debug stream reports as link state.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            started: self.started,
            playing: self.playing,
            selected: self.selected,
            speakers: self.speakers,
            volume: self.volume,
            bus: self.audio_master().map(|am| am.state()),
            holds_bus_lock: self.audio_master().is_some_and(|am| am.holds_bus_lock()),
            distributing: self.source_center().is_some_and(|sc| sc.distributing()),
            current_source: match &self.machine {
                Machine::AudioMaster(am) => am.current_source(),
                Machine::SourceCenter(sc) => sc.current_source(),
            },
        }
    }
    pub fn audio_master(&self) -> Option<&AudioMaster> {
        match &self.machine {
            Machine::AudioMaster(am) => Some(am),
            Machine::SourceCenter(_) => None,
        }
    }
    pub fn source_center(&self) -> Option<&SourceCenter> {
        match &self.machine {
            Machine::SourceCenter(sc) => Some(sc),
            Machine::AudioMaster(_) => None,
        }
    }

    // ------------------------------------------------------------ plumbing

    /// Run the state machine with a recording host, then carry out what it asked for.
    fn run(&mut self, now: Time, out: &mut Outputs, f: impl FnOnce(&mut Machine, &mut dyn Host)) {
        let map = self.map;
        let active = self.playing.or(self.selected);
        let mut host = Effects::new(move |src| map.local_source(src, active).is_some(), (self.cfg.clock)());
        f(&mut self.machine, &mut host);
        for e in host.out {
            self.apply(e, out);
        }
        if let Some((src, local)) = self.announce.take() {
            let track = self.track.clone();
            self.send_track_info(src, display_name(local), &track, now, out);
        }
    }

    fn apply(&mut self, e: MlEffect, out: &mut Outputs) {
        match e {
            MlEffect::Send(t) => out.pc2.push(Pc2Cmd::Telegram(t.encode())),
            MlEffect::MlDistribution(on) => self.mixer_out(out, |m| m.ml_distribute(on)),
            MlEffect::MlListening(on) => self.mixer_out(out, |m| m.transmit_from_ml(on)),
            MlEffect::StartSource(src) => {
                let Some(local) = self.map.local_source(src, self.playing.or(self.selected)) else { return };
                info!("Link room asked for source {src:#04x}: {local:?}");
                self.play(local, out);
                if self.cfg.role == LinkRole::SourceCenter {
                    // DISPLAY_DATA and METADATA once the connect replies are out (track_info.rs)
                    self.announce = Some((src, local));
                }
            }
            MlEffect::StopSource => {
                info!("Link rooms no longer need our source");
                // keep playing for our own speakers, or for a pick that is waiting to be connected
                let wanted_here = self.speakers || (self.cfg.role == LinkRole::SourceCenter && self.selected.is_some());
                if !wanted_here {
                    self.stop_player(out);
                }
            }
            MlEffect::SourceKey { src, key } => match self.playing {
                Some(p) if self.map.local_source(src, Some(p)) == Some(p) => {
                    out.app.push(AppEffect::SourceKey { source: p, key })
                }
                _ => debug!("Link key {key:#04x} for source {src:#04x}, which is not playing"),
            },
            MlEffect::SourceChannel { src, channel } => {
                if let Some(p) = self.playing.filter(|p| self.map.local_source(src, Some(*p)) == Some(*p)) {
                    out.app.push(AppEffect::SourceChannel { source: p, channel });
                }
            }
            MlEffect::LinkStandby => {
                info!("All standby on the link");
                self.speakers_off(out);
                self.selected = None;
                self.stop_player(out);
                out.app.push(AppEffect::Standby);
            }
            MlEffect::RequestBusLock => out.pc2.push(Pc2Cmd::Message(vec![0x24])),
            MlEffect::ReleaseBusLock => out.pc2.push(Pc2Cmd::Message(vec![0xe4, 0x01])),
        }
    }

    fn send_track_info(&mut self, src: u8, name: &str, track: &TrackInfo, now: Time, out: &mut Outputs) {
        let name = name.to_string();
        let track = track.clone();
        self.run(now, out, |m, host| {
            if let Machine::SourceCenter(sc) = m {
                sc.send_track_info(host, src, &name, &track);
            }
        });
    }

    fn mixer_out(&mut self, out: &mut Outputs, f: impl FnOnce(&mut Mixer) -> Vec<pc2::Message>) {
        out.pc2.extend(f(&mut self.mixer).into_iter().map(Pc2Cmd::Message));
    }

    fn play(&mut self, local: LocalSource, out: &mut Outputs) {
        if self.playing != Some(local) {
            self.playing = Some(local);
            out.app.push(AppEffect::Play(local));
        }
    }

    fn stop_player(&mut self, out: &mut Outputs) {
        if self.playing.take().is_some() {
            out.app.push(AppEffect::Stop);
        }
    }

    /// B&O's order: speakers off first, then routing.
    fn speakers_off(&mut self, out: &mut Outputs) {
        if self.speakers {
            self.speakers = false;
            self.mixer_out(out, |m| m.speaker_power(false));
            self.mixer_out(out, |m| m.transmit_locally(false));
        }
    }

    fn speakers_on(&mut self, local_audio: bool, out: &mut Outputs) {
        let (v, b, t, l) = (self.volume, self.cfg.bass, self.cfg.treble, self.cfg.loudness);
        if !self.speakers {
            self.mixer_out(out, |m| m.set_parameters(v as i32, b as i32, t as i32, 0, l));
        }
        self.mixer_out(out, |m| m.transmit_locally(local_audio));
        if !self.speakers {
            self.mixer_out(out, |m| m.speaker_power(true));
        }
        self.speakers = true;
    }

    fn remote_key(&mut self, key: u8, now: Time, out: &mut Outputs) {
        let dest = self.cfg.room_master;
        self.run(now, out, |m, host| {
            if let Machine::SourceCenter(sc) = m {
                // as captured: sent from the audio master's address C1
                sc.send_remote_key(host, dest, key, node::AUDIO_MASTER);
            }
        });
    }

    // ------------------------------------------------------------ lifecycle

    /// The PC2 is open and initialised: announce ourselves (audio master: idle status, clock,
    /// LINK_INFO probe; source center: idle status). Called again after a reconnect.
    pub fn start(&mut self, now: Time) -> Outputs {
        let mut out = Outputs::default();
        info!("Masterlink: starting as {:?}", self.cfg.role);
        self.run(now, &mut out, |m, host| match m {
            Machine::AudioMaster(am) => am.start(host, now),
            Machine::SourceCenter(sc) => sc.start(host, now),
        });
        self.started = true;
        out
    }

    /// The PC2 went away (unplugged, USB error). Its routing state is gone with it.
    pub fn stopped(&mut self) {
        self.started = false;
        self.speakers = false;
        self.mixer = Mixer::new();
    }

    pub fn tick(&mut self, now: Time) -> Outputs {
        let mut out = Outputs::default();
        if self.started {
            self.run(now, &mut out, |m, host| match m {
                Machine::AudioMaster(am) => am.tick(host, now),
                Machine::SourceCenter(sc) => sc.tick(host, now),
            });
        }
        out
    }

    pub fn next_deadline(&self) -> Option<Time> {
        if !self.started {
            return None;
        }
        match &self.machine {
            Machine::AudioMaster(am) => am.next_deadline(),
            Machine::SourceCenter(sc) => sc.next_deadline(),
        }
    }

    // ------------------------------------------------------------ intents

    /// A source picked on the panel or with the local remote.
    pub fn select_source(&mut self, local: LocalSource, now: Time) -> Outputs {
        let mut out = Outputs::default();
        self.selected = Some(local);
        match self.cfg.role {
            LinkRole::AudioMaster => {
                self.speakers_on(true, &mut out);
                self.play(local, &mut out);
                let src = self.map.ml_source(local);
                self.run(now, &mut out, |m, host| {
                    if let Machine::AudioMaster(am) = m {
                        am.set_local_source(host, src);
                    }
                });
            }
            LinkRole::SourceCenter => {
                let src = self.map.ml_source(local);
                let distributing = self.source_center().is_some_and(|sc| sc.current_source() == src);
                if let Machine::SourceCenter(sc) = &mut self.machine {
                    sc.set_local_playback(true);
                }
                self.remote_key(self.map.beo4_key(local), now, &mut out);
                // The master connects us with SOURCE_CONNECT and playback starts then. If the
                // slot is already on the link (A.AUX and N.MUSIC share it here), switch now.
                if distributing {
                    self.play(local, &mut out);
                }
            }
        }
        out
    }

    /// TV from the video master (audio master only; a link source sends the TV key instead).
    pub fn select_tv(&mut self, now: Time) -> Outputs {
        let mut out = Outputs::default();
        match self.cfg.role {
            LinkRole::AudioMaster => {
                self.selected = None;
                self.run(now, &mut out, |m, host| {
                    if let Machine::AudioMaster(am) = m {
                        am.listen_to_video_source(host, ml_source::TV, now);
                    }
                });
                self.speakers_on(false, &mut out);
                if self.audio_master().is_some_and(|am| am.listeners().is_empty()) {
                    self.stop_player(&mut out);
                }
            }
            LinkRole::SourceCenter => self.remote_key(beo4::TV, now, &mut out),
        }
        out
    }

    /// The volume wheel (absolute UI volume 0..=90).
    pub fn set_volume(&mut self, volume: u8, now: Time) -> Outputs {
        let mut out = Outputs::default();
        match self.cfg.role {
            LinkRole::AudioMaster => {
                self.volume = volume.min(pc2::mixer::MAX_VOLUME);
                self.muted_volume = None;
                let v = self.volume;
                self.mixer_out(&mut out, |m| m.set_volume(v as i32));
            }
            LinkRole::SourceCenter => {
                let last = self.last_ui_volume.replace(volume).unwrap_or(volume);
                self.volume_pending += volume as i32 - last as i32;
                while self.volume_pending.abs() >= COUNTS_PER_VOLUME_KEY {
                    let up = self.volume_pending > 0;
                    self.volume_pending -= COUNTS_PER_VOLUME_KEY * self.volume_pending.signum();
                    self.remote_key(if up { beo4::VOL_UP } else { beo4::VOL_DOWN }, now, &mut out);
                }
            }
        }
        out
    }

    /// Link source: the wheel's absolute value means nothing here, so the core re-centres the
    /// UI's volume after each turn; this keeps the step accumulator in line with it.
    pub fn rebase_volume(&mut self, volume: u8) {
        self.last_ui_volume = Some(volume);
    }

    /// Short standby: this room off.
    pub fn standby(&mut self, now: Time) -> Outputs {
        let mut out = Outputs::default();
        self.selected = None;
        match self.cfg.role {
            LinkRole::AudioMaster => {
                self.speakers_off(&mut out);
                self.run(now, &mut out, |m, host| {
                    if let Machine::AudioMaster(am) = m {
                        am.local_standby(host, now);
                    }
                });
                // Rooms still listening keep our source (B&O does the same).
                if self.audio_master().is_some_and(|am| am.listeners().is_empty()) {
                    self.stop_player(&mut out);
                }
            }
            LinkRole::SourceCenter => {
                if let Machine::SourceCenter(sc) = &mut self.machine {
                    sc.set_local_playback(false);
                }
                self.remote_key(beo4::STANDBY, now, &mut out);
                if !self.source_center().is_some_and(|sc| sc.distributing()) {
                    self.stop_player(&mut out);
                }
            }
        }
        out.app.push(AppEffect::Standby);
        out
    }

    /// Long standby: every node on the link off.
    pub fn all_standby(&mut self, now: Time) -> Outputs {
        let mut out = Outputs::default();
        self.selected = None;
        self.speakers_off(&mut out);
        self.run(now, &mut out, |m, host| match m {
            Machine::AudioMaster(am) => am.all_standby(host, now),
            Machine::SourceCenter(sc) => sc.all_standby(host, now),
        });
        self.stop_player(&mut out);
        out.app.push(AppEffect::Standby);
        out
    }

    /// The playing track changed (from Music Assistant). In the source center role it goes to
    /// the link rooms as DISPLAY_DATA and METADATA while we distribute.
    // Music Assistant's now-playing feeds this once the player is wired in.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn track_info(&mut self, info: TrackInfo, now: Time) -> Outputs {
        let mut out = Outputs::default();
        self.track = info;
        if let (Some(sc), Some(p)) = (self.source_center(), self.playing) {
            if sc.distributing() {
                let src = sc.current_source();
                let track = self.track.clone();
                self.send_track_info(src, display_name(p), &track, now, &mut out);
            }
        }
        out
    }

    // ------------------------------------------------------------ the PC2

    pub fn pc2_event(&mut self, ev: pc2::Event, now: Time) -> Outputs {
        let mut out = Outputs::default();
        match ev {
            pc2::Event::Masterlink(bytes) => match Telegram::decode(&bytes) {
                Ok(t) => {
                    trace!("ML in: {t:?}");
                    self.run(now, &mut out, |m, host| match m {
                        Machine::AudioMaster(am) => am.handle(host, &t, now),
                        Machine::SourceCenter(sc) => sc.handle(host, &t, now),
                    });
                }
                Err(e) => warn!("Undecodable Masterlink telegram {bytes:02x?}: {e:?}"),
            },
            pc2::Event::BusLockGranted => self.run(now, &mut out, |m, host| match m {
                Machine::AudioMaster(am) => am.bus_lock_granted(host),
                Machine::SourceCenter(sc) => sc.bus_lock_granted(host, now),
            }),
            pc2::Event::Beo4Key { kind, code } => return self.ir_key(kind, code, now),
            pc2::Event::MixerState { volume, .. } => {
                if self.cfg.role == LinkRole::AudioMaster {
                    self.volume = volume;
                    out.app.push(AppEffect::Volume(volume));
                }
            }
            pc2::Event::Temperature(c) => debug!("PC2 temperature {c} C"),
            pc2::Event::NodeAddress(a) => info!("Masterlink node address is now {a:#04x}"),
            pc2::Event::Wakeup(w) => info!("PC2 wake-up: {w:?}"),
            pc2::Event::Headphones(on) => debug!("Headphones {}", if on { "in" } else { "out" }),
            pc2::Event::Other(m) => trace!("PC2: {m:02x?}"),
        }
        out
    }

    /// A key from the PC2's IR eye. Only the audio master acts on it; in a link room the room's
    /// master has its own eye and answers the same remote.
    fn ir_key(&mut self, kind: u8, code: u8, now: Time) -> Outputs {
        let held = kind == beo4::TYPE_HELD;
        if self.cfg.role != LinkRole::AudioMaster {
            debug!("IR key {code:#04x} (type {kind:#04x}): ignored, the room's master acts on it");
            return Outputs::default();
        }
        info!("IR key {code:#04x}{}", if held { " (held)" } else { "" });
        let source =
            [LocalSource::NMusic, LocalSource::NRadio, LocalSource::AAux].into_iter().find(|s| self.map.beo4_key(*s) == code);
        match code {
            beo4::STANDBY if held => self.all_standby(now),
            beo4::STANDBY => self.standby(now),
            beo4::VOL_UP | beo4::VOL_DOWN => {
                let step: i32 = if code == beo4::VOL_UP { 1 } else { -1 };
                let v = (self.muted_volume.unwrap_or(self.volume) as i32 + step).clamp(0, pc2::mixer::MAX_VOLUME as i32) as u8;
                let mut out = self.set_volume(v, now);
                out.app.push(AppEffect::Volume(v));
                out
            }
            beo4::MUTE if !held => {
                let mut out = Outputs::default();
                let v = match self.muted_volume.take() {
                    Some(v) => v,
                    None => {
                        self.muted_volume = Some(self.volume);
                        0
                    }
                };
                self.mixer_out(&mut out, |m| m.set_volume(v as i32));
                if v != 0 {
                    self.volume = v;
                }
                out
            }
            _ if held => Outputs::default(),
            beo4::TV => self.select_tv(now),
            _ if source.is_some() => self.select_source(source.unwrap(), now),
            _ => {
                let mut out = Outputs::default();
                if let Some(p) = self.playing {
                    out.app.push(AppEffect::SourceKey { source: p, key: code });
                }
                out
            }
        }
    }
}

/// The name a link room's display shows for our source.
pub fn display_name(local: LocalSource) -> &'static str {
    match local {
        LocalSource::NMusic => "N.MUSIC",
        LocalSource::NRadio => "N.RADIO",
        LocalSource::AAux => "A.AUX",
    }
}

/// The UI's source names.
pub fn local_source(name: &str) -> Option<LocalSource> {
    match name {
        "N.MUSIC" => Some(LocalSource::NMusic),
        "N.RADIO" => Some(LocalSource::NRadio),
        "A.AUX" => Some(LocalSource::AAux),
        _ => None,
    }
}

/// The wall clock, in the local time zone, for the audio master's clock broadcast.
pub fn local_time() -> LocalTime {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    // SAFETY: localtime_r only writes the struct we own.
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return LocalTime { hour: 0, minute: 0, second: 0, day: 1, month: 1, year: 2000 };
    }
    LocalTime {
        hour: tm.tm_hour as u8,
        minute: tm.tm_min as u8,
        second: tm.tm_sec as u8,
        day: tm.tm_mday as u8,
        month: (tm.tm_mon + 1) as u8,
        year: (tm.tm_year + 1900) as u16,
    }
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
