//! What AudioMaster and SourceCenter need from their surroundings (libpc2 `AudioMasterHost`).
//!
//! The state machines never touch a device. Each call takes a `&mut dyn Host`; whatever they do
//! comes out through it. Tests implement it to record telegrams; neomaster's core uses [`Effects`]
//! to collect them as values and hands them to the PC2 actor.

use crate::protocol::clock::LocalTime;
use crate::telegram::Telegram;

pub trait Host {
    /// Send a Masterlink telegram (encode it with [`Telegram::encode`]: checksum and EOT included).
    fn send_telegram(&mut self, tgram: Telegram);
    /// Distribute our audio on the Masterlink audio bus (PC2 mixer: ml_distribute).
    fn set_ml_distribution(&mut self, on: bool);
    /// Play audio arriving over Masterlink locally (PC2 mixer: transmit_from_ml).
    fn set_ml_listening(&mut self, on: bool);
    /// A link room wants source `src`. Start it and return true, or return false if we can't.
    fn start_source(&mut self, src: u8) -> bool;
    /// Nobody (link rooms) needs our source any more.
    fn stop_source(&mut self);
    /// A Beo4 key aimed at our source from a link room (SOURCE_CONTROL telegram).
    fn source_key(&mut self, _src: u8, _key: u8) {}
    /// Someone sent "all standby" on the link.
    fn link_standby(&mut self) {}
    /// Ask the PC2 firmware for the Masterlink bus lock (host command 24). The firmware runs the
    /// LOCK_MANAGER protocol itself and answers IN 44 01 when granted, which the caller passes
    /// to `AudioMaster::bus_lock_granted()`.
    fn request_bus_lock(&mut self) {}
    /// Release the bus lock early (host command e4 01) instead of letting the firmware's hold expire.
    fn release_bus_lock(&mut self) {}
    /// Local time for the clock broadcast.
    fn local_time(&mut self) -> LocalTime;
}

/// Everything a state machine asked for, as a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Send(Telegram),
    MlDistribution(bool),
    MlListening(bool),
    /// A link room asked for this source and [`Effects::accept_source`] accepted it: start it.
    StartSource(u8),
    StopSource,
    SourceKey {
        src: u8,
        key: u8,
    },
    LinkStandby,
    RequestBusLock,
    ReleaseBusLock,
}

/// A [`Host`] that records [`Effect`]s. `accept_source` answers [`Host::start_source`]
/// synchronously (e.g. "is this one of the sources Music Assistant can play?").
pub struct Effects<F: FnMut(u8) -> bool> {
    pub out: Vec<Effect>,
    pub accept_source: F,
    pub time: LocalTime,
}

impl<F: FnMut(u8) -> bool> Effects<F> {
    pub fn new(accept_source: F, time: LocalTime) -> Self {
        Effects { out: Vec::new(), accept_source, time }
    }
}

impl<F: FnMut(u8) -> bool> Host for Effects<F> {
    fn send_telegram(&mut self, tgram: Telegram) {
        self.out.push(Effect::Send(tgram));
    }
    fn set_ml_distribution(&mut self, on: bool) {
        self.out.push(Effect::MlDistribution(on));
    }
    fn set_ml_listening(&mut self, on: bool) {
        self.out.push(Effect::MlListening(on));
    }
    fn start_source(&mut self, src: u8) -> bool {
        let ok = (self.accept_source)(src);
        if ok {
            self.out.push(Effect::StartSource(src));
        }
        ok
    }
    fn stop_source(&mut self) {
        self.out.push(Effect::StopSource);
    }
    fn source_key(&mut self, src: u8, key: u8) {
        self.out.push(Effect::SourceKey { src, key });
    }
    fn link_standby(&mut self) {
        self.out.push(Effect::LinkStandby);
    }
    fn request_bus_lock(&mut self) {
        self.out.push(Effect::RequestBusLock);
    }
    fn release_bus_lock(&mut self) {
        self.out.push(Effect::ReleaseBusLock);
    }
    fn local_time(&mut self) -> LocalTime {
        self.time
    }
}
