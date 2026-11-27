//! Finding the A.AUX sound card.
//!
//! BeoSound5.exe (`NetworkPlayer_IsUsbAudioCodecPresent`, 0x0060e557) matches device names with
//! the boost regex `USB\s+AUDIO\s+CODEC` and flag 0x100000 (icase). "USB Audio CODEC" is the
//! USB product string of TI/Burr-Brown's PCM2900, PCM2902 and PCM2904 codecs, which most cheap
//! USB line-in adapters use. On Linux the card shows up in `/proc/asound/cards`.

use std::fmt;

/// An ALSA sound card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub index: u32,
    /// ALSA card id, e.g. "CODEC" (stable across replugging, unlike the index).
    pub id: String,
    /// Driver, e.g. "USB-Audio".
    pub driver: String,
    /// Short name, e.g. "USB Audio CODEC".
    pub name: String,
    /// The second line, e.g. "Burr-Brown from TI USB Audio CODEC at usb-0000:00:1d.0-1, full speed".
    pub long_name: String,
}

impl Card {
    /// The ALSA capture device for its line in. `plughw` converts the codec's native format
    /// (16-bit, 32/44.1/48 kHz) to whatever the loop asks for.
    pub fn capture_device(&self) -> String {
        format!("plughw:CARD={},DEV=0", self.id)
    }
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "card {} [{}] {}", self.index, self.id, self.name)
    }
}

/// BeoSound5.exe's test: does the name contain "USB", "AUDIO", "CODEC" separated by whitespace,
/// ignoring case (regex search, not a whole-string match)?
pub fn is_usb_audio_codec(name: &str) -> bool {
    let words: Vec<String> = name.split_whitespace().map(str::to_ascii_uppercase).collect();
    words.windows(3).any(|w| w[0].ends_with("USB") && w[1] == "AUDIO" && w[2].starts_with("CODEC"))
}

/// Parse `/proc/asound/cards`:
///
/// ```text
///  1 [CODEC          ]: USB-Audio - USB Audio CODEC
///                       Burr-Brown from TI USB Audio CODEC at usb-0000:00:1d.0-1, full speed
/// ```
pub fn parse_cards(text: &str) -> Vec<Card> {
    let mut cards: Vec<Card> = Vec::new();
    for line in text.lines() {
        if let Some(card) = parse_header(line) {
            cards.push(card);
        } else if let Some(card) = cards.last_mut() {
            if card.long_name.is_empty() && !line.trim().is_empty() {
                card.long_name = line.trim().to_string();
            }
        }
    }
    cards
}

fn parse_header(line: &str) -> Option<Card> {
    let (index, rest) = line.trim_start().split_once(' ')?;
    let index = index.parse().ok()?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (id, rest) = rest.split_once(']')?;
    let rest = rest.trim_start().strip_prefix(':')?;
    let (driver, name) = rest.split_once(" - ")?;
    Some(Card {
        index,
        id: id.trim().to_string(),
        driver: driver.trim().to_string(),
        name: name.trim().to_string(),
        long_name: String::new(),
    })
}

/// The first card that BeoSound5.exe would have taken as the A.AUX input.
pub fn find_usb_audio_codec(cards: &[Card]) -> Option<Card> {
    cards.iter().find(|c| is_usb_audio_codec(&c.name) || is_usb_audio_codec(&c.long_name)).cloned()
}

/// Which sound card A.AUX plays. B&O shipped a PCM2902 USB line-in adapter with the BM5, and
/// BeoSound5.exe only ever looked for that one; neomaster lets any capture card be used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CardSelector {
    /// BeoSound5.exe's rule: the first card named like "USB Audio CODEC".
    #[default]
    UsbAudioCodec,
    /// The card with this ALSA id (e.g. "CODEC", "EVO8"), or failing that the first whose name
    /// contains this text, ignoring case.
    Named(String),
}

impl CardSelector {
    /// `NEOMASTER_AUX_CARD`: unset or empty = [`CardSelector::UsbAudioCodec`], otherwise
    /// [`CardSelector::Named`].
    pub fn from_env() -> Self {
        Self::parse(&std::env::var("NEOMASTER_AUX_CARD").unwrap_or_default())
    }

    pub fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() {
            CardSelector::UsbAudioCodec
        } else {
            CardSelector::Named(value.to_string())
        }
    }

    pub fn select(&self, cards: &[Card]) -> Option<Card> {
        match self {
            CardSelector::UsbAudioCodec => find_usb_audio_codec(cards),
            CardSelector::Named(want) => {
                let lower = want.to_lowercase();
                let contains = |s: &str| s.to_lowercase().contains(&lower);
                cards
                    .iter()
                    .find(|c| c.id == *want)
                    .or_else(|| cards.iter().find(|c| contains(&c.name) || contains(&c.long_name)))
                    .cloned()
            }
        }
    }
}

/// Read `/proc/asound/cards` and pick the card. Missing file (no ALSA) = no card.
pub fn scan(selector: &CardSelector) -> Option<Card> {
    let text = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default();
    selector.select(&parse_cards(&text))
}
