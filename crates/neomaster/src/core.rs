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
use tokio::sync::mpsc;
use tokio::time::sleep_until;
use ui::{Effect, Ui};

use crate::link::{self, AppEffect, Link, Outputs, Pc2Cmd};

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

    /// Carry out what the link glue decided: PC2 messages to the actor, player effects here.
    fn link_out(&mut self, out: Outputs, now: Time) {
        for cmd in out.pc2 {
            self.pc2_out.send(cmd);
        }
        for e in out.app {
            match e {
                // A.AUX plays the USB card; Music Assistant (N.MUSIC, N.RADIO) is not wired in yet.
                AppEffect::Play(s) => {
                    info!("play {}", link::display_name(s));
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
                    let fx = self.aux.deselect();
                    self.aux_effects(fx);
                }
                AppEffect::SourceKey { source, key } => info!("{}: Beo4 key {key:#04x}", link::display_name(source)),
                AppEffect::SourceChannel { source, channel } => info!("{}: channel {channel}", link::display_name(source)),
                AppEffect::Standby => {
                    let fx = self.ui.blank();
                    self.effects(fx, now);
                }
                AppEffect::Volume(v) => self.ui.set_volume(v),
            }
        }
    }

    fn now(&self) -> Time {
        self.epoch.elapsed()
    }

    fn effects(&mut self, fx: Vec<Effect>, now: Time) {
        for e in fx {
            match e {
                Effect::Click => {
                    if let Some(r) = self.panel.click(now) {
                        self.panel_out.write(r);
                    }
                }
                Effect::Backlight(on) => {
                    let r = if on { self.panel.wake() } else { self.panel.blank() };
                    self.panel_out.write(r);
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
        [self.ui.next_timer(), self.ui.next_frame(now), self.decoder.next_deadline()].into_iter().flatten().min()
    }

    /// Handle one event. Returns false if the screen cannot have changed.
    fn event(&mut self, ev: Event, now: Time) -> bool {
        match ev {
            Event::PanelReport(bytes) => {
                if let Some(report) = Report::parse(&bytes) {
                    for input in self.decoder.feed(report, now) {
                        let fx = self.ui.input(input, now);
                        self.effects(fx, now);
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
                let out = self.link.pc2_event(ev, now);
                let visible = !out.app.is_empty();
                self.link_out(out, now);
                return visible;
            }
            Event::Pc2Connected => {
                let out = self.link.start(now);
                self.link_out(out, now);
                return false;
            }
            Event::Pc2Disconnected => {
                info!("PC2 gone; Masterlink paused until it is back");
                self.link.stopped();
                return false;
            }
            Event::AuxCard(card) => {
                self.aux_card(card);
                return false;
            }
            Event::Redraw => {}
            Event::Quit => {}
        }
        true
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
                    for ev in batch {
                        if matches!(ev, Event::Quit) {
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
                        let fx = self.ui.input(input, now);
                        self.effects(fx, now);
                    }
                    let fx = self.ui.tick(now);
                    self.effects(fx, now);
                    let out = self.link.tick(now);
                    self.link_out(out, now);
                    ui_due
                }
            };
            if redraw {
                self.draw(self.now());
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
