//! The core: one task that owns all application state. Actors send it [`Event`]s; it updates
//! the state, carries out effects through the actors' channels, and hands a fresh scene to the
//! render thread. Nothing here blocks, sleeps or touches a device, so a run can be replayed from
//! its event log.

use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use bs5_panel::{Decoder, Output, Report};
use log::{debug, info};
use render::Scene;
use tokio::sync::mpsc;
use tokio::time::sleep_until;
use ui::{Effect, Ui};

/// Time since the core started.
pub type Time = Duration;

#[derive(Debug)]
pub enum Event {
    /// A raw IN report from the panel (or the simulator's stand-in).
    PanelReport([u8; bs5_panel::REPORT_LEN]),
    /// The keyboard's standby key (simulator only; the panel has none).
    Standby,
    /// The screen needs redrawing (window exposed).
    Redraw,
    Quit,
}

/// Where effects for the panel go: OUT reports for the click and backlight.
pub trait PanelOut: Send {
    fn write(&mut self, report: [u8; 2]);
}

pub struct Core<P: PanelOut> {
    ui: Ui,
    decoder: Decoder,
    panel: Output,
    panel_out: P,
    scenes: std_mpsc::SyncSender<Scene>,
    epoch: Instant,
}

impl<P: PanelOut> Core<P> {
    pub fn new(ui: Ui, panel_out: P, scenes: std_mpsc::SyncSender<Scene>) -> Self {
        Core { ui, decoder: Decoder::new(), panel: Output::new(), panel_out, scenes, epoch: Instant::now() }
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
                // (link source) once the role glue exists (M4/M5).
                Effect::Volume(v) => debug!("volume {v}"),
                Effect::Play(t) => info!("play {} track(s) (fake library; Music Assistant arrives in M3)", t.len()),
                Effect::Enqueue(t) => info!("enqueue {} track(s)", t.len()),
                Effect::SelectSource(s) => info!("select source {s}"),
                Effect::Standby => info!("standby"),
            }
        }
    }

    fn draw(&mut self, now: Time) {
        // A full channel means the render thread is behind; it only wants the newest scene anyway.
        let _ = self.scenes.try_send(self.ui.scene(now));
    }

    /// The next moment the core has to wake up without an event.
    fn deadline(&self, now: Time) -> Time {
        [self.ui.next_timer(), self.ui.next_frame(now), self.decoder.next_deadline()]
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
            tokio::select! {
                ev = events.recv() => {
                    let Some(ev) = ev else { return };
                    // Handle everything that is already queued, then draw once.
                    let mut batch = vec![ev];
                    while let Ok(ev) = events.try_recv() {
                        batch.push(ev);
                    }
                    let now = self.now();
                    for ev in batch {
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
                            Event::Redraw => {}
                            Event::Quit => return,
                        }
                    }
                }
                _ = sleep_until(wake) => {
                    let now = self.now();
                    for input in self.decoder.tick(now) {
                        let fx = self.ui.input(input, now);
                        self.effects(fx, now);
                    }
                    let fx = self.ui.tick(now);
                    self.effects(fx, now);
                }
            }
            self.draw(self.now());
        }
    }
}
