//! A simulated PC2: answers OUT messages the way the real one does in the USB captures, and
//! can inject IR keys and Masterlink telegrams as IN packets. Used by `--sim`, the actor tests
//! and the capture replays.
//!
//! What it reproduces (raw USB captures in the rea-local workspace, `work/raw_*.txt`):
//! - every Masterlink telegram (`e0 ...`) is answered with the ML status `60 04 01 01 01 <node> 61`;
//! - "ML START" `80 <node> 00` is answered with `60 04 41 01 01 <node> 61`;
//! - the bus lock request `24` is granted with `60 02 44 01 61` (the firmware's LOCK_MANAGER
//!   round on the link is not simulated);
//! - volume (`eb`) and tone (`e3`) changes are reported back as IN 03;
//! - IR keys arrive as `60 05 02 00 <type> cb <code> 61`;
//! - IN messages longer than the 9-byte interrupt packet arrive split over several packets.

use std::collections::VecDeque;

use tokio::sync::mpsc;

use crate::pc2_actor::UsbPipes;

/// The interrupt endpoints' wMaxPacketSize (lsusb on the BM5).
pub const PACKET_SIZE: usize = 9;

pub struct SimPc2 {
    node: u8,
    volume: u8,
    bass: i8,
    treble: i8,
    balance: i8,
    loudness: bool,
    /// Grant the bus lock when asked (`24`).
    pub grant_lock: bool,
    /// Answer Masterlink telegrams with ML status.
    pub ack_telegrams: bool,
    /// Every complete OUT message, framing removed.
    pub received: Vec<Vec<u8>>,
    pending: VecDeque<Vec<u8>>,
}

impl Default for SimPc2 {
    fn default() -> Self {
        SimPc2 {
            node: 0x01,
            volume: 0,
            bass: 0,
            treble: 0,
            balance: 0,
            loudness: false,
            grant_lock: true,
            ack_telegrams: true,
            received: Vec::new(),
            pending: VecDeque::new(),
        }
    }
}

impl SimPc2 {
    pub fn new() -> Self {
        Self::default()
    }

    /// Masterlink telegrams we were asked to send (encoded, checksum and EOT included).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn telegrams(&self) -> Vec<Vec<u8>> {
        self.received.iter().filter(|m| m.first() == Some(&0xe0)).map(|m| m[1..].to_vec()).collect()
    }

    /// Everything that is not a Masterlink telegram.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn commands(&self) -> Vec<Vec<u8>> {
        self.received.iter().filter(|m| m.first() != Some(&0xe0)).cloned().collect()
    }

    /// The IN packets to deliver next.
    pub fn poll_in(&mut self) -> Option<Vec<u8>> {
        self.pending.pop_front()
    }

    fn queue_message(&mut self, msg: &[u8]) {
        let framed = pc2::frame::encode(msg);
        for chunk in framed.chunks(PACKET_SIZE) {
            self.pending.push_back(chunk.to_vec());
        }
    }

    /// One OUT transfer from the host (one framed message).
    pub fn on_out(&mut self, packet: &[u8]) {
        let [0x60, len, ref rest @ ..] = *packet else { return };
        let Some(msg) = rest.get(..len as usize) else { return };
        let msg = msg.to_vec();
        self.received.push(msg.clone());
        match msg.as_slice() {
            [0xe0, ..] if self.ack_telegrams => self.queue_message(&[0x01, 0x01, 0x01, self.node]),
            [0x80, node, ..] => {
                self.node = *node;
                self.queue_message(&[0x41, 0x01, 0x01, self.node]);
            }
            [0x24] if self.grant_lock => self.queue_message(&[0x44, 0x01]),
            [0xeb, v] => {
                match *v {
                    0x80 => self.volume = (self.volume + 1).min(pc2::mixer::MAX_VOLUME),
                    0x81 => self.volume = self.volume.saturating_sub(1),
                    v => self.volume = v.min(pc2::mixer::MAX_VOLUME),
                }
                self.report_mixer();
            }
            [0xe3, v, b, t, bal] => {
                self.volume = v & 0x7f;
                self.loudness = v & 0x80 != 0;
                (self.bass, self.treble, self.balance) = (*b as i8, *t as i8, *bal as i8);
                self.report_mixer();
            }
            _ => {}
        }
    }

    fn report_mixer(&mut self) {
        let v = self.volume | if self.loudness { 0x80 } else { 0 };
        self.queue_message(&[0x03, v, self.bass as u8, self.treble as u8, self.balance as u8]);
    }

    /// A key on the PC2's IR eye (`kind` 0x01 short, 0x0f held).
    pub fn ir(&mut self, kind: u8, code: u8) {
        self.queue_message(&[0x02, 0x00, kind, 0xcb, code]);
    }

    /// A Masterlink telegram from the link (encoded, checksum and EOT included).
    pub fn telegram(&mut self, telegram: &[u8]) {
        let mut m = vec![0x00];
        m.extend_from_slice(telegram);
        self.queue_message(&m);
    }

    /// IN 1c: temperature report.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn temperature(&mut self, celsius: i8) {
        self.queue_message(&[0x1c, celsius as u8, 0xff]);
    }
}

/// Things a test (or `--sim`) can make the simulated PC2 do.
/// (Only the tests send these so far; `--sim` could bind keys to them.)
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum SimInput {
    Ir {
        kind: u8,
        code: u8,
    },
    Telegram(Vec<u8>),
    /// Unplug: the actor sees the device go away.
    Unplug,
}

/// Run a [`SimPc2`] as a device: returns the pipes the actor talks to and a sender for
/// [`SimInput`]. `observer` gets every OUT message (framing removed).
pub fn spawn(observer: Option<mpsc::UnboundedSender<Vec<u8>>>) -> (UsbPipes, mpsc::UnboundedSender<SimInput>) {
    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(64);
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    let (sim_tx, mut sim_rx) = mpsc::unbounded_channel::<SimInput>();
    tokio::spawn(async move {
        let mut pc2 = SimPc2::new();
        // Nobody holding the input sender is not an unplug: the device just runs on.
        let mut inputs_open = true;
        loop {
            tokio::select! {
                out = out_rx.recv() => {
                    let Some(out) = out else { return };
                    pc2.on_out(&out);
                    if let (Some(obs), Some(m)) = (&observer, pc2.received.last()) {
                        let _ = obs.send(m.clone());
                    }
                }
                input = sim_rx.recv(), if inputs_open => match input {
                    Some(SimInput::Ir { kind, code }) => pc2.ir(kind, code),
                    Some(SimInput::Telegram(t)) => pc2.telegram(&t),
                    Some(SimInput::Unplug) => return,
                    None => inputs_open = false,
                },
            }
            while let Some(p) = pc2.poll_in() {
                if in_tx.send(p).await.is_err() {
                    return;
                }
            }
        }
    });
    (UsbPipes { rx: in_rx, tx: out_tx }, sim_tx)
}
