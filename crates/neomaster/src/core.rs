//! The core: one task that owns all application state. Actors send it [`Event`]s; it updates
//! the state, carries out effects through the actors' channels, and hands a fresh scene to the
//! render thread. Nothing here blocks, sleeps or touches a device, so a run can be replayed from
//! its event log.

use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use a_aux::AuxSource;
use bs5_panel::{Decoder, Output, Report};
use log::{debug, info};
use masterlink::{LinkRole, LocalSource};
use render::Scene;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::sleep_until;
use ui::{Effect, Ui};

use crate::debug::{self, Inject, Tap};
use crate::link::{self, AppEffect, Link, Outputs, Pc2Cmd};
use crate::radio_actor::{NoRadio, RadioCmd, RadioOut};

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
    /// The screen needs redrawing (window exposed).
    Redraw,
    /// The N.RADIO actor matched preset `number`'s name to this Music Assistant station.
    RadioResolved {
        number: u8,
        uri: String,
    },
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
}

impl AuxOut for a_aux::loopback::Loopback {
    fn apply(&mut self, effect: &a_aux::Effect) {
        a_aux::loopback::Loopback::apply(self, effect)
    }
    fn ended(&mut self) -> bool {
        a_aux::loopback::Loopback::ended(self)
    }
}

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
    link: Link,
    pc2_out: Q,
    scenes: std_mpsc::SyncSender<Scene>,
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
}

impl<P: PanelOut, A: AuxOut, Q: Pc2Out> Core<P, A, Q> {
    pub fn new(mut ui: Ui, panel_out: P, aux_out: A, link: Link, pc2_out: Q, scenes: std_mpsc::SyncSender<Scene>) -> Self {
        let mut link = link;
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
        }
    }

    /// N.RADIO's presets and where its commands go (the actor in [`crate::radio_actor`]).
    pub fn with_radio(mut self, tuner: radio::Tuner, out: impl RadioOut + 'static) -> Self {
        self.tuner = tuner;
        self.radio_out = Box::new(out);
        self
    }

    /// Send debug records here instead of to [`Tap::global`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_tap(mut self, tap: Tap) -> Self {
        self.tap = tap;
        self
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
        self.tap.emit("radio", || json!({ "preset": t.preset.number, "name": t.preset.name, "uri": t.preset.uri }));
        self.radio_out.send(RadioCmd::Play(t.preset));
        self.radio_save();
    }

    /// N.RADIO is no longer the source: stop its Music Assistant queue.
    fn radio_stop(&mut self) {
        if self.radio_playing != 0 {
            self.radio_playing = 0;
            self.tuner.stopped();
            self.radio_out.send(RadioCmd::Stop);
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
                        self.radio_stop();
                    }
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
                    self.radio_stop();
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

    fn now(&self) -> Time {
        self.epoch.elapsed()
    }

    fn panel_write(&mut self, report: [u8; 2]) {
        self.tap.emit("panel_out", || json!({ "raw": debug::hex(&report) }));
        self.panel_out.write(report);
    }

    fn effects(&mut self, fx: Vec<Effect>, now: Time) {
        for e in fx {
            self.tap.emit("ui_effect", || {
                json!({ "effect": match &e {
                    Effect::Play(t) => format!("Play({} tracks)", t.len()),
                    Effect::Enqueue(t) => format!("Enqueue({} tracks)", t.len()),
                    e => format!("{e:?}"),
                } })
            });
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
                // Blank-screen standby stops what plays locally. Music Assistant is not wired in
                // yet; A.AUX is.
                Effect::StopLocalPlayback => {
                    self.radio_stop();
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
                Effect::Play(t) => info!("play {} track(s) (fake library; Music Assistant arrives in M3)", t.len()),
                Effect::Enqueue(t) => info!("enqueue {} track(s)", t.len()),
                Effect::SelectSource(s) => {
                    info!("select source {s}");
                    if let Some(local) = link::local_source(s) {
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
        // A full channel means the render thread is behind; it only wants the newest scene anyway.
        let _ = self.scenes.try_send(self.ui.scene(now));
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
        [self.ui_deadline(now), self.link.next_deadline()]
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
            let started = Instant::now();
            let mut events_handled = 0;
            let redraw = tokio::select! {
                ev = events.recv() => {
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
                    ui_due
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

/// Look for the A.AUX card every [`AUX_POLL`] and tell the core. Ends when the core is gone.
pub async fn poll_aux_card(events: mpsc::UnboundedSender<Event>) {
    let mut interval = tokio::time::interval(AUX_POLL);
    loop {
        interval.tick().await;
        let card = tokio::task::spawn_blocking(a_aux::device::scan).await.unwrap_or(None);
        if events.send(Event::AuxCard(card)).is_err() {
            return;
        }
    }
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
        let (scene_tx, _scene_rx) = std_mpsc::sync_channel(64);
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

    /// The core with a debug tap: injected input takes the real path, and the stream shows it.
    #[tokio::test]
    async fn injected_input_takes_the_real_path_and_is_streamed() {
        let tap = Tap::new();
        let mut rx = tap.subscribe().unwrap();
        let (scene_tx, _scene_rx) = std_mpsc::sync_channel(64);
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
