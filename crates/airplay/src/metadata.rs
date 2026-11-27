//! shairport-sync's metadata pipe.
//!
//! With `metadata.enabled = "yes"` shairport-sync writes every piece of metadata it gets or
//! makes up to a FIFO, one item at a time, as XML-ish text (it is not well-formed XML: there is
//! no root, and items follow each other forever):
//!
//! ```text
//! <item><type>73736e63</type><code>70626567</code><length>0</length></item>
//! <item><type>636f7265</type><code>6d696e6d</code><length>9</length>
//! <data encoding="base64">
//! QWxsIG9mIE1l</data></item>
//! ```
//!
//! `type` and `code` are four ASCII characters in hex (`ssnc` `pbeg`, `core` `minm`), `length`
//! is the decoded data length, and the data, when there is any, is base64 on one line. `core`
//! items are the DAAP fields the sender sent (`minm` title, `asar` artist, ...); `ssnc` items are
//! shairport-sync's own (`pbeg` play begins, `PICT` cover art, ...). See [`crate::code`].
//!
//! [`Parser`] takes the bytes as they come off the pipe, in chunks of any size, and returns the
//! complete items. It skips anything it cannot read up to the next `<item>`, so a reader that
//! starts in the middle of an item (opening the FIFO late) resynchronises by itself.

use std::fmt;

use base64::Engine;

/// A four-character code (`ssnc`, `pbeg`, ...).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    pub const fn new(s: &[u8; 4]) -> Self {
        FourCc(*s)
    }

    fn from_hex(hex: &str) -> Option<Self> {
        let n = u32::from_str_radix(hex.trim(), 16).ok()?;
        Some(FourCc(n.to_be_bytes()))
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// One metadata item.
#[derive(Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: FourCc,
    pub code: FourCc,
    pub data: Vec<u8>,
}

impl Item {
    pub fn new(kind: &[u8; 4], code: &[u8; 4], data: impl Into<Vec<u8>>) -> Self {
        Item { kind: FourCc(*kind), code: FourCc(*code), data: data.into() }
    }

    /// The data as text (DAAP strings are UTF-8).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }

    /// The item as shairport-sync writes it to the pipe (for tests and replays).
    pub fn encode(&self) -> String {
        let head = format!(
            "<item><type>{:08x}</type><code>{:08x}</code><length>{}</length>",
            u32::from_be_bytes(self.kind.0),
            u32::from_be_bytes(self.code.0),
            self.data.len()
        );
        if self.data.is_empty() {
            format!("{head}</item>\n")
        } else {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&self.data);
            format!("{head}\n<data encoding=\"base64\">\n{b64}</data></item>\n")
        }
    }
}

impl fmt::Debug for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} ", self.kind, self.code)?;
        if self.data.len() > 64 || self.data.iter().any(|b| *b < 0x20 && *b != b'\n') {
            write!(f, "<{} bytes>", self.data.len())
        } else {
            write!(f, "{:?}", self.text())
        }
    }
}

/// Incremental parser for the pipe; see the module docs.
#[derive(Debug, Default)]
pub struct Parser {
    buf: Vec<u8>,
}

/// Where the item text could not be read (the parser logs and skips it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Garbled(pub String);

const ITEM_OPEN: &[u8] = b"<item>";
const ITEM_CLOSE: &[u8] = b"</item>";
/// More than any cover art shairport-sync passes on (it is a few hundred kB at most); a buffer
/// past this without a complete item is junk.
const MAX_ITEM: usize = 16 << 20;

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes from the pipe; returns the items they complete, and items it had to skip.
    pub fn feed(&mut self, bytes: &[u8]) -> (Vec<Item>, Vec<Garbled>) {
        self.buf.extend_from_slice(bytes);
        let (mut items, mut bad) = (Vec::new(), Vec::new());
        loop {
            let Some(start) = find(&self.buf, ITEM_OPEN) else {
                // keep a possible partial "<item" at the end
                let keep = self.buf.len().min(ITEM_OPEN.len() - 1);
                self.buf.drain(..self.buf.len() - keep);
                break;
            };
            if start > 0 {
                let junk = &self.buf[..start];
                if junk.iter().any(|b| !b.is_ascii_whitespace()) {
                    bad.push(Garbled(preview(junk)));
                }
                self.buf.drain(..start);
            }
            // A new <item> before this one's </item> means this one was cut off.
            let close = find(&self.buf[ITEM_OPEN.len()..], ITEM_CLOSE).map(|i| i + ITEM_OPEN.len());
            let next_open = find(&self.buf[ITEM_OPEN.len()..], ITEM_OPEN).map(|i| i + ITEM_OPEN.len());
            match (close, next_open) {
                (Some(c), n) if n.is_none_or(|n| c < n) => {
                    let end = c + ITEM_CLOSE.len();
                    match parse_item(&self.buf[ITEM_OPEN.len()..c]) {
                        Ok(item) => items.push(item),
                        Err(e) => bad.push(e),
                    }
                    self.buf.drain(..end);
                }
                (_, Some(n)) => {
                    bad.push(Garbled(preview(&self.buf[..n])));
                    self.buf.drain(..n);
                }
                (_, None) => {
                    if self.buf.len() > MAX_ITEM {
                        bad.push(Garbled(preview(&self.buf)));
                        self.buf.clear();
                    }
                    break;
                }
            }
        }
        (items, bad)
    }
}

fn preview(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(&b[..b.len().min(80)]).into_owned();
    if b.len() > 80 {
        format!("{s}... ({} bytes)", b.len())
    } else {
        s
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn tag<'a>(body: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let s = find(body, open.as_bytes())?;
    let content = s + find(&body[s..], b">")? + 1;
    let e = content + find(&body[content..], close.as_bytes())?;
    Some(&body[content..e])
}

fn parse_item(body: &[u8]) -> Result<Item, Garbled> {
    let bad = || Garbled(preview(body));
    let text = |name| tag(body, name).and_then(|b| std::str::from_utf8(b).ok());
    let kind = text("type").and_then(FourCc::from_hex).ok_or_else(bad)?;
    let code = text("code").and_then(FourCc::from_hex).ok_or_else(bad)?;
    let length: usize = text("length").and_then(|l| l.trim().parse().ok()).ok_or_else(bad)?;
    let data = match tag(body, "data") {
        Some(b64) => {
            let clean: Vec<u8> = b64.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(clean).map_err(|_| bad())?
        }
        None => Vec::new(),
    };
    if data.len() != length {
        return Err(bad());
    }
    Ok(Item { kind, code, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_takes_any_chunking() {
        let items = vec![
            Item::new(b"ssnc", b"pbeg", vec![]),
            Item::new(b"core", b"minm", "All of Me"),
            Item::new(b"ssnc", b"PICT", (0..=255u8).cycle().take(5000).collect::<Vec<u8>>()),
            Item::new(b"core", b"asar", "Bjørn Eidsvåg"),
        ];
        let text: String = items.iter().map(Item::encode).collect();
        for chunk in [1, 7, 64, 4096, text.len()] {
            let mut p = Parser::new();
            let mut got = Vec::new();
            for c in text.as_bytes().chunks(chunk) {
                let (i, bad) = p.feed(c);
                assert!(bad.is_empty(), "{bad:?}");
                got.extend(i);
            }
            assert_eq!(got, items, "chunk {chunk}");
        }
    }

    #[test]
    fn starting_mid_item_resynchronises() {
        let a = Item::new(b"core", b"minm", "First").encode();
        let b = Item::new(b"core", b"asar", "Second").encode();
        let mut p = Parser::new();
        let (items, bad) = p.feed(format!("{}{}", &a[30..], b).as_bytes());
        assert_eq!(items, vec![Item::new(b"core", b"asar", "Second")]);
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn a_cut_off_item_is_skipped_at_the_next_one() {
        let a = Item::new(b"core", b"minm", "Cut").encode();
        let b = Item::new(b"ssnc", b"pend", vec![]).encode();
        let mut p = Parser::new();
        let (items, bad) = p.feed(format!("{}{}", &a[..a.len() - 12], b).as_bytes());
        assert_eq!(items, vec![Item::new(b"ssnc", b"pend", vec![])]);
        assert_eq!(bad.len(), 1);
    }

    #[test]
    fn length_mismatch_is_garbled() {
        let s = "<item><type>636f7265</type><code>6d696e6d</code><length>3</length>\n<data encoding=\"base64\">\nQWxsIG9mIE1l</data></item>";
        let (items, bad) = Parser::new().feed(s.as_bytes());
        assert!(items.is_empty());
        assert_eq!(bad.len(), 1);
    }
}
