//! A.AUX: a USB sound card's line in, played through the BeoMaster 5.
//!
//! What the original software does (BeoSound5.exe, decompiled and annotated in the Ghidra
//! project; see `work/ghidra-annotations/bs5exe_aaux.tsv`):
//!
//! - Selecting A.AUX (`A18::AuxSource::OpenForPlayback`, source 8) starts USB playback unless a
//!   DLNA controller is pushing a stream to the BM5's renderer (the DMR half of A.AUX, not
//!   replicated here).
//! - USB playback (`NetworkPlayer::DirectShowEngine::PlayFromUsb`) is a DirectShow graph from
//!   the capture device "USB Audio CODEC" (the name a TI/Burr-Brown PCM2900-family USB sound
//!   card reports) straight to "DirectSound: HD Audio rear output", the output every BM5 source
//!   plays through. No processing, no track info ("TAG-info is send from AUX but not impl.").
//! - The device is found by the case-insensitive regex `USB\s+AUDIO\s+CODEC`.
//! - Plugging the card in only logs; unplugging it while A.AUX plays tells the source.
//!
//! On the link, A.AUX is ML source 0x97 when the BM5 is audio master and rides on the N.MUSIC
//! slot when it is a source center: see `masterlink::sources`.
//!
//! This crate keeps neomaster's split: [`AuxSource`] is sans-IO (state in, [`Effect`]s out),
//! [`device`] finds the card in `/proc/asound/cards`, and [`loopback::Loopback`] carries out the
//! effects by running `alsaloop` from alsa-utils.

pub mod device;
pub mod loopback;

pub use device::Card;

/// What the A.AUX source wants done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Start playing this ALSA capture device through the BM5's output.
    Start { capture: String },
    /// Stop the loop.
    Stop,
}

/// The A.AUX source: plays the USB sound card while A.AUX is selected and the card is there.
///
/// Unlike BeoSound5.exe, which tries the capture device once when A.AUX is selected and gives up
/// if it is missing, this follows the card: selecting A.AUX with no card plugged in waits for
/// it, and unplugging and replugging it while selected resumes playback.
#[derive(Debug, Default)]
pub struct AuxSource {
    selected: bool,
    card: Option<Card>,
    /// The card the loop was last told to capture from.
    playing: Option<Card>,
}

impl AuxSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// A.AUX was picked (panel, Beo4 A.AUX key, or a link room asking for it).
    pub fn select(&mut self) -> Vec<Effect> {
        self.selected = true;
        self.reconcile()
    }

    /// Another source was picked, or standby.
    pub fn deselect(&mut self) -> Vec<Effect> {
        self.selected = false;
        self.reconcile()
    }

    /// The USB sound card came, went or changed (from [`device::find_usb_audio_codec`]).
    pub fn set_card(&mut self, card: Option<Card>) -> Vec<Effect> {
        self.card = card;
        self.reconcile()
    }

    /// The loop ended by itself (the capture device failed). Starts it again if A.AUX is still
    /// selected and the card is still there; call it no more often than the card is polled.
    pub fn loop_ended(&mut self) -> Vec<Effect> {
        if self.playing.is_none() {
            return Vec::new();
        }
        self.playing = None;
        self.reconcile()
    }

    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// The loop is (meant to be) running.
    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    pub fn card(&self) -> Option<&Card> {
        self.card.as_ref()
    }

    fn reconcile(&mut self) -> Vec<Effect> {
        let want = if self.selected { self.card.clone() } else { None };
        if want == self.playing {
            return Vec::new();
        }
        let mut fx = Vec::new();
        if self.playing.is_some() {
            fx.push(Effect::Stop);
        }
        if let Some(card) = &want {
            fx.push(Effect::Start { capture: card.capture_device() });
        }
        self.playing = want;
        fx
    }
}
