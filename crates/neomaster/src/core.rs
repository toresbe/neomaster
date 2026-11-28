//! The core: one task that owns all application state. Actors send it [`Event`]s; it updates
//! the state, carries out effects through the actors' channels, and hands a fresh scene to the
//! render thread. Nothing here blocks, sleeps or touches a device, so a run can be replayed from
//! its event log.

use std::time::{Duration, Instant};

use a_aux::AuxSource;
use bs5_panel::{Decoder, Output, Report};
use log::{debug, info};
use masterlink::{LinkRole, LocalSource, TrackInfo};
use render::Scene;

/// Where the core puts each new scene for the render thread, with the moment it was made.
pub type SceneTx = render::mailbox::Sender<(Instant, Scene)>;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::sleep_until;
use ui::{Effect, Ui};

use crate::debug::{self, Inject, Tap};
use crate::link::{self, AppEffect, Link, Outputs, Pc2Cmd};
use crate::music::{MusicCmd, MusicEvent, MusicOut, NoMusic};
use crate::radio_actor::{NoRadio, RadioCmd, RadioOut};
use crate::settings_rt::{self, SettingsOut};

/// Time since the core started.
pub type Time = Duration;

#[derive(Debug)]
pub enum Event {
    /// A raw IN report from the panel (or the simulator's stand-in).
    PanelReport([u8; bs5_panel::REPORT_LEN]),
    /// The keyboard's standby key (simulator only; the panel has none).
    Standby,
    /// Long standby: everything on the link off (simulator: Shift+S).
    AllStandby,
    /// Something the PC2 reported (IR key, Masterlink telegram, mixer state, ...).
    Pc2(pc2::Event),
    /// The PC2 is open and its start-up messages are on their way.
    Pc2Connected,
    /// The PC2 went away.
    Pc2Disconnected,
    /// The A.AUX card poll (every [`AUX_POLL`]): the USB sound card, or `None` if absent.
    AuxCard(Option<a_aux::Card>),
    /// The machine facts for the SETTINGS menu, gathered off the core task.
    Facts(Box<settings::menu::Facts>),
    /// From the AirPlay receiver: a metadata item, or shairport-sync restarted.
    AirPlay(airplay::receiver::Report),
    /// The cover art from the `PICT` item just before, decoded to [`ui::COVER_SIZE`] (`None`:
    /// the sender has none, or it could not be decoded). Decoded on the receiver's thread.
    AirPlayCover(Option<std::sync::Arc<render::Image>>),
    /// The screen needs redrawing (window exposed).
    Redraw,
    /// The N.RADIO actor matched preset `number`'s name to this Music Assistant station.
    RadioResolved {
        number: u8,
        uri: String,
    },
    /// From the N.MUSIC actor (Music Assistant): the library, what plays, the queue.
    Music(MusicEvent),
    /// From a debug client (see [`crate::debug`]).
    Debug(Inject),
    Quit,
}

/// Where effects for the panel go: OUT reports for the click and backlight.
pub trait PanelOut: Send {
    fn write(&mut self, report: [u8; 2]);
}

/// How often the A.AUX card is looked for (BeoSound5.exe got device arrival from Windows).
pub const AUX_POLL: Duration = Duration::from_secs(2);

/// Where A.AUX effects go: the loop that plays the USB card through the BM5.
pub trait AuxOut: Send {
    fn apply(&mut self, effect: &a_aux::Effect);
    /// The loop exited by itself since the last call.
    fn ended(&mut self) -> bool;
    /// The A.AUX settings changed (they apply from the next start of the loop).
    fn configure(&mut self, _settings: &settings::Aux) {}
}

impl AuxOut for a_aux::loopback::Loopback {
    fn apply(&mut self, effect: &a_aux::Effect) {
        a_aux::loopback::Loopback::apply(self, effect)
    }
    fn ended(&mut self) -> bool {
        a_aux::loopback::Loopback::ended(self)
    }
    fn configure(&mut self, settings: &settings::Aux) {
        self.set_config(settings_rt::loopback_config(settings));
    }
}

/// Where AirPlay effects go: the shairport-sync receiver.
pub trait AirPlayOut: Send {
    fn apply(&mut self, effect: airplay::Effect);
}

impl AirPlayOut for airplay::receiver::Receiver {
    fn apply(&mut self, effect: airplay::Effect) {
        airplay::receiver::Receiver::apply(self, effect)
    }
}

/// No AirPlay receiver (disabled, or tests).
pub struct NoAirPlay;

impl AirPlayOut for NoAirPlay {
    fn apply(&mut self, _: airplay::Effect) {}
}

/// The panel's and the link rooms' name for AirPlay.
const AIRPLAY: &str = "AIRPLAY";

/// An AirPlay track goes to link rooms once it has stood this long. Senders send several
/// metadata bundles in a burst (Music Assistant sends its queue neighbours' before the playing
/// track's), and each costs seven telegrams on the link.
const LINK_TRACK_SETTLE: Duration = Duration::from_secs(1);

/// Where messages for the PC2 go (the PC2 actor's command channel).
pub trait Pc2Out: Send {
    fn send(&mut self, cmd: Pc2Cmd);
}

impl Pc2Out for mpsc::UnboundedSender<Pc2Cmd> {
    fn send(&mut self, cmd: Pc2Cmd) {
        // Nobody listening means the actor has stopped; the core is shutting down too.
        let _ = mpsc::UnboundedSender::send(self, cmd);
    }
}

/// The UI's volume in the link source role, where the room's master owns the real one: the
/// wheel turns from here and the core puts it back after each turn.
const NEUTRAL_VOLUME: u8 = ui::MAX_VOLUME / 2;

pub struct Core<P: PanelOut, A: AuxOut, Q: Pc2Out> {
    ui: Ui,
    decoder: Decoder,
    panel: Output,
    panel_out: P,
    aux: AuxSource,
    aux_out: A,
    airplay: airplay::Session,
    airplay_out: Box<dyn AirPlayOut>,
    /// An AirPlay track waiting for [`LINK_TRACK_SETTLE`] before it goes to link rooms.
    link_track: Option<(Time, TrackInfo)>,
    link: Link,
    pc2_out: Q,
    scenes: SceneTx,
    epoch: Instant,
    /// The debug stream ([`Tap::global`] unless [`Core::with_tap`] says otherwise).
    tap: Tap,
    /// Buttons held in the last panel report, for injected movement.
    panel_buttons: u8,
    pc2_connected: bool,
    /// The last `state` record's contents, to report what changed.
    last_state: Option<Value>,
    /// N.RADIO: which preset a channel number, Beo4 key or the panel asks for.
    tuner: radio::Tuner,
    radio_out: Box<dyn RadioOut>,
    /// The preset playing through Music Assistant (0: N.RADIO is not playing).
    radio_playing: u8,
    /// A preset picked on the panel while N.RADIO waits to be connected (link source role).
    radio_pending: Option<u8>,
    settings: settings::Settings,
    settings_out: Option<Box<dyn SettingsOut>>,
    /// The core's own event sender, for work it hands to a blocking thread.
    events: Option<mpsc::UnboundedSender<Event>>,
    /// The A.AUX card poll's card choice (settings `aux.card`).
    aux_card_choice: Option<tokio::sync::watch::Sender<String>>,
    /// Where N.MUSIC's commands go (the actor in [`crate::music`]).
    music_out: Box<dyn MusicOut>,
    /// Music Assistant is configured (else the library is made up and nothing plays).
    music_enabled: bool,
    /// What the panel asked to play while N.MUSIC waits to be connected (link source role) or
    /// selected; sent when it starts.
    music_pending: Option<MusicCmd>,
    /// N.MUSIC is the playing source.
    music_playing: bool,
    /// N.MUSIC was selected because Music Assistant started playing on the BM5 by itself (from
    /// its app): it plays already, so selecting it sends nothing.
    music_external: bool,
    /// What N.MUSIC last played (album from the track, or the track), and whether N.RADIO has
    /// replaced the queue since: then selecting N.MUSIC plays that again instead of resuming
    /// the station.
    music_last: Option<ui::PlayRequest>,
    radio_took_queue: bool,
}

impl<P: PanelOut, A: AuxOut, Q: Pc2Out> Core<P, A, Q> {
    pub fn new(mut ui: Ui, panel_out: P, aux_out: A, link: Link, pc2_out: Q, scenes: SceneTx) -> Self {
        let mut link = link;
        ui.set_volume_known(link.role() != LinkRole::SourceCenter);
        if link.role() == LinkRole::SourceCenter {
            ui.set_volume(NEUTRAL_VOLUME);
            link.rebase_volume(NEUTRAL_VOLUME);
        }
        Core {
            ui,
            decoder: Decoder::new(),
            panel: Output::new(),
            panel_out,
            aux: AuxSource::new(),
            aux_out,
            airplay: airplay::Session::new(),
            airplay_out: Box::new(NoAirPlay),
            link_track: None,
            link,
            pc2_out,
            scenes,
            epoch: Instant::now(),
            tap: Tap::global(),
            panel_buttons: 0,
            pc2_connected: false,
            last_state: None,
            tuner: radio::Tuner::default(),
            radio_out: Box::new(NoRadio),
            radio_playing: 0,
            radio_pending: None,
            settings: settings::Settings::default(),
            settings_out: None,
            events: None,
            aux_card_choice: None,
            music_out: Box::new(NoMusic),
            music_enabled: false,
            music_pending: None,
            music_playing: false,
            music_external: false,
            music_last: None,
            radio_took_queue: false,
        }
    }

    /// N.RADIO's presets and where its commands go (the actor in [`crate::radio_actor`]).
    pub fn with_radio(mut self, tuner: radio::Tuner, out: impl RadioOut + 'static) -> Self {
        self.tuner = tuner;
        self.radio_out = Box::new(out);
        self
    }

    /// N.MUSIC through Music Assistant: where its commands go.
    pub fn with_music(mut self, out: impl MusicOut + 'static) -> Self {
        self.music_out = Box::new(out);
        self.music_enabled = true;
        self
    }

    fn music_send(&mut self, cmd: MusicCmd) {
        self.tap.emit("music_cmd", || json!({ "cmd": format!("{cmd:?}") }));
        self.music_out.send(cmd);
    }

    /// The panel asked to play something: at once if N.MUSIC plays, else select N.MUSIC and
    /// send it when the source starts (in the link source role, when the room connects us).
    fn music_request(&mut self, cmd: MusicCmd, now: Time) {
        if !self.music_enabled {
            info!("{cmd:?} (no Music Assistant configured; nothing plays)");
            return;
        }
        if self.link.playing() == Some(LocalSource::NMusic) {
            self.music_send(cmd);
            return;
        }
        self.music_pending = Some(cmd);
        let out = self.link.select_source(LocalSource::NMusic, now);
        self.link_out(out, now);
    }

    /// N.MUSIC became the playing source.
    fn music_started(&mut self) {
        self.music_playing = true;
        let pending = self.music_pending.take();
        if std::mem::take(&mut self.music_external) && pending.is_none() {
            return;
        }
        let took = std::mem::take(&mut self.radio_took_queue);
        if self.music_enabled {
            let resume = match (took, self.music_last.clone()) {
                (true, Some(last)) => MusicCmd::Play(last),
                _ => MusicCmd::Resume,
            };
            self.music_send(pending.unwrap_or(resume));
        }
    }

    /// N.MUSIC is no longer the source (another one, standby, or released by the room): pause
    /// it, so it resumes where it was. N.RADIO plays on the same Music Assistant queue and
    /// replaces it, so it needs no pause (which could arrive after the station started).
    fn music_released(&mut self, next: Option<LocalSource>) {
        self.music_pending = None;
        self.music_external = false;
        if std::mem::take(&mut self.music_playing) && next != Some(LocalSource::NRadio) && self.music_enabled {
            self.music_send(MusicCmd::Pause);
        }
    }

    /// A Beo4 key (ours, or a link room's) for N.MUSIC while it plays. Inferred from the
    /// manual's Beo4 page: arrows up and down step tracks, left and right step albums.
    fn music_key(&mut self, key: u8) {
        use pc2::beo4;
        let cmd = match key {
            beo4::PLAY => Some(MusicCmd::Resume),
            beo4::STOP => Some(MusicCmd::Pause),
            beo4::ARROW_UP => Some(MusicCmd::Next),
            beo4::ARROW_DOWN => Some(MusicCmd::Previous),
            beo4::ARROW_RIGHT | beo4::ARROW_LEFT => {
                let delta = if key == beo4::ARROW_RIGHT { 1 } else { -1 };
                self.ui.album_step(delta).map(|uri| MusicCmd::Play(ui::PlayRequest { uri, start: None }))
            }
            _ => None,
        };
        match cmd {
            Some(c) if self.music_enabled => self.music_send(c),
            Some(c) => info!("N.MUSIC: Beo4 key {key:#04x} -> {c:?} (no Music Assistant configured)"),
            None => info!("N.MUSIC: Beo4 key {key:#04x} does nothing"),
        }
    }

    /// GO, LEFT and RIGHT on the now-playing view: N.MUSIC's play/pause and track steps; on
    /// N.RADIO, LEFT and RIGHT step presets as the Beo4's arrows do; with nothing playing, GO
    /// starts N.MUSIC.
    fn transport(&mut self, t: ui::Transport, now: Time) {
        use ui::Transport::*;
        match (self.link.playing(), t) {
            (Some(LocalSource::NMusic), _) if self.music_enabled => self.music_send(match t {
                PlayPause => MusicCmd::PlayPause,
                Previous => MusicCmd::Previous,
                Next => MusicCmd::Next,
            }),
            (Some(LocalSource::NRadio), Previous | Next) => {
                let key = if t == Next { pc2::beo4::ARROW_UP } else { pc2::beo4::ARROW_DOWN };
                let tune = self.tuner.key(key, now);
                self.radio_tune(tune);
            }
            (None, PlayPause) => self.music_request(MusicCmd::Resume, now),
            (playing, t) => info!("{t:?} on now-playing does nothing while {:?} plays", playing.map(link::display_name)),
        }
    }

    /// Something from the N.MUSIC actor. Returns whether the screen may have changed.
    fn music_event(&mut self, ev: MusicEvent, now: Time) -> bool {
        self.tap.emit("music", || json!({ "event": format!("{ev:?}") }));
        match ev {
            MusicEvent::Library(lib) => self.ui.set_library(*lib),
            MusicEvent::NowPlaying(np) => {
                let info = np.as_ref().map(|n| {
                    let mut t = masterlink::TrackInfo::unknown();
                    let set = |field: &mut String, v: &str| {
                        if !v.is_empty() {
                            *field = v.to_string();
                        }
                    };
                    set(&mut t.title, &n.title);
                    set(&mut t.artist, &n.artist);
                    set(&mut t.album, &n.album);
                    t
                });
                let changed = self.ui.now_playing().map(|n| (&n.title, &n.artist, &n.album))
                    != np.as_ref().map(|n| (&n.title, &n.artist, &n.album));
                if self.music_playing {
                    if let Some(n) = np.as_ref().filter(|n| n.uri.is_some()) {
                        self.music_last = Some(match &n.album_uri {
                            Some(album) => ui::PlayRequest { uri: album.clone(), start: n.uri.clone() },
                            None => ui::PlayRequest { uri: n.uri.clone().unwrap_or_default(), start: None },
                        });
                    }
                }
                self.ui.set_now_playing(np);
                // link rooms' displays follow the track while we distribute N.MUSIC
                if let (true, Some(info), true) = (changed, info, self.music_playing) {
                    let out = self.link.track_info(info, now);
                    self.link_out(out, now);
                }
            }
            MusicEvent::Art { uri, image } => self.ui.set_now_playing_art(uri.as_deref(), image),
            MusicEvent::Queue(q) => self.ui.set_queue(q),
            MusicEvent::Stream(true) => {
                // Started from Music Assistant's app with nothing local playing: as audio master,
                // put N.MUSIC on the speakers. A link source cannot pick the room's source.
                if !self.music_playing && self.link.playing().is_none() {
                    if self.link.role() == LinkRole::AudioMaster {
                        info!("N.MUSIC: Music Assistant started playing here; selecting N.MUSIC");
                        self.music_external = true;
                        let out = self.link.select_source(LocalSource::NMusic, now);
                        self.link_out(out, now);
                    } else {
                        info!("N.MUSIC: Music Assistant started playing here; select N.MUSIC in the room to hear it");
                    }
                }
                return false;
            }
            MusicEvent::Stream(false) => return false,
        }
        true
    }

    /// Send debug records here instead of to [`Tap::global`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_tap(mut self, tap: Tap) -> Self {
        self.tap = tap;
        self
    }

    /// Run with these settings (the link given to [`Core::new`] should already follow them):
    /// the UI shows them, changes made in the SETTINGS menu go to `out`, and `events` is the
    /// sender of the core's own event channel.
    pub fn with_settings(
        mut self,
        settings: settings::Settings,
        out: Box<dyn SettingsOut>,
        events: mpsc::UnboundedSender<Event>,
        aux_card_choice: Option<tokio::sync::watch::Sender<String>>,
    ) -> Self {
        self.aux_out.configure(&settings.aux);
        self.ui.set_settings(settings.clone());
        self.settings = settings;
        self.apply_idle();
        self.settings_out = Some(out);
        self.events = Some(events);
        self.aux_card_choice = aux_card_choice;
        self
    }

    fn apply_idle(&mut self) {
        let idle = &self.settings.idle;
        let cfg = self.ui.idle_config();
        cfg.return_after = idle.return_after();
        cfg.standby_tick = idle.standby_tick();
        cfg.playing_standby_after = idle.active_playback_timeout();
    }

    /// The SETTINGS menu changed something: save it and apply what can be applied live.
    fn settings_changed(&mut self, new: settings::Settings) {
        let old = std::mem::replace(&mut self.settings, new);
        if let Some(out) = self.settings_out.as_mut() {
            out.save(&self.settings);
        }
        if old.idle != self.settings.idle {
            self.apply_idle();
        }
        if old.system.debug_logging != self.settings.system.debug_logging {
            settings_rt::set_debug_logging(self.settings.system.debug_logging);
        }
        if old.aux != self.settings.aux {
            self.aux_out.configure(&self.settings.aux);
            if let Some(tx) = &self.aux_card_choice {
                let _ = tx.send(self.settings.aux.card.clone());
            }
        }
        if old.roles != self.settings.roles || old.link != self.settings.link {
            // A new role or new source addresses: stop what plays, start the link glue and the
            // PC2 over in the new configuration.
            let cfg = link::Config::from_settings(&self.settings);
            info!(
                "Masterlink: now {:?}, SystemConfiguration {}, N.MUSIC {:#04x}, N.RADIO {:#04x}",
                cfg.role, cfg.system_configuration, cfg.n_music, cfg.n_radio
            );
            let fx = self.aux.deselect();
            self.aux_effects(fx);
            let mode = cfg.pc2_mode();
            self.link = Link::new(cfg);
            self.ui.set_volume_known(self.link.role() != LinkRole::SourceCenter);
            if self.link.role() == LinkRole::SourceCenter {
                self.ui.set_volume(NEUTRAL_VOLUME);
                self.link.rebase_volume(NEUTRAL_VOLUME);
            }
            self.tap.emit("settings_roles", || json!({ "mode": format!("{mode:?}") }));
            self.pc2_out.send(Pc2Cmd::Restart(mode));
        }
    }

    fn refresh_facts(&mut self) {
        let Some(events) = self.events.clone() else { return };
        let s = self.settings.clone();
        tokio::task::spawn_blocking(move || {
            let _ = events.send(Event::Facts(Box::new(settings_rt::gather_facts(&s))));
        });
    }

    /// Take AirPlay effects to this receiver (the default is none).
    pub fn with_airplay(mut self, out: Box<dyn AirPlayOut>) -> Self {
        self.airplay_out = out;
        self
    }

    /// Something other than AirPlay plays now, or nothing does: end the AirPlay session.
    fn drop_airplay(&mut self, now: Time) {
        if self.airplay.is_active() {
            self.airplay_out.apply(airplay::Effect::Drop);
            // The restart report would say the same; act now so the panel does not lag.
            let ev = self.airplay.receiver_restarted();
            self.airplay_events(ev, now);
        }
    }

    fn airplay_report(&mut self, report: airplay::receiver::Report, now: Time) {
        let ev = match report {
            airplay::receiver::Report::Item(item) => {
                self.tap.emit("airplay_item", || json!({ "item": format!("{item:?}") }));
                self.airplay.item(&item, now)
            }
            airplay::receiver::Report::Restarted => self.airplay.receiver_restarted(),
        };
        self.airplay_events(ev, now);
    }

    fn airplay_events(&mut self, events: Vec<airplay::Event>, now: Time) {
        for e in events {
            self.tap.emit("airplay", || {
                json!({ "event": match &e {
                    airplay::Event::Cover(c) => format!("Cover({} bytes)", c.len()),
                    e => format!("{e:?}"),
                } })
            });
            match e {
                airplay::Event::Started(sender) => {
                    info!("AirPlay: {} started streaming; taking the source", sender.label());
                    // shairport-sync plays at once; A.AUX's loop must not hold the output.
                    let fx = self.aux.deselect();
                    self.aux_effects(fx);
                    self.ui.set_plain_source(None);
                    self.ui.set_external(Some(ui::External {
                        source: AIRPLAY.into(),
                        from: sender.name.clone().unwrap_or_default(),
                        ..Default::default()
                    }));
                    let fx = self.ui.show_now_playing(now);
                    self.effects(fx, now);
                    let out = self.link.track_info(TrackInfo::unknown(), now);
                    self.link_out(out, now);
                    let out = self.link.select_source(LocalSource::AirPlay, now);
                    self.link_out(out, now);
                }
                airplay::Event::Track(t) => {
                    info!("AirPlay: {} - {}", t.artist, t.title);
                    if let Some(x) = self.ui.external_mut() {
                        (x.title, x.artist, x.album) = (t.title.clone(), t.artist.clone(), t.album.clone());
                    }
                    self.link_track = Some((now + LINK_TRACK_SETTLE, track_info(&t)));
                }
                // The decoded copy arrives as Event::AirPlayCover.
                airplay::Event::Cover(_) => {}
                airplay::Event::Paused | airplay::Event::Resumed => {
                    let paused = e == airplay::Event::Paused;
                    if let Some(x) = self.ui.external_mut() {
                        x.paused = paused;
                    }
                    self.ui.set_playing(!paused);
                }
                airplay::Event::Ended => {
                    info!("AirPlay: streaming ended; releasing the source");
                    self.ui.set_external(None);
                    self.link_track = None;
                    let out = self.link.release_source(LocalSource::AirPlay, now);
                    self.link_out(out, now);
                    let out = self.link.track_info(TrackInfo::unknown(), now);
                    self.link_out(out, now);
                }
            }
        }
    }

    /// Send the settled AirPlay track to link rooms, if it is due.
    fn send_link_track(&mut self, now: Time) {
        if self.link_track.as_ref().is_some_and(|(due, _)| *due <= now) {
            let (_, info) = self.link_track.take().unwrap();
            let out = self.link.track_info(info, now);
            self.link_out(out, now);
        }
    }

    fn aux_effects(&mut self, fx: Vec<a_aux::Effect>) {
        for e in &fx {
            self.aux_out.apply(e);
        }
    }

    fn aux_card(&mut self, card: Option<a_aux::Card>) {
        if card.as_ref() != self.aux.card() {
            match &card {
                Some(c) => info!("A.AUX: USB sound card found: {c}"),
                None if self.aux.card().is_some() => info!("A.AUX: USB sound card gone"),
                None => {}
            }
        }
        let mut fx = self.aux.set_card(card);
        if fx.is_empty() && self.aux_out.ended() {
            fx = self.aux.loop_ended();
        }
        self.aux_effects(fx);
    }

    /// Start a preset the tuner chose, unless it is already the one playing.
    fn radio_tune(&mut self, tune: Option<radio::Tune>) {
        let Some(t) = tune else { return };
        if self.radio_playing == t.preset.number {
            return;
        }
        self.radio_playing = t.preset.number;
        self.radio_took_queue = true;
        self.tap.emit("radio", || json!({ "preset": t.preset.number, "name": t.preset.name, "uri": t.preset.uri }));
        self.radio_out.send(RadioCmd::Play(t.preset));
        self.radio_save();
    }

    /// N.RADIO is no longer the source: stop its Music Assistant queue.
    fn radio_stop(&mut self) {
        self.radio_stop_unless_music(false);
    }

    /// N.RADIO is no longer the source. When N.MUSIC follows, it takes over the same queue, so
    /// no stop is sent: from another actor's connection it could land after N.MUSIC started.
    fn radio_stop_unless_music(&mut self, music_next: bool) {
        if self.radio_playing != 0 {
            self.radio_playing = 0;
            self.tuner.stopped();
            if !music_next {
                self.radio_out.send(RadioCmd::Stop);
            }
        }
    }

    fn radio_save(&mut self) {
        if self.tuner.take_dirty() {
            self.radio_out.send(RadioCmd::Save(self.tuner.presets().clone()));
        }
    }

    /// A preset picked on the panel (or by a debug client): N.RADIO at that number.
    fn radio_preset(&mut self, number: u8, now: Time) {
        if self.link.snapshot().playing == Some(LocalSource::NRadio) {
            let t = self.tuner.channel(number);
            self.radio_tune(t);
            return;
        }
        self.radio_pending = Some(number);
        let out = self.link.select_source(LocalSource::NRadio, now);
        self.link_out(out, now);
    }

    /// Carry out what the link glue decided: PC2 messages to the actor, player effects here.
    fn link_out(&mut self, out: Outputs, now: Time) {
        // A room that connects N.RADIO at a channel (a timer's preset) sends both; start that
        // preset rather than the last one and then the asked-for one.
        let radio_channel = out.app.iter().find_map(|e| match e {
            AppEffect::SourceChannel { source: LocalSource::NRadio, channel } => Some(*channel),
            _ => None,
        });
        for cmd in out.pc2 {
            match &cmd {
                Pc2Cmd::Telegram(t) => self.tap.emit("ml_out", || debug::telegram_fields(t)),
                Pc2Cmd::Message(m) => self.tap.emit("pc2_out", || json!({ "raw": debug::hex(m) })),
                Pc2Cmd::Restart(m) => self.tap.emit("pc2_restart", || json!({ "mode": format!("{m:?}") })),
            }
            self.pc2_out.send(cmd);
        }
        for e in out.app {
            self.tap.emit("app_effect", || json!({ "effect": format!("{e:?}") }));
            match e {
                // A.AUX plays the USB card, N.RADIO a preset through Music Assistant; N.MUSIC is
                // not wired in yet.
                AppEffect::Play(s) => {
                    info!("play {}", link::display_name(s));
                    self.ui.set_playing(true);
                    if s == LocalSource::NRadio {
                        let tune = match radio_channel.or(self.radio_pending.take()) {
                            Some(n) => self.tuner.channel(n).or_else(|| self.tuner.resume()),
                            None => self.tuner.resume(),
                        };
                        if tune.is_none() {
                            info!("N.RADIO: no presets (see {})", radio::store::default_path().display());
                        }
                        self.radio_tune(tune);
                    } else {
                        // N.MUSIC plays on the same queue and replaces the station: no stop
                        // that could land after it starts
                        self.radio_stop_unless_music(s == LocalSource::NMusic);
                    }
                    if s == LocalSource::NMusic {
                        self.music_started();
                    } else {
                        self.music_released(Some(s));
                    }
                    if s != LocalSource::AirPlay {
                        self.drop_airplay(now);
                    }
                    // A.AUX is a line in: now-playing shows only its name.
                    self.ui.set_plain_source((s == LocalSource::AAux).then_some("A.AUX"));
                    let fx = if s == LocalSource::AAux {
                        if self.aux.card().is_none() {
                            info!("A.AUX: waiting for a USB sound card (\"USB Audio CODEC\")");
                        }
                        self.aux.select()
                    } else {
                        self.aux.deselect()
                    };
                    self.aux_effects(fx);
                }
                AppEffect::Stop => {
                    info!("stop playback");
                    self.ui.set_plain_source(None);
                    self.radio_stop();
                    self.drop_airplay(now);
                    self.music_released(None);
                    let fx = self.aux.deselect();
                    self.aux_effects(fx);
                    // The stock UI blanks when its source stops and nothing local listens.
                    let fx = self.ui.source_stopped(self.link.speakers(), now);
                    self.effects(fx, now);
                }
                AppEffect::SourceKey { source: LocalSource::NRadio, key } => {
                    let t = self.tuner.key(key, now);
                    self.radio_tune(t);
                }
                AppEffect::SourceKey { source: LocalSource::NMusic, key } => self.music_key(key),
                AppEffect::SourceKey { source, key } => info!("{}: Beo4 key {key:#04x}", link::display_name(source)),
                AppEffect::SourceChannel { source: LocalSource::NRadio, channel } => {
                    // When it came with the Play above, that preset is already playing and
                    // radio_tune leaves it be.
                    let t = self.tuner.channel(channel);
                    self.radio_tune(t);
                }
                AppEffect::SourceChannel { source, channel } => info!("{}: channel {channel}", link::display_name(source)),
                AppEffect::Standby => {
                    let fx = self.ui.blank(now);
                    self.effects(fx, now);
                }
                AppEffect::Volume(v) => self.ui.set_volume(v),
            }
        }
    }

    /// A MODE list label as a local source: the slot names (A.MEM, CD, RADIO...) follow the
    /// settings.
    fn local_source(&self, label: &str) -> Option<LocalSource> {
        use settings::names::Slot;
        let slot = self.settings.roles.source_labels(true).into_iter().find(|(l, _)| *l == label).map(|(_, s)| s);
        match slot {
            Some(Slot::NMusic) => Some(LocalSource::NMusic),
            Some(Slot::NRadio) => Some(LocalSource::NRadio),
            // PC CD has no player in neomaster.
            Some(_) => None,
            None => link::local_source(label),
        }
    }

    fn now(&self) -> Time {
        self.epoch.elapsed()
    }

    fn panel_write(&mut self, report: [u8; 2]) {
        self.tap.emit("panel_out", || json!({ "raw": debug::hex(&report) }));
        self.panel_out.write(report);
    }

    fn effects(&mut self, fx: Vec<Effect>, now: Time) {
        for e in fx {
            self.tap.emit("ui_effect", || json!({ "effect": format!("{e:?}") }));
            match e {
                Effect::Click => {
                    if let Some(r) = self.panel.click(now) {
                        self.panel_write(r);
                    }
                }
                Effect::Backlight(on) => {
                    for r in if on { self.panel.wake() } else { self.panel.blank() } {
                        self.panel_write(r);
                    }
                }
                Effect::LedRefresh => {
                    let r = self.panel.led_refresh();
                    self.panel_write(r);
                }
                // Blank-screen standby stops what plays locally.
                Effect::StopLocalPlayback => {
                    self.radio_stop();
                    self.drop_airplay(now);
                    self.music_released(None);
                    let fx = self.aux.deselect();
                    self.aux_effects(fx);
                }
                // Volume goes to the PC2 (audio master) or as Beo4 keys to the room's master
                // (link source).
                Effect::Volume(v) => {
                    debug!("volume {v}");
                    let out = self.link.set_volume(v, now);
                    if self.link.role() == LinkRole::SourceCenter {
                        self.ui.set_volume(NEUTRAL_VOLUME);
                        self.link.rebase_volume(NEUTRAL_VOLUME);
                    }
                    self.link_out(out, now);
                }
                Effect::SettingsOpened => self.refresh_facts(),
                Effect::SettingsChanged(s) => self.settings_changed(*s),
                Effect::Play(req) => self.music_request(MusicCmd::Play(req), now),
                Effect::PlayQueueIndex(i) => self.music_request(MusicCmd::PlayIndex(i), now),
                Effect::Transport(t) => self.transport(t, now),
                // Adding to the queue does not change the source.
                Effect::Enqueue(uris) if self.music_enabled => self.music_send(MusicCmd::Enqueue(uris)),
                Effect::Enqueue(uris) => info!("enqueue {uris:?} (no Music Assistant configured)"),
                Effect::SelectSource(s) => {
                    info!("select source {s}");
                    if let Some(local) = self.local_source(s) {
                        // Picking a source on the panel ends AirPlay at once, even where the new
                        // source waits for the room's master to connect it.
                        self.drop_airplay(now);
                        // Show A.AUX at once, also where the room's master connects it later.
                        self.ui.set_plain_source((local == LocalSource::AAux).then_some("A.AUX"));
                        let out = self.link.select_source(local, now);
                        self.link_out(out, now);
                    }
                }
                Effect::Standby => {
                    info!("standby");
                    let out = self.link.standby(now);
                    self.link_out(out, now);
                }
            }
        }
    }

    fn draw(&mut self, now: Time) {
        // Replaces a scene the render thread has not taken yet: it only wants the newest.
        let _ = self.scenes.send((Instant::now(), self.ui.scene(now)));
    }

    /// When the UI next needs a tick or a frame.
    fn ui_deadline(&self, now: Time) -> Option<Time> {
        [self.ui.next_timer(), self.ui.next_frame(now), self.decoder.next_deadline(), self.tuner.next_deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Handle one event. Returns false if the screen cannot have changed.
    fn event(&mut self, ev: Event, now: Time) -> bool {
        match &ev {
            Event::Pc2Connected | Event::Pc2Disconnected | Event::Standby | Event::AllStandby | Event::Quit => {
                self.tap.emit("core_event", || json!({ "event": format!("{ev:?}") }))
            }
            _ => {}
        }
        match ev {
            Event::PanelReport(bytes) => {
                self.panel_buttons = bytes[3];
                let report = Report::parse(&bytes);
                self.tap.emit("panel_in", || {
                    let mut v = json!({ "raw": debug::hex(&bytes) });
                    if let Some(r) = report {
                        v["menu"] = json!(r.menu);
                        v["volume"] = json!(r.volume);
                        v["pointer"] = json!(r.pointer);
                        v["buttons"] = json!(format!("{:02x}", r.buttons));
                    }
                    v
                });
                if let Some(report) = report {
                    for input in self.decoder.feed(report, now) {
                        self.panel_input(input, now);
                    }
                }
            }
            Event::Standby => {
                let fx = self.ui.standby(now);
                self.effects(fx, now);
            }
            Event::AllStandby => {
                info!("long standby");
                let _ = self.ui.standby(now);
                let out = self.link.all_standby(now);
                self.link_out(out, now);
            }
            Event::Pc2(ev) => {
                match &ev {
                    pc2::Event::Masterlink(t) => self.tap.emit("ml_in", || debug::telegram_fields(t)),
                    pc2::Event::Beo4Key { kind, code } => self.tap.emit("ir", || {
                        json!({ "key_type": format!("{kind:02x}"), "code": format!("{code:02x}"),
                                "key": debug::beo4_name(*code), "held": *kind == pc2::beo4::TYPE_HELD })
                    }),
                    e => self.tap.emit("pc2_in", || json!({ "event": format!("{e:?}") })),
                }
                let out = self.link.pc2_event(ev, now);
                let visible = !out.app.is_empty();
                self.link_out(out, now);
                return visible;
            }
            Event::Pc2Connected => {
                self.pc2_connected = true;
                let out = self.link.start(now);
                self.link_out(out, now);
                return false;
            }
            Event::Pc2Disconnected => {
                info!("PC2 gone; Masterlink paused until it is back");
                self.link.stopped();
                self.pc2_connected = false;
                return false;
            }
            Event::AuxCard(card) => {
                self.aux_card(card);
                return false;
            }
            Event::RadioResolved { number, uri } => {
                if self.tuner.presets_mut().set_uri(number, &uri) {
                    self.radio_save();
                }
                return false;
            }
            Event::Facts(f) => {
                self.ui.set_facts(*f);
                return self.ui.settings_open();
            }
            Event::Music(m) => return self.music_event(m, now),
            Event::AirPlay(report) => self.airplay_report(report, now),
            Event::AirPlayCover(image) => {
                if self.airplay.is_active() {
                    if let Some(x) = self.ui.external_mut() {
                        x.cover = image;
                    }
                }
            }
            Event::Redraw => {}
            Event::Debug(i) => return self.inject(i, now),
            Event::Quit => {}
        }
        true
    }

    fn panel_input(&mut self, input: bs5_panel::Input, now: Time) {
        self.tap.emit("panel_input", || json!({ "input": format!("{input:?}") }));
        let fx = self.ui.input(input, now);
        self.effects(fx, now);
    }

    /// A panel report as the panel would send it now, with these deltas.
    fn panel_report(&self, menu: i32, volume: i32, pointer: Option<u8>, buttons: u8) -> [u8; bs5_panel::REPORT_LEN] {
        // Before the first report the decoder has no pointer; the middle of the arc will do.
        let pointer = pointer.or(self.decoder.pointer()).unwrap_or(bs5_panel::POINTER_MAX / 2);
        Report { menu: menu as i8, volume: volume as i8, pointer, buttons }.encode()
    }

    /// Carry out a debug client's injection; it takes the path the real input would.
    fn inject(&mut self, i: Inject, now: Time) -> bool {
        self.tap.emit("inject", || {
            json!({ "inject": match &i {
                Inject::TelegramIn(t) => format!("TelegramIn [{}]", debug::hex(t)),
                Inject::TelegramOut(t) => format!("TelegramOut [{}]", debug::hex(t)),
                Inject::Pc2Message(m) => format!("Pc2Message [{}]", debug::hex(m)),
                i => format!("{i:?}"),
            } })
        });
        match i {
            Inject::PanelRaw(bytes) => self.event(Event::PanelReport(bytes), now),
            Inject::Panel { mut menu, mut volume, pointer } => {
                // One report per 127 counts, as the panel would split a fast turn.
                let mut pointer = pointer;
                let mut redraw = false;
                loop {
                    let (m, v) = (menu.clamp(-127, 127), volume.clamp(-127, 127));
                    let r = self.panel_report(m, v, pointer.take(), self.panel_buttons);
                    redraw |= self.event(Event::PanelReport(r), now);
                    (menu, volume) = (menu - m, volume - v);
                    if menu == 0 && volume == 0 {
                        return redraw;
                    }
                }
            }
            Inject::Button { button, down } => {
                let buttons = if down { self.panel_buttons | button.mask() } else { self.panel_buttons & !button.mask() };
                let r = self.panel_report(0, 0, None, buttons);
                self.event(Event::PanelReport(r), now)
            }
            Inject::Beo4 { kind, code } => self.event(Event::Pc2(pc2::Event::Beo4Key { kind, code }), now),
            Inject::TelegramIn(t) => self.event(Event::Pc2(pc2::Event::Masterlink(t)), now),
            Inject::TelegramOut(t) => {
                self.link_out(Outputs { pc2: vec![Pc2Cmd::Telegram(t)], app: vec![] }, now);
                false
            }
            Inject::Pc2Message(m) => {
                self.link_out(Outputs { pc2: vec![Pc2Cmd::Message(m)], app: vec![] }, now);
                false
            }
            Inject::Standby => self.event(Event::Standby, now),
            Inject::AllStandby => self.event(Event::AllStandby, now),
            Inject::Snapshot => {
                self.emit_state(true);
                false
            }
            Inject::RadioPreset(n) => {
                self.radio_preset(n, now);
                true
            }
        }
    }

    /// What the `state` record reports.
    fn state(&self) -> Value {
        let l = self.link.snapshot();
        json!({
            "role": format!("{:?}", self.link.role()),
            "pc2": self.pc2_connected,
            "link_started": l.started,
            "playing": l.playing.map(link::display_name),
            "selected": l.selected.map(link::display_name),
            "speakers": l.speakers,
            "volume": l.volume,
            "bus": l.bus.map(|b| format!("{b:?}")),
            "bus_lock": l.holds_bus_lock,
            "distributing": l.distributing,
            "ml_source": format!("{:02x}", l.current_source),
            "zone": self.ui.zone().name,
            "ui_volume": self.ui.volume(),
            "blanked": self.ui.blanked(),
            "aux_card": self.aux.card().map(|c| c.to_string()),
            "radio_preset": self.tuner.current().map(|p| json!({ "number": p.number, "name": p.name })),
            "airplay": self.airplay.is_active().then(|| json!({
                "sender": self.airplay.sender().label(),
                "title": self.airplay.track().title,
                "artist": self.airplay.track().artist,
                "paused": self.airplay.is_paused(),
            })),
            "music": self.ui.now_playing().map(|n| json!({ "title": n.title, "artist": n.artist, "playing": n.playing })),
            "music_playing": self.music_playing,
        })
    }

    /// Emit a `state` record if anything changed since the last one (or always, if `force`).
    fn emit_state(&mut self, force: bool) {
        if !self.tap.listening() {
            return;
        }
        let now = self.state();
        let mut changed = serde_json::Map::new();
        if let (Some(Value::Object(old)), Value::Object(new)) = (&self.last_state, &now) {
            for (k, v) in new {
                if old.get(k) != Some(v) {
                    changed.insert(k.clone(), json!([old.get(k), v]));
                }
            }
        }
        if force || self.last_state.is_none() || !changed.is_empty() {
            let state = now.clone();
            self.tap.emit("state", move || json!({ "changed": changed, "state": state }));
        }
        self.last_state = Some(now);
    }

    /// The next moment the core has to wake up without an event.
    fn deadline(&self, now: Time) -> Time {
        [self.ui_deadline(now), self.link.next_deadline(), self.airplay.next_deadline(), self.link_track.as_ref().map(|t| t.0)]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(now + Duration::from_secs(3600))
            .max(now)
    }

    pub async fn run(mut self, mut events: mpsc::UnboundedReceiver<Event>) {
        self.draw(self.now());
        loop {
            let now = self.now();
            let wake = tokio::time::Instant::from_std(self.epoch + self.deadline(now));
            // Set when the loop wakes, so handle_us is the work and not the wait before it.
            let started;
            let mut events_handled = 0;
            let redraw = tokio::select! {
                ev = events.recv() => {
                    started = Instant::now();
                    let Some(ev) = ev else { return };
                    // Handle everything that is already queued, then draw once.
                    let mut batch = vec![ev];
                    while let Ok(ev) = events.try_recv() {
                        batch.push(ev);
                    }
                    let now = self.now();
                    let mut redraw = false;
                    events_handled = batch.len();
                    for ev in batch {
                        if matches!(ev, Event::Quit) {
                            self.event(ev, now);
                            return;
                        }
                        redraw |= self.event(ev, now);
                    }
                    redraw
                }
                _ = sleep_until(wake) => {
                    started = Instant::now();
                    let now = self.now();
                    let ui_due = self.ui_deadline(now).is_some_and(|d| d <= now);
                    for input in self.decoder.tick(now) {
                        self.panel_input(input, now);
                    }
                    let fx = self.ui.tick(now);
                    self.effects(fx, now);
                    let t = self.tuner.tick(now);
                    self.radio_tune(t);
                    let out = self.link.tick(now);
                    self.link_out(out, now);
                    let ev = self.airplay.tick(now);
                    let airplay_ended = !ev.is_empty();
                    self.airplay_events(ev, now);
                    self.send_link_track(now);
                    ui_due || airplay_ended
                }
            };
            let handled = started.elapsed();
            if redraw {
                self.draw(self.now());
            }
            if self.tap.listening() {
                let (handled, total) = (handled.as_micros() as u64, started.elapsed().as_micros() as u64);
                let trigger = if events_handled > 0 { "events" } else { "tick" };
                self.tap.emit("timing", || {
                    json!({ "trigger": trigger, "events": events_handled, "handle_us": handled, "draw_us": total - handled, "redraw": redraw })
                });
                self.emit_state(false);
            }
        }
    }
}

/// What link rooms show for an AirPlay track. Fields the sender left out get B&O's "unknown"
/// texts, as its own players send for untagged tracks.
fn track_info(t: &airplay::Track) -> TrackInfo {
    let or = |s: &str, d: &str| if s.is_empty() { d.to_string() } else { s.to_string() };
    TrackInfo {
        genre: or(&t.genre, TrackInfo::UNKNOWN_GENRE),
        album: or(&t.album, TrackInfo::UNKNOWN_ALBUM),
        artist: or(&t.artist, TrackInfo::UNKNOWN_ARTIST),
        title: or(&t.title, TrackInfo::UNKNOWN_TITLE),
        ..TrackInfo::unknown()
    }
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
}

/// Start the AirPlay receiver (shairport-sync), reporting to the core. Cover art is decoded on
/// the receiver's thread. It plays to `device` (A.AUX's playback device: the output the PC2
/// hears) unless `NEOMASTER_AIRPLAY_DEVICE` says otherwise. `None` if it is switched off or
/// cannot be set up.
pub fn start_airplay(events: mpsc::UnboundedSender<Event>, device: &str) -> Option<airplay::receiver::Receiver> {
    let config = airplay::receiver::ReceiverConfig::from_env_with_device(device)?;
    let report = move |r: airplay::receiver::Report| {
        let cover = match &r {
            airplay::receiver::Report::Item(i) if i.kind == airplay::code::SSNC && i.code == airplay::code::PICT => {
                Some(match airplay::cover::decode_square(&i.data, ui::COVER_SIZE) {
                    Ok(img) => Some(render::Image::new(img.width, img.height, rgb_to_rgba(&img.data))),
                    Err(airplay::cover::Error::Empty) => None,
                    Err(e) => {
                        log::warn!("AirPlay: {e}");
                        None
                    }
                })
            }
            _ => None,
        };
        let _ = events.send(Event::AirPlay(r));
        if let Some(c) = cover {
            let _ = events.send(Event::AirPlayCover(c));
        }
    };
    match airplay::receiver::Receiver::start(config, report) {
        Ok(r) => Some(r),
        Err(e) => {
            log::warn!("AirPlay: could not set up the receiver: {e}");
            None
        }
    }
}

/// [`poll_aux_card`], with the card chosen in the settings (`aux.card`, an ALSA card id; empty
/// = the first "USB Audio CODEC"). The choice can change while running.
pub async fn poll_aux_card_choice(events: mpsc::UnboundedSender<Event>, choice: tokio::sync::watch::Receiver<String>) {
    let mut interval = tokio::time::interval(AUX_POLL);
    loop {
        interval.tick().await;
        let id = choice.borrow().clone();
        let card = tokio::task::spawn_blocking(move || a_aux::device::scan_for(Some(id.as_str()).filter(|i| !i.is_empty())))
            .await
            .unwrap_or(None);
        if events.send(Event::AuxCard(card)).is_err() {
            return;
        }
    }
}

/// Look for the A.AUX card every [`AUX_POLL`] and tell the core. Ends when the core is gone.
#[allow(dead_code)] // the binary uses poll_aux_card_choice; kept for callers without settings
pub async fn poll_aux_card(events: mpsc::UnboundedSender<Event>) {
    poll_aux_card_choice(events, tokio::sync::watch::channel(String::new()).1).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use bs5_panel::Button;
    use std::collections::HashMap;

    struct NoPanel;
    impl PanelOut for NoPanel {
        fn write(&mut self, _: [u8; 2]) {}
    }
    struct NoAux;
    impl AuxOut for NoAux {
        fn apply(&mut self, _: &a_aux::Effect) {}
        fn ended(&mut self) -> bool {
            false
        }
    }

    fn presets() -> radio::Presets {
        let mut p = radio::Presets::default();
        for (n, name) in [(1, "NRK P1"), (8, "Deutschlandfunk"), (12, "Radio Paradise")] {
            p.set(radio::Preset::new(n, name, format!("library://radio/{n}")));
        }
        p.last = 12;
        p
    }

    /// Run `events` through a core in `role` with the presets above; return the N.RADIO commands.
    async fn radio_run(role: LinkRole, events: Vec<Event>) -> Vec<RadioCmd> {
        let (scene_tx, _scene_rx) = render::mailbox::channel();
        let (pc2_tx, _pc2_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
        let (radio_tx, mut radio_rx) = mpsc::unbounded_channel::<RadioCmd>();
        let link = Link::new(link::Config::new(role));
        let core = Core::new(Ui::new(ui::fake::Library::generate()), NoPanel, NoAux, link, pc2_tx, scene_tx)
            .with_tap(Tap::new())
            .with_radio(radio::Tuner::new(presets()), radio_tx);
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(core.run(ev_rx));
        for e in events {
            ev_tx.send(e).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ev_tx.send(Event::Quit).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
        let mut cmds = vec![];
        while let Ok(c) = radio_rx.try_recv() {
            cmds.push(c);
        }
        cmds
    }

    fn plays(cmds: &[RadioCmd]) -> Vec<u8> {
        cmds.iter().filter_map(|c| if let RadioCmd::Play(p) = c { Some(p.number) } else { None }).collect()
    }

    fn telegram(hex: &str) -> Event {
        Event::Debug(Inject::TelegramIn(debug::inject::telegram(hex).unwrap()))
    }

    /// This morning's Beosystem 3 wake-up timer (work/ml_wake_timer_0700.txt): the audio master
    /// asks us, the link source, for N.RADIO at channel 8, then again without the timer flag.
    /// Preset 8 plays once, without the last preset (12) starting first; RELEASE stops it.
    #[tokio::test]
    async fn wake_up_timer_plays_preset_8() {
        let cmds = radio_run(
            LinkRole::SourceCenter,
            vec![
                Event::Pc2Connected,
                telegram("c2c1010ba100006c0a0100000800020201030c00"),
                telegram("c2c1010ba100006c0a0100000800020201ff0200"),
                telegram("c2c1010aa10000110002"),
            ],
        )
        .await;
        assert_eq!(plays(&cmds), [8], "{cmds:?}");
        assert!(matches!(cmds.last(), Some(RadioCmd::Stop)), "{cmds:?}");
        let saved = cmds.iter().find_map(|c| if let RadioCmd::Save(p) = c { Some(p.last) } else { None });
        assert_eq!(saved, Some(8), "the last preset is saved");
    }

    /// Audio master: the N.RADIO key resumes the last preset, then Beo4 digits and arrows tune.
    #[tokio::test]
    async fn beo4_digits_and_arrows_pick_presets() {
        let ir = |code| Event::Pc2(pc2::Event::Beo4Key { kind: 0x01, code });
        let n_radio = masterlink::SourceMap::new(LinkRole::AudioMaster).beo4_key(LocalSource::NRadio);
        let cmds = radio_run(
            LinkRole::AudioMaster,
            vec![
                Event::Pc2Connected,
                ir(n_radio),
                ir(pc2::beo4::DIGIT_8),
                ir(pc2::beo4::DIGIT_1),
                ir(pc2::beo4::DIGIT_2),
                ir(pc2::beo4::ARROW_UP),
                Event::Debug(Inject::RadioPreset(8)),
            ],
        )
        .await;
        assert_eq!(plays(&cmds), [12, 8, 12, 1, 8], "{cmds:?}");
    }

    /// A preset picked on the panel selects N.RADIO at that number.
    #[tokio::test]
    async fn panel_pick_selects_n_radio_at_the_preset() {
        let cmds = radio_run(LinkRole::AudioMaster, vec![Event::Pc2Connected, Event::Debug(Inject::RadioPreset(1))]).await;
        assert_eq!(plays(&cmds), [1], "{cmds:?}");
    }

    #[derive(Clone, Default)]
    struct AirPlayLog(std::sync::Arc<std::sync::Mutex<Vec<airplay::Effect>>>);
    impl AirPlayOut for AirPlayLog {
        fn apply(&mut self, e: airplay::Effect) {
            self.0.lock().unwrap().push(e);
        }
    }

    fn items(text: &str) -> Vec<airplay::Item> {
        airplay::Parser::new().feed(text.as_bytes()).0
    }

    fn sent_telegrams(rx: &mut mpsc::UnboundedReceiver<Pc2Cmd>) -> Vec<Vec<u8>> {
        let mut v = vec![];
        while let Ok(c) = rx.try_recv() {
            if let Pc2Cmd::Telegram(t) = c {
                v.push(t);
            }
        }
        v
    }

    /// A captured AirPlay session (Music Assistant to shairport-sync) through the core, as a
    /// link source: the room's master gets the N.MUSIC key, the panel shows the track, the
    /// release sends STANDBY; picking another source drops a running session.
    #[test]
    fn airplay_session_takes_and_releases_the_source() {
        let (scene_tx, _scene_rx) = render::mailbox::channel();
        let (pc2_tx, mut pc2_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
        let link = Link::new(link::Config::new(LinkRole::SourceCenter));
        let log = AirPlayLog::default();
        let mut core = Core::new(Ui::new(ui::fake::Library::generate()), NoPanel, NoAux, link, pc2_tx, scene_tx)
            .with_tap(Tap::new())
            .with_airplay(Box::new(log.clone()));
        let t = Duration::from_secs(1);
        core.event(Event::Pc2Connected, t);
        sent_telegrams(&mut pc2_rx);

        let capture = items(include_str!("../../airplay/tests/data/ma_session.txt"));
        let pend = capture.iter().position(|i| i.code == airplay::code::PEND).unwrap();
        for item in &capture[..pend] {
            core.event(Event::AirPlay(airplay::receiver::Report::Item(item.clone())), t);
        }
        let n_music_key =
            [0xc0, 0xc1, 0x01, 0x0a, 0x00, 0x47, 0x00, 0x20, 0x05, 0x02, 0x00, 0x01, 0xff, 0xff, pc2::beo4::N_MUSIC];
        let sent = sent_telegrams(&mut pc2_rx);
        assert!(sent.iter().any(|s| s[..15] == n_music_key), "{sent:02x?}");
        let x = core.ui.external().expect("the panel shows AirPlay");
        assert_eq!((x.source.as_str(), x.title.as_str()), ("AIRPLAY", "All In Love Is Fair"));
        assert!(x.paused, "the capture ends paused");
        core.event(Event::AirPlayCover(Some(render::Image::new(2, 2, vec![0; 16]))), t);
        assert!(core.ui.external().unwrap().cover.is_some());

        // pend, then the grace period
        core.event(Event::AirPlay(airplay::receiver::Report::Item(capture[pend].clone())), t);
        assert!(core.ui.external().is_some());
        let later = t + airplay::Session::END_GRACE;
        let ev = core.airplay.tick(later);
        core.airplay_events(ev, later);
        assert!(core.ui.external().is_none());
        // never connected by the room's master: no STANDBY for a room on something else
        let standby_key =
            [0xc0, 0xc1, 0x01, 0x0a, 0x00, 0x47, 0x00, 0x20, 0x05, 0x02, 0x00, 0x01, 0xff, 0xff, pc2::beo4::STANDBY];
        assert!(!sent_telegrams(&mut pc2_rx).iter().any(|s| s[..15] == standby_key));
        assert!(log.0.lock().unwrap().is_empty(), "the session ended by itself");

        // a new session, connected this time; then N.MUSIC picked on the panel drops it
        for item in &capture[..pend] {
            core.event(Event::AirPlay(airplay::receiver::Report::Item(item.clone())), later);
        }
        let connect = [
            0xc2, 0xc1, 0x01, 0x0b, 0x7a, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02,
            0x00,
        ];
        let connect = masterlink::Telegram::decode(&connect).unwrap().encode();
        core.event(Event::Pc2(pc2::Event::Masterlink(connect)), later);
        assert_eq!(core.link.playing(), Some(LocalSource::AirPlay));
        // the track reaches the room once it has settled, as DISPLAY_DATA and six METADATA
        sent_telegrams(&mut pc2_rx);
        core.send_link_track(later);
        assert!(sent_telegrams(&mut pc2_rx).is_empty(), "not before it settles");
        core.send_link_track(later + LINK_TRACK_SETTLE);
        let info = sent_telegrams(&mut pc2_rx);
        assert_eq!(info.len(), 7);
        assert!(info.iter().any(|t| t.windows(19).any(|w| w == b"All In Love Is Fair")), "{info:02x?}");
        let fx = vec![Effect::SelectSource("N.MUSIC")];
        core.effects(fx, later);
        // N.MUSIC shares the slot here, so it plays at once and AirPlay is dropped
        assert_eq!(core.link.playing(), Some(LocalSource::NMusic));
        assert_eq!(*log.0.lock().unwrap(), vec![airplay::Effect::Drop]);
        assert!(core.ui.external().is_none());
        assert!(!core.airplay.is_active());
    }

    /// A.AUX picked on the panel: now-playing shows just "A.AUX", in both roles; another
    /// source puts the normal view back.
    #[test]
    fn a_aux_shows_only_its_name() {
        for role in [LinkRole::AudioMaster, LinkRole::SourceCenter] {
            let (scene_tx, _scene_rx) = render::mailbox::channel();
            let (pc2_tx, _pc2_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
            let link = Link::new(link::Config::new(role));
            let mut core =
                Core::new(Ui::new(ui::fake::Library::generate()), NoPanel, NoAux, link, pc2_tx, scene_tx).with_tap(Tap::new());
            let t = Duration::from_secs(1);
            core.event(Event::Pc2Connected, t);
            core.effects(vec![Effect::SelectSource("A.AUX")], t);
            assert_eq!(core.ui.plain_source(), Some("A.AUX"), "{role:?}");
            core.effects(vec![Effect::SelectSource("N.MUSIC")], t);
            assert_eq!(core.ui.plain_source(), None, "{role:?}");
        }
    }

    /// Run `events` through a core in `role` with the made-up library and N.MUSIC wired to a
    /// channel; return the Music Assistant commands it sent.
    async fn music_run(role: LinkRole, events: Vec<Event>) -> Vec<MusicCmd> {
        let (scene_tx, _scene_rx) = render::mailbox::channel();
        let (pc2_tx, _pc2_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
        let (music_tx, mut music_rx) = mpsc::unbounded_channel::<MusicCmd>();
        let link = Link::new(link::Config::new(role));
        let core = Core::new(Ui::new(ui::fake::Library::generate()), NoPanel, NoAux, link, pc2_tx, scene_tx)
            .with_tap(Tap::new())
            .with_music(music_tx);
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(core.run(ev_rx));
        for e in events {
            ev_tx.send(e).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ev_tx.send(Event::Quit).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
        let mut cmds = vec![];
        while let Ok(c) = music_rx.try_recv() {
            cmds.push(c);
        }
        cmds
    }

    /// GO on the first album in ALBUMS (pointer at 70).
    fn go_on_first_album() -> Vec<Event> {
        vec![
            Event::Debug(Inject::Panel { menu: 0, volume: 0, pointer: Some(70) }),
            Event::Debug(Inject::Button { button: Button::Go, down: true }),
            Event::Debug(Inject::Button { button: Button::Go, down: false }),
        ]
    }

    fn first_album_uri() -> String {
        let lib = ui::fake::Library::generate();
        lib.albums[lib.albums_in_order()[0]].uri.clone()
    }

    fn ir(code: u8) -> Event {
        Event::Pc2(pc2::Event::Beo4Key { kind: 0x01, code })
    }

    /// Audio master: GO plays the album at once (N.MUSIC goes on the speakers with it); the
    /// Beo4's arrows step tracks and albums, STOP pauses, and another source pauses N.MUSIC.
    #[tokio::test]
    async fn audio_master_go_plays_and_beo4_steps() {
        let n_music = masterlink::SourceMap::new(LinkRole::AudioMaster).beo4_key(LocalSource::NMusic);
        let a_aux = masterlink::SourceMap::new(LinkRole::AudioMaster).beo4_key(LocalSource::AAux);
        let mut ev = vec![Event::Pc2Connected];
        ev.extend(go_on_first_album());
        ev.extend([
            ir(pc2::beo4::ARROW_UP),
            ir(pc2::beo4::ARROW_DOWN),
            ir(pc2::beo4::ARROW_RIGHT),
            ir(pc2::beo4::STOP),
            ir(pc2::beo4::PLAY),
            ir(a_aux),
            ir(n_music),
        ]);
        let cmds = music_run(LinkRole::AudioMaster, ev).await;
        let lib = ui::fake::Library::generate();
        let order = lib.albums_in_order();
        let play = |a: usize| MusicCmd::Play(ui::PlayRequest { uri: lib.albums[a].uri.clone(), start: None });
        assert_eq!(
            cmds,
            vec![
                play(order[0]),
                MusicCmd::Next,
                MusicCmd::Previous,
                play(order[1]), // the next album after the one playing
                MusicCmd::Pause,
                MusicCmd::Resume,
                MusicCmd::Pause,  // A.AUX selected
                MusicCmd::Resume, // N.MUSIC again: where it was
            ]
        );
    }

    /// Link source: GO asks the room for N.MUSIC (Beo4 key to the master) and plays only once
    /// the master connects us; a link room's SOURCE_CONTROL arrow steps the track. The room
    /// releasing us keeps it playing, since the panel picked it (link.rs); when only a room
    /// asked for N.MUSIC, it resumes the queue and the release pauses it.
    #[tokio::test]
    async fn link_source_plays_when_connected_and_takes_room_keys() {
        let connect = masterlink::Telegram::decode(&[
            0xc2, 0xc1, 0x01, 0x0b, 0x7a, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02,
            0x00,
        ])
        .unwrap()
        .encode();
        let room_key = masterlink::Telegram::decode(&[
            0xc2, 0xc1, 0x01, 0x0a, 0x7a, 0x00, 0x00, 0x0d, 0x06, 0x01, 0x7a, 0x1e, 0x02, 0x01, 0x02, 0x00,
        ])
        .unwrap()
        .encode();
        let release =
            masterlink::Telegram::decode(&[0xc2, 0xc1, 0x01, 0x0a, 0x7a, 0x00, 0x00, 0x11, 0x00, 0x02]).unwrap().encode();
        let mut ev = vec![Event::Pc2Connected];
        ev.extend(go_on_first_album());
        let mut first = vec![Event::Pc2Connected];
        first.extend(go_on_first_album());
        let before = music_run(LinkRole::SourceCenter, first).await;
        assert!(before.is_empty(), "nothing plays before the master connects us: {before:?}");
        ev.extend([
            Event::Debug(Inject::TelegramIn(connect.clone())),
            Event::Debug(Inject::TelegramIn(room_key)),
            Event::Debug(Inject::TelegramIn(release.clone())),
        ]);
        let cmds = music_run(LinkRole::SourceCenter, ev).await;
        assert_eq!(cmds, vec![MusicCmd::Play(ui::PlayRequest { uri: first_album_uri(), start: None }), MusicCmd::Next]);
        let cmds = music_run(
            LinkRole::SourceCenter,
            vec![Event::Pc2Connected, Event::Debug(Inject::TelegramIn(connect)), Event::Debug(Inject::TelegramIn(release))],
        )
        .await;
        assert_eq!(cmds, vec![MusicCmd::Resume, MusicCmd::Pause]);
    }

    /// A Beo4 N.MUSIC with nothing asked on the panel resumes the queue; Music Assistant's
    /// now-playing reaches the UI.
    #[tokio::test]
    async fn n_music_key_resumes_and_now_playing_shows() {
        let n_music = masterlink::SourceMap::new(LinkRole::AudioMaster).beo4_key(LocalSource::NMusic);
        let np = ui::NowPlaying { title: "Something".into(), artist: "The Beatles".into(), playing: true, ..Default::default() };
        let cmds = music_run(
            LinkRole::AudioMaster,
            vec![Event::Pc2Connected, ir(n_music), Event::Music(MusicEvent::NowPlaying(Some(np)))],
        )
        .await;
        assert_eq!(cmds, vec![MusicCmd::Resume]);
    }

    /// Music Assistant starting a stream on the BM5 by itself (its app) with nothing local
    /// playing selects N.MUSIC as audio master, without sending it a play of its own.
    #[tokio::test]
    async fn stream_from_ma_selects_n_music_as_audio_master() {
        let cmds = music_run(
            LinkRole::AudioMaster,
            vec![Event::Pc2Connected, Event::Music(MusicEvent::Stream(true)), ir(pc2::beo4::ARROW_UP)],
        )
        .await;
        assert_eq!(cmds, vec![MusicCmd::Next], "the arrow works: N.MUSIC is the source now");
    }

    /// On the now-playing view GO plays and pauses and LEFT / RIGHT step tracks, through MA.
    #[tokio::test]
    async fn now_playing_buttons_are_transport() {
        let button = |button, down| Event::Debug(Inject::Button { button, down });
        let mut ev = vec![Event::Pc2Connected];
        ev.extend(go_on_first_album());
        ev.extend([
            Event::Debug(Inject::Panel { menu: 0, volume: 0, pointer: Some(5) }),
            button(Button::Go, true),
            button(Button::Go, false),
            button(Button::Left, true),
            button(Button::Left, false),
            button(Button::Right, true),
            button(Button::Right, false),
        ]);
        let cmds = music_run(LinkRole::AudioMaster, ev).await;
        assert_eq!(
            cmds,
            vec![
                MusicCmd::Play(ui::PlayRequest { uri: first_album_uri(), start: None }),
                MusicCmd::PlayPause,
                MusicCmd::Previous,
                MusicCmd::Next,
            ]
        );
    }

    /// N.MUSIC and N.RADIO share the BM5's Music Assistant queue. Going from N.MUSIC to N.RADIO
    /// sends N.MUSIC no pause (the station replaces it); going back sends N.RADIO no stop (it
    /// could land after N.MUSIC started) and plays N.MUSIC's last album again from its track,
    /// since resuming would resume the station.
    #[tokio::test]
    async fn n_music_and_n_radio_hand_the_queue_over() {
        let (scene_tx, _scene_rx) = render::mailbox::channel();
        let (pc2_tx, _pc2_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
        let (music_tx, mut music_rx) = mpsc::unbounded_channel::<MusicCmd>();
        let (radio_tx, mut radio_rx) = mpsc::unbounded_channel::<RadioCmd>();
        let link = Link::new(link::Config::new(LinkRole::AudioMaster));
        let core = Core::new(Ui::new(ui::fake::Library::generate()), NoPanel, NoAux, link, pc2_tx, scene_tx)
            .with_tap(Tap::new())
            .with_music(music_tx)
            .with_radio(radio::Tuner::new(presets()), radio_tx);
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(core.run(ev_rx));
        let map = masterlink::SourceMap::new(LinkRole::AudioMaster);
        let np = ui::NowPlaying {
            title: "Track".into(),
            uri: Some("library://track/7".into()),
            album_uri: Some("library://album/3".into()),
            playing: true,
            ..Default::default()
        };
        let mut events = vec![Event::Pc2Connected];
        events.extend(go_on_first_album());
        events.extend([
            Event::Music(MusicEvent::NowPlaying(Some(np))),
            ir(map.beo4_key(LocalSource::NRadio)),
            ir(map.beo4_key(LocalSource::NMusic)),
        ]);
        for e in events {
            ev_tx.send(e).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ev_tx.send(Event::Quit).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
        let music: Vec<MusicCmd> = std::iter::from_fn(|| music_rx.try_recv().ok()).collect();
        let radio: Vec<RadioCmd> = std::iter::from_fn(|| radio_rx.try_recv().ok()).collect();
        assert_eq!(
            music,
            vec![
                MusicCmd::Play(ui::PlayRequest { uri: first_album_uri(), start: None }),
                MusicCmd::Play(ui::PlayRequest { uri: "library://album/3".into(), start: Some("library://track/7".into()) }),
            ]
        );
        assert_eq!(plays(&radio), [12]);
        assert!(!radio.contains(&RadioCmd::Stop), "{radio:?}");
    }

    /// The core with a debug tap: injected input takes the real path, and the stream shows it.
    #[tokio::test]
    async fn injected_input_takes_the_real_path_and_is_streamed() {
        let tap = Tap::new();
        let mut rx = tap.subscribe().unwrap();
        let (scene_tx, _scene_rx) = render::mailbox::channel();
        let (pc2_tx, mut pc2_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
        let link = Link::new(link::Config::new(LinkRole::SourceCenter));
        let core =
            Core::new(Ui::new(ui::fake::Library::generate()), NoPanel, NoAux, link, pc2_tx, scene_tx).with_tap(tap.clone());
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(core.run(ev_rx));

        let out = masterlink::Telegram::decode(&[0x80, 0xc2, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x11, 0x00, 0x01]).unwrap().encode();
        let incoming =
            masterlink::Telegram::decode(&[0xc2, 0xc1, 0x01, 0x0a, 0x7a, 0x00, 0x00, 0x14, 0x00, 0x04]).unwrap().encode();
        for e in [
            Event::Pc2Connected,
            Event::Debug(Inject::Panel { menu: 0, volume: 0, pointer: Some(40) }),
            Event::Debug(Inject::Panel { menu: 300, volume: 0, pointer: None }),
            Event::Debug(Inject::Button { button: Button::Go, down: true }),
            Event::Debug(Inject::Button { button: Button::Go, down: false }),
            Event::Debug(Inject::TelegramOut(out.clone())),
            Event::Debug(Inject::TelegramIn(incoming)),
            Event::Debug(Inject::Beo4 { kind: 0x01, code: pc2::beo4::N_MUSIC }),
            Event::Debug(Inject::Snapshot),
        ] {
            ev_tx.send(e).unwrap();
        }
        // let that batch finish (timing and state records follow it) before quitting
        tokio::time::sleep(Duration::from_millis(200)).await;
        ev_tx.send(Event::Quit).unwrap();
        tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();

        let mut by_kind: HashMap<String, Vec<Value>> = HashMap::new();
        while let Ok(l) = rx.try_recv() {
            let v: Value = serde_json::from_str(&l.json).unwrap();
            by_kind.entry(l.kind.to_string()).or_default().push(v);
        }
        let panel = &by_kind["panel_in"];
        // pointer move, three reports for 300 counts (127 + 127 + 46), GO down and up
        assert_eq!(panel.len(), 6, "{panel:?}");
        assert_eq!(panel[0]["pointer"], 40);
        assert_eq!(panel[1]["menu"], 127);
        assert_eq!(panel[3]["menu"], 46);
        assert_eq!(panel[3]["pointer"], 40, "a wheel turn keeps the pointer where it is");
        assert_eq!(panel[4]["buttons"], "40");
        assert_eq!(panel[5]["buttons"], "00");
        assert!(by_kind["panel_input"].iter().any(|v| v["input"] == "Pressed(Go)"));
        // the injected telegram went to the PC2 actor and into the stream as ml_out
        let mut sent = vec![];
        while let Ok(Pc2Cmd::Telegram(t)) | Ok(Pc2Cmd::Message(t)) = pc2_rx.try_recv() {
            sent.push(t);
        }
        assert!(sent.contains(&out));
        let ml_out = &by_kind["ml_out"];
        assert!(ml_out.iter().any(|v| v["payload_type"] == "STANDBY_1" && v["to"] == "ALL"), "{ml_out:?}");
        assert_eq!(by_kind["ml_in"][0]["type"], "COMMAND");
        assert_eq!(by_kind["ir"][0]["key"], "n.music");
        // the Beo4 key is ignored in the link source role (the room's master has its own eye),
        // but the snapshot comes either way
        let state = by_kind["state"].last().unwrap();
        assert_eq!(state["state"]["role"], "SourceCenter");
        assert_eq!(state["state"]["pc2"], true);
        assert!(by_kind.contains_key("timing"));
        assert!(by_kind["core_event"].iter().any(|v| v["event"] == "Pc2Connected"));
        assert!(by_kind["inject"].len() >= 8);
    }
}
