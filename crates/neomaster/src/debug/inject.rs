//! Commands a debug client sends, and what the core is asked to do about them.

use bs5_panel::{Button, REPORT_LEN};
use serde::Deserialize;

/// What reaches the core as [`crate::core::Event::Debug`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inject {
    /// A raw panel IN report, as if the panel sent it.
    PanelRaw([u8; REPORT_LEN]),
    /// Panel movement: the core fills in the pointer (unless given) and the buttons held, so a
    /// wheel turn does not also move the pointer.
    Panel { menu: i32, volume: i32, pointer: Option<u8> },
    /// A panel button goes down or up.
    Button { button: Button, down: bool },
    /// A Beo4 key from the PC2's IR eye (`kind` 0x0f = held).
    Beo4 { kind: u8, code: u8 },
    /// A telegram as if it came in from the link (encoded: checksum and EOT included).
    TelegramIn(Vec<u8>),
    /// A telegram to put on the link (encoded), through the PC2 actor's pacer.
    TelegramOut(Vec<u8>),
    /// A PC2 message (without the USB framing) to send to the PC2.
    Pc2Message(Vec<u8>),
    /// The simulator's standby key (this room).
    Standby,
    /// Long standby (everything on the link off).
    AllStandby,
    /// Emit a full `state` record now.
    Snapshot,
    /// Play N.RADIO preset N, as picking it on the panel will.
    RadioPreset(u8),
}

/// A number as JSON gives it, or a string like `"0x94"`.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Num {
    Int(i64),
    Str(String),
}

impl Num {
    fn byte(&self) -> Result<u8, String> {
        match self {
            Num::Int(i) => u8::try_from(*i).map_err(|_| format!("{i} is not a byte")),
            Num::Str(s) => super::parse_u8(s).ok_or_else(|| format!("{s:?} is not a byte")),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
}

/// One line from a client. JSON with a `cmd` field, e.g. `{"cmd":"panel","menu":15}`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Only send records of these kinds (`["*"]` or no `kinds`: everything but `usb_in` /
    /// `usb_out`; `["all"]`: everything; `[]`: nothing).
    Filter {
        #[serde(default)]
        kinds: Option<Vec<String>>,
    },
    Panel {
        #[serde(default)]
        menu: i32,
        #[serde(default)]
        volume: i32,
        #[serde(default)]
        pointer: Option<u8>,
    },
    PanelRaw {
        hex: String,
    },
    Button {
        button: String,
        down: bool,
    },
    /// `code`: a number or a key name (`"n.music"`); `held`: kind 0x0f.
    Beo4 {
        code: Num,
        #[serde(default)]
        kind: Option<Num>,
        #[serde(default)]
        held: bool,
    },
    /// A telegram, with or without checksum / EOT; it is re-encoded either way.
    Telegram {
        hex: String,
        dir: Direction,
    },
    Pc2 {
        hex: String,
    },
    Standby,
    AllStandby,
    State,
    /// N.RADIO preset `preset` (1-99), as if picked on the panel.
    Radio {
        preset: u8,
    },
    /// Put a note in the stream (and so in recordings), e.g. "the list froze here".
    Mark {
        text: String,
    },
    Ping,
}

pub fn button(name: &str) -> Result<Button, String> {
    match name.to_ascii_lowercase().as_str() {
        "go" => Ok(Button::Go),
        "left" => Ok(Button::Left),
        "right" => Ok(Button::Right),
        _ => Err(format!("no button {name:?} (go, left, right)")),
    }
}

/// A telegram typed by a person or copied from an MLGW log: normalised to the encoded form.
pub fn telegram(hex: &str) -> Result<Vec<u8>, String> {
    let bytes = super::parse_hex(hex)?;
    let t = masterlink::Telegram::decode(&bytes).map_err(|e| e.to_string())?;
    Ok(t.encode())
}

impl Command {
    /// What the core should do, or `None` for commands the server handles itself.
    pub fn to_inject(&self) -> Result<Option<Inject>, String> {
        Ok(Some(match self {
            Command::Filter { .. } | Command::Mark { .. } | Command::Ping => return Ok(None),
            Command::Panel { menu, volume, pointer } => {
                if pointer.is_some_and(|p| p > bs5_panel::POINTER_MAX) {
                    return Err(format!("pointer runs 0-{}", bs5_panel::POINTER_MAX));
                }
                Inject::Panel { menu: *menu, volume: *volume, pointer: *pointer }
            }
            Command::PanelRaw { hex } => {
                let b = super::parse_hex(hex)?;
                if b.len() < 4 || b.len() > REPORT_LEN {
                    return Err(format!("a panel report is 4 to {REPORT_LEN} bytes"));
                }
                let mut r = [0, 0, 0, 0, 0xc0, b[2]];
                r[..b.len()].copy_from_slice(&b);
                Inject::PanelRaw(r)
            }
            Command::Button { button: b, down } => Inject::Button { button: button(b)?, down: *down },
            Command::Beo4 { code, kind, held } => {
                let code = match code {
                    Num::Str(s) => super::beo4_code(s).ok_or_else(|| format!("no Beo4 key {s:?}"))?,
                    n => n.byte()?,
                };
                let kind = match kind {
                    Some(k) => k.byte()?,
                    None if *held => pc2::beo4::TYPE_HELD,
                    None => 0x01,
                };
                Inject::Beo4 { kind, code }
            }
            Command::Telegram { hex, dir: Direction::In } => Inject::TelegramIn(telegram(hex)?),
            Command::Telegram { hex, dir: Direction::Out } => Inject::TelegramOut(telegram(hex)?),
            Command::Pc2 { hex } => {
                let b = super::parse_hex(hex)?;
                if b.is_empty() {
                    return Err("empty PC2 message".into());
                }
                Inject::Pc2Message(b)
            }
            Command::Standby => Inject::Standby,
            Command::AllStandby => Inject::AllStandby,
            Command::State => Inject::Snapshot,
            Command::Radio { preset } => {
                if !(1..=radio::MAX_NUMBER).contains(preset) {
                    return Err(format!("presets run 1-{}", radio::MAX_NUMBER));
                }
                Inject::RadioPreset(*preset)
            }
        }))
    }

    /// Commands that change something; refused over TCP unless injection is allowed there.
    pub fn injects(&self) -> bool {
        !matches!(self, Command::Filter { .. } | Command::Mark { .. } | Command::Ping | Command::State)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Option<Inject>, String> {
        serde_json::from_str::<Command>(s).map_err(|e| e.to_string())?.to_inject()
    }

    #[test]
    fn commands_become_core_injections() {
        assert_eq!(parse(r#"{"cmd":"panel","menu":15}"#), Ok(Some(Inject::Panel { menu: 15, volume: 0, pointer: None })));
        assert_eq!(parse(r#"{"cmd":"panel","pointer":40}"#), Ok(Some(Inject::Panel { menu: 0, volume: 0, pointer: Some(40) })));
        assert!(parse(r#"{"cmd":"panel","pointer":200}"#).is_err());
        assert_eq!(
            parse(r#"{"cmd":"panel_raw","hex":"0f 00 28 40"}"#),
            Ok(Some(Inject::PanelRaw([0x0f, 0x00, 0x28, 0x40, 0xc0, 0x28])))
        );
        assert_eq!(
            parse(r#"{"cmd":"button","button":"GO","down":true}"#),
            Ok(Some(Inject::Button { button: Button::Go, down: true }))
        );
        assert_eq!(parse(r#"{"cmd":"beo4","code":"n.music"}"#), Ok(Some(Inject::Beo4 { kind: 1, code: 0x94 })));
        assert_eq!(parse(r#"{"cmd":"beo4","code":12,"held":true}"#), Ok(Some(Inject::Beo4 { kind: 0x0f, code: 0x0c })));
        assert_eq!(parse(r#"{"cmd":"beo4","code":"0x0c","kind":"0x0f"}"#), Ok(Some(Inject::Beo4 { kind: 0x0f, code: 0x0c })));
        assert!(parse(r#"{"cmd":"beo4","code":"nope"}"#).is_err());
        assert_eq!(parse(r#"{"cmd":"mark","text":"x"}"#), Ok(None));
        assert!(parse(r#"{"cmd":"warp"}"#).is_err());
        assert!(parse(r#"{"cmd":"panel","menu":1,"bogus":2}"#).is_err());
    }

    #[test]
    fn telegrams_in_mlgw_form_get_their_checksum() {
        // the MLGW form (no checksum / EOT) and the encoded form give the same telegram
        let mlgw = "c0 c1 01 0b 00 00 00 04 03 04 01 01 00";
        let encoded = vec![0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9a, 0x00];
        let cmd = format!(r#"{{"cmd":"telegram","dir":"out","hex":"{mlgw}"}}"#);
        assert_eq!(parse(&cmd), Ok(Some(Inject::TelegramOut(encoded.clone()))));
        let cmd = format!(r#"{{"cmd":"telegram","dir":"in","hex":"{}"}}"#, super::super::hex(&encoded));
        assert_eq!(parse(&cmd), Ok(Some(Inject::TelegramIn(encoded))));
        assert!(parse(r#"{"cmd":"telegram","dir":"in","hex":"c0 c1"}"#).is_err());
    }

    #[test]
    fn only_changes_count_as_injection() {
        let c: Command = serde_json::from_str(r#"{"cmd":"state"}"#).unwrap();
        assert!(!c.injects());
        let c: Command = serde_json::from_str(r#"{"cmd":"standby"}"#).unwrap();
        assert!(c.injects());
    }
}
