//! The PC2 as a state machine: start-up sequence, incoming messages, outgoing pacing and the fan
//! loop. Port of libpc2 `PC2Device`, the dispatch in `PC2Device::process_message` and
//! `PC2Beolink::send_telegram`.

use std::collections::VecDeque;
use std::fmt;

use log::{debug, info, warn};

use crate::fan::FanControl;
use crate::frame::{self, FragmentAssembler, Wakeup};
use crate::pacer::TelegramPacer;
use crate::{Message, Time};

/// How the PC2 is configured at start-up (libpc2 `PC2Interface::address_mask_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    AudioMaster,
    /// A Beolink PC2 on a desktop PC.
    Beoport,
    Promisc,
    /// Provide sources for another audio master (as a BeoMaster 5 does).
    SourceCenter,
}

impl Mode {
    /// The node address sent as `80 <address> 00` when none is configured: audio master 0x01,
    /// Beoport 0x40, source center 0x29 (B&O's LastLegalAdr default).
    pub fn default_node_address(self) -> u8 {
        match self {
            Mode::Beoport => 0x40,
            Mode::SourceCenter => 0x29,
            Mode::AudioMaster | Mode::Promisc => 0x01,
        }
    }

    /// The F6 option telegram. BeoIpcML picks it from its SystemConfiguration (registry), 1..10:
    ///
    /// ```text
    /// 1: 02 C0 C1 80                 2: 01 C0 80 83               3: 00 82 80 83
    /// 4/5/6/8: 10 C1 80 83 <cfg> R C   7/9/10: 10 80 80 83 <cfg> R C
    /// ```
    ///
    /// where R = LCMLRerouteON and C = CompatabilityEnabled (registry, normally 0). Its own log
    /// names 4 "A.OPT=1 (A-master)", 5 "A.OPT=0 (A-master)", 6 "A.OPT=2 (AV-master)", 7
    /// "SL-source center", 8 "Compatability mode", 9 "A.OPT=5 (Linkroom A-master)", 10 "A.OPT=6
    /// (Linkroom AV-master)".
    pub fn option_message(self) -> Message {
        match self {
            Mode::AudioMaster => vec![0xf6, 0x10, 0xc1, 0x80, 0x83, 0x05, 0x00, 0x00],
            Mode::Beoport => vec![0xf6, 0x00, 0x82, 0x80, 0x83],
            Mode::SourceCenter => vec![0xf6, 0x10, 0x80, 0x80, 0x83, 0x07, 0x00, 0x00],
            Mode::Promisc => vec![0xf6, 0xc0, 0xc1, 0x80, 0x83, 0x05, 0x00, 0x00],
        }
    }
}

/// What the PC2 told us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A Masterlink telegram, from `dest` on (checksum and EOT included); decode it with
    /// `masterlink::Telegram::decode`.
    Masterlink(Vec<u8>),
    /// A Beo4 key from the PC2's IR eye. `kind` 0x0f means held.
    Beo4Key {
        kind: u8,
        code: u8,
    },
    /// The PC2 reported its tone settings (IN 03 / IN 1d).
    MixerState {
        volume: u8,
        loudness: bool,
        bass: i8,
        treble: i8,
        balance: i8,
    },
    /// IN 06 (dead in firmware V0236).
    Headphones(bool),
    /// IN 44 01: the firmware's lock manager holds the Masterlink bus lock for us.
    BusLockGranted,
    /// IN 1c: temperature (signed whole degrees C), every ~30 s. With host fan control on, the
    /// f9 answer is already queued.
    Temperature(i8),
    Wakeup(Wakeup),
    /// The PC2's ML-status replies carry our node address; BeoIpcML adopts it when it changes.
    NodeAddress(u8),
    /// Any other complete message, framing included.
    Other(Vec<u8>),
}

/// [`Pc2::send_telegram`] refused a telegram the PC2 firmware would silently drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub telegram_type: u8,
    pub payload_len: usize,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PC2 would refuse telegram type {:#04x} with a {}-byte payload", self.telegram_type, self.payload_len)
    }
}

impl std::error::Error for Refused {}

/// The PC2 firmware (0x3daa) silently refuses telegrams whose type is not one of these, or whose
/// payload is longer than 40 bytes, but still answers IN 01, so the refusal would go unnoticed.
pub const ALLOWED_TELEGRAM_TYPES: [u8; 13] = [0x0a, 0x0b, 0x0c, 0x14, 0x15, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x40, 0x5e];
pub const MAX_TELEGRAM_PAYLOAD: usize = 40;

pub struct Pc2 {
    mode: Mode,
    node_address: u8,
    /// Answer the PC2's temperature reports (IN 1c) with fan duty (f9), as B&O's software does.
    /// Without this the PC2 runs its fan output at ~94 % (BeoMaster 5).
    pub host_fan_control: bool,
    pub fan: FanControl,
    assembler: FragmentAssembler,
    pacer: TelegramPacer,
    out: VecDeque<Vec<u8>>,
}

impl Pc2 {
    /// `node_address` 0 = the mode's default.
    pub fn new(mode: Mode, node_address: u8) -> Self {
        Pc2 {
            mode,
            node_address: if node_address == 0 { mode.default_node_address() } else { node_address },
            host_fan_control: true,
            fan: FanControl::new(),
            assembler: FragmentAssembler::new(),
            pacer: TelegramPacer::default(),
            out: VecDeque::new(),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn node_address(&self) -> u8 {
        self.node_address
    }

    /// Start-up, as BeoIpcML.exe does it (static analysis, see reports/pc2-control.md): "F1";
    /// "ML START" `80 <node address> 00` (the address comes from the registry value LastLegalAdr,
    /// default 0x29, and is replaced by the address the PC2 reports in its ML-status replies);
    /// then "TRANSMIT OPTION", the F6 option telegram. libpc2 sends F1, 80 and F6 without waiting
    /// for the version reply; GET VERSION (29), E7 01 and 26 are not reproduced yet.
    pub fn init(&mut self, now: Time) {
        self.fan.skip_next();
        self.send(vec![0xf1], now);
        self.send(vec![0x80, self.node_address, 0x00], now);
        info!("Sending PC2 option for {:?} mode", self.mode);
        self.send(self.mode.option_message(), now);
    }

    /// Before closing the device (libpc2 `PC2Device::close`).
    pub fn close(&mut self, now: Time) {
        self.send(vec![0xa7], now);
    }

    /// Send a PC2 message. Masterlink telegrams (first byte 0xE0) are queued and sent one at a
    /// time, each after the PC2 has answered the previous one, as B&O's software does. Everything
    /// else goes out immediately.
    pub fn send(&mut self, message: Message, now: Time) {
        if message.first() == Some(&0xe0) {
            if let Some(m) = self.pacer.enqueue(message, now) {
                self.out.push_back(frame::encode(&m));
            }
        } else {
            self.out.push_back(frame::encode(&message));
        }
    }

    pub fn send_all(&mut self, messages: Vec<Message>, now: Time) {
        for m in messages {
            self.send(m, now);
        }
    }

    /// Send an encoded Masterlink telegram (from `dest`, checksum and EOT included).
    pub fn send_telegram(&mut self, telegram: &[u8], now: Time) -> Result<(), Refused> {
        let ttype = telegram.get(3).copied().unwrap_or(0);
        let payload_len = telegram.get(8).copied().unwrap_or(0) as usize;
        if !ALLOWED_TELEGRAM_TYPES.contains(&ttype) || payload_len > MAX_TELEGRAM_PAYLOAD {
            log::error!("Not sending Masterlink telegram the PC2 would refuse: type {ttype:#04x}, payload {payload_len} bytes");
            return Err(Refused { telegram_type: ttype, payload_len });
        }
        let mut m = Vec::with_capacity(telegram.len() + 1);
        m.push(0xe0);
        m.extend_from_slice(telegram);
        self.send(m, now);
        Ok(())
    }

    /// Ask the firmware for the Masterlink bus lock (24); it answers IN 44 01.
    pub fn request_bus_lock(&mut self, now: Time) {
        self.send(vec![0x24], now);
    }
    /// Release the bus lock (e4 01).
    pub fn release_bus_lock(&mut self, now: Time) {
        self.send(vec![0xe4, 0x01], now);
    }

    /// Call periodically: lets the next Masterlink telegram go if the PC2 never answered.
    pub fn tick(&mut self, now: Time) {
        if let Some(m) = self.pacer.tick(now) {
            self.out.push_back(frame::encode(&m));
        }
    }

    /// When [`Pc2::tick`] next has something to do, if ever.
    pub fn next_deadline(&self) -> Option<Time> {
        self.pacer.deadline()
    }

    /// The next USB packet to write to the OUT endpoint.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        self.out.pop_front()
    }

    /// Feed one IN packet. Malformed packets are logged and dropped (they used to end libpc2's
    /// event thread).
    pub fn on_usb_packet(&mut self, packet: &[u8], now: Time) -> Vec<Event> {
        if packet == frame::BOGUS_PACKET {
            debug!("Got bogus PC2 packet; ignoring");
            return vec![];
        }
        // Raw wake-up packets (no 60 ... 61 framing), only between messages
        if !self.assembler.in_progress() {
            if let Some(w) = frame::wakeup_packet(packet) {
                info!("PC2 wake-up: {w:?}");
                return vec![Event::Wakeup(w)];
            }
        }
        match self.assembler.push(packet) {
            Ok(Some(msg)) => self.on_message(&msg, now),
            Ok(None) => vec![],
            Err(e) => {
                warn!("Dropping malformed PC2 packet: {e}");
                vec![]
            }
        }
    }

    fn on_message(&mut self, msg: &[u8], now: Time) -> Vec<Event> {
        let mut events = Vec::new();
        if msg.len() < 3 {
            return events;
        }
        let kind = msg[2];
        // Type 0x01 is the PC2's status answer to a Masterlink telegram we sent
        if kind == 0x01 {
            if let Some(m) = self.pacer.acknowledge(now) {
                self.out.push_back(frame::encode(&m));
            }
        }
        // ML-status (object 01, e.g. "60 04 41 01 01 02 61"): byte 5 is our node address;
        // BeoIpcML adopts it when it is 1..0x7f
        if msg.len() >= 7 && kind & 0x1f == 0x01 && (0x01..=0x7f).contains(&msg[5]) && msg[5] != self.node_address {
            info!("PC2 reports Masterlink node address {:#04x}", msg[5]);
            self.node_address = msg[5];
            events.push(Event::NodeAddress(msg[5]));
        }
        match kind {
            0x00 if msg.len() >= 4 => events.push(Event::Masterlink(msg[3..msg.len() - 1].to_vec())),
            0x02 if msg.len() >= 7 => events.push(Event::Beo4Key { kind: msg[4], code: msg[6] }),
            0x03 | 0x1d if msg.len() >= 7 => events.push(Event::MixerState {
                volume: msg[3] & 0x7f,
                loudness: msg[3] & 0x80 != 0,
                bass: msg[4] as i8,
                treble: msg[5] as i8,
                balance: msg[6] as i8,
            }),
            0x06 if msg.len() >= 4 => events.push(Event::Headphones(msg[3] == 0x01)),
            0x44 => events.push(Event::BusLockGranted),
            0x1c if msg.len() >= 5 => {
                // IN 1c T ff: temperature (signed degrees C) every ~30 s; answer with the fan duty
                let celsius = msg[3] as i8;
                if self.host_fan_control {
                    if self.fan.consume_skip() {
                        // the first report after the host opens the device is a stale buffered packet
                        debug!("Ignoring first (stale) temperature report");
                    } else {
                        let duty = self.fan.on_temperature(celsius);
                        debug!("PC2 temperature {celsius} C -> fan duty {duty:#04x}");
                        self.send(vec![0xf9, duty], now);
                    }
                }
                events.push(Event::Temperature(celsius));
            }
            0x01 => {}
            _ => events.push(Event::Other(msg.to_vec())),
        }
        events
    }
}
