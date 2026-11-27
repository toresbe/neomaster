//! AirPlay: a phone or computer streams to the BeoMaster 5, which takes it as a source.
//!
//! shairport-sync (AirPlay 1, "classic") does the receiving and plays the audio through ALSA;
//! neomaster runs it as a child process ([`receiver`]) and follows its metadata pipe
//! ([`metadata`]). [`Session`] turns the pipe's items into what the core needs to know:
//! a device started streaming ([`Event::Started`]), what plays ([`Event::Track`],
//! [`Event::Cover`]), pause and resume, and that the device is gone ([`Event::Ended`]). The
//! core then takes the source on the link (role-dependent, see `masterlink::sources`), shows the
//! track on the panel, sends it to link rooms, and releases the source afterwards.
//!
//! When the user picks another source, the core tells the receiver to [`Effect::Drop`] the
//! session: shairport-sync is restarted, which ends the sender's connection (the phone shows the
//! BM5 as no longer playing), as the stock BeoSound5.exe stopped a DLNA push when A.AUX was left.
//!
//! What the pipe carries, as captured from Music Assistant's AirPlay sender against
//! shairport-sync 3.3.8 (bookworm i386), one play / next / pause / play / stop:
//!
//! ```text
//! ssnc snua "iTunes/7.6.2 (Windows; N;)"   ssnc clip "192.168.2.238"   ssnc pbeg
//! ssnc pvol "-24.00,0.00,0.00,0.00"        ssnc mdst
//! core minm "All of Me"  core asar "Django Reinhardt"  core asal "Django Reinhardt: ..."
//! ssnc mden   ssnc pcst  ssnc PICT <115012 bytes JPEG>  ssnc pcen   ssnc prsm
//! ... next track: mdst minm asar asal mden pcst PICT pcen
//! ... pause: pfls   play: pffr prsm   stop: pfls pend
//! ```
//!
//! Like the other sources this crate is sans-IO: items and `now` in, events out.

pub mod cover;
pub mod metadata;
pub mod receiver;

use std::time::Duration;

pub use metadata::{FourCc, Item, Parser};

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = Duration;

/// The item codes [`Session`] acts on.
pub mod code {
    use super::FourCc;
    /// shairport-sync's own items
    pub const SSNC: FourCc = FourCc::new(b"ssnc");
    /// DAAP fields from the sender
    pub const CORE: FourCc = FourCc::new(b"core");

    /// A play session begins (a sender connected and set up a stream).
    pub const PBEG: FourCc = FourCc::new(b"pbeg");
    /// The play session ended (the sender disconnected or stopped).
    pub const PEND: FourCc = FourCc::new(b"pend");
    /// Flush: the sender paused or skipped.
    pub const PFLS: FourCc = FourCc::new(b"pfls");
    /// Resume after a flush.
    pub const PRSM: FourCc = FourCc::new(b"prsm");
    /// Metadata bundle start and end.
    pub const MDST: FourCc = FourCc::new(b"mdst");
    pub const MDEN: FourCc = FourCc::new(b"mden");
    /// Cover art (JPEG or PNG; empty: none).
    pub const PICT: FourCc = FourCc::new(b"PICT");
    /// The sender's name (AirPlay 2 and iOS senders; Music Assistant sends none).
    pub const SNAM: FourCc = FourCc::new(b"snam");
    /// The sender's user agent, e.g. "AirPlay/600.8.41" or "iTunes/7.6.2 (Windows; N;)".
    pub const SNUA: FourCc = FourCc::new(b"snua");
    /// The sender's IP address.
    pub const CLIP: FourCc = FourCc::new(b"clip");
    /// The sender's volume: "airplay,volume,lowest,highest" in dB.
    pub const PVOL: FourCc = FourCc::new(b"pvol");

    pub const MINM: FourCc = FourCc::new(b"minm"); // title
    pub const ASAR: FourCc = FourCc::new(b"asar"); // artist
    pub const ASAL: FourCc = FourCc::new(b"asal"); // album
    pub const ASGN: FourCc = FourCc::new(b"asgn"); // genre
}

/// What plays, as the sender describes it. Empty strings: the sender did not say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
}

/// Who is streaming.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sender {
    /// The device's name (`snam`), when it sends one.
    pub name: Option<String>,
    /// Its address (`clip`).
    pub address: Option<String>,
    /// Its user agent (`snua`).
    pub user_agent: Option<String>,
}

impl Sender {
    /// A name for the log and the panel.
    pub fn label(&self) -> String {
        self.name.clone().or_else(|| self.address.clone()).unwrap_or_else(|| "an AirPlay device".into())
    }
}

/// What the core is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A device started streaming: take the source.
    Started(Sender),
    /// The track changed (sent at the end of each metadata bundle that changed it).
    Track(Track),
    /// New cover art for the current track (encoded; empty when the sender has none).
    Cover(Vec<u8>),
    /// The sender paused (or is skipping; a resume follows a skip at once).
    Paused,
    Resumed,
    /// The device stopped streaming and did not come back within [`Session::END_GRACE`]:
    /// release the source.
    Ended,
}

/// What the core asks of the receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// End the current session (another source was picked, or standby): restart
    /// shairport-sync, which disconnects the sender.
    Drop,
}

/// The state of AirPlay reception, from the metadata pipe; see the crate docs.
#[derive(Debug, Default)]
pub struct Session {
    active: bool,
    paused: bool,
    sender: Sender,
    track: Track,
    /// The fields of the metadata bundle being received.
    pending: Track,
    /// `pend` arrived at this time; [`Event::Ended`] goes out at `+ END_GRACE` unless a new
    /// session begins first.
    ending: Option<Time>,
}

impl Session {
    /// How long after `pend` the source is released. A sender that reconnects at once (some do
    /// between tracks or when switching outputs) keeps it.
    pub const END_GRACE: Duration = Duration::from_secs(3);

    pub fn new() -> Self {
        Self::default()
    }

    /// A device is streaming (from `pbeg` until [`Event::Ended`]).
    pub fn is_active(&self) -> bool {
        self.active
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn sender(&self) -> &Sender {
        &self.sender
    }
    pub fn track(&self) -> &Track {
        &self.track
    }

    /// One item from the pipe.
    pub fn item(&mut self, item: &Item, now: Time) -> Vec<Event> {
        let mut ev = Vec::new();
        if item.kind == code::CORE {
            let text = item.text();
            match item.code {
                code::MINM => self.pending.title = text,
                code::ASAR => self.pending.artist = text,
                code::ASAL => self.pending.album = text,
                code::ASGN => self.pending.genre = text,
                _ => {}
            }
            return ev;
        }
        if item.kind != code::SSNC {
            return ev;
        }
        match item.code {
            // Who is calling comes just before pbeg; a new sender starts a new description.
            code::SNUA => self.sender.user_agent = Some(item.text()),
            code::CLIP => self.sender.address = Some(item.text()),
            code::SNAM => self.sender.name = Some(item.text()),
            code::PBEG => self.begin(&mut ev),
            // Metadata or audio without pbeg: the pipe was opened mid-session.
            code::PRSM | code::MDEN if !self.active => {
                self.begin(&mut ev);
                self.continue_item(item, &mut ev);
            }
            code::PEND => {
                self.ending = Some(now);
                self.paused = false;
                // the next session describes its sender before its pbeg
                self.sender = Sender::default();
            }
            _ => self.continue_item(item, &mut ev),
        }
        ev
    }

    fn begin(&mut self, ev: &mut Vec<Event>) {
        self.ending = None;
        self.paused = false;
        if !self.active {
            self.active = true;
            self.track = Track::default();
            ev.push(Event::Started(self.sender.clone()));
        }
    }

    fn continue_item(&mut self, item: &Item, ev: &mut Vec<Event>) {
        match item.code {
            code::MDST => self.pending = Track::default(),
            code::MDEN => {
                let t = std::mem::take(&mut self.pending);
                if self.active && t != self.track {
                    self.track = t.clone();
                    ev.push(Event::Track(t));
                }
            }
            code::PICT if self.active => ev.push(Event::Cover(item.data.clone())),
            code::PFLS if self.active && !self.paused => {
                self.paused = true;
                ev.push(Event::Paused);
            }
            code::PRSM if self.active && self.paused => {
                self.paused = false;
                ev.push(Event::Resumed);
            }
            _ => {}
        }
    }

    /// The receiver was restarted or died: whatever played is gone, at once.
    pub fn receiver_restarted(&mut self) -> Vec<Event> {
        self.ending = None;
        self.end()
    }

    fn end(&mut self) -> Vec<Event> {
        if !std::mem::take(&mut self.active) {
            return Vec::new();
        }
        self.paused = false;
        self.track = Track::default();
        vec![Event::Ended]
    }

    pub fn tick(&mut self, now: Time) -> Vec<Event> {
        match self.ending {
            Some(t) if now >= t + Self::END_GRACE => {
                self.ending = None;
                self.end()
            }
            _ => Vec::new(),
        }
    }

    pub fn next_deadline(&self) -> Option<Time> {
        self.ending.map(|t| t + Self::END_GRACE)
    }
}
