//! A recording's telegrams as a capture-replay vector: the `&[Row]` format of
//! `crates/masterlink/tests/vectors` (MLGW form, no checksum / EOT; `out` = sent by neomaster,
//! which a replay expects from the code under test instead of feeding it in).

use std::fmt::Write as _;

use serde_json::Value;

/// One telegram from a recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub t: f64,
    pub out: bool,
    pub bytes: Vec<u8>,
}

/// The telegrams of a JSON Lines recording between `from` and `to` (recording seconds), with
/// `t` counted from the first one kept. Lines that are not telegram records are skipped.
pub fn rows(jsonl: &str, from: Option<f64>, to: Option<f64>) -> Result<Vec<Row>, String> {
    let mut rows = vec![];
    for (n, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let out = match v["kind"].as_str() {
            Some("ml_in") => false,
            Some("ml_out") => true,
            _ => continue,
        };
        let t = v["t"].as_f64().unwrap_or(0.0);
        if from.is_some_and(|f| t < f) || to.is_some_and(|e| t > e) {
            continue;
        }
        let Some(mlgw) = v["mlgw"].as_str() else { continue };
        rows.push(Row { t, out, bytes: super::parse_hex(mlgw).map_err(|e| format!("line {}: {e}", n + 1))? });
    }
    if let Some(t0) = rows.first().map(|r| r.t) {
        for r in &mut rows {
            r.t -= t0;
        }
    }
    Ok(rows)
}

/// The Rust source of a vector named `name`, ready to drop into `tests/vectors`.
pub fn render(name: &str, rows: &[Row], source: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "// Recorded by neomaster's debug connection ({source}); converted by `neomaster debug vectors`.");
    let _ = writeln!(s, "// t = seconds. Bytes in MLGW form: no checksum / EOT. out = sent by neomaster.");
    let _ = writeln!(s, "pub const {name}: &[Row] = &[");
    for r in rows {
        let bytes: Vec<String> = r.bytes.iter().map(|b| format!("0x{b:02x}")).collect();
        let _ = writeln!(s, "    Row {{ t: {:.3}, out: {}, bytes: &[{}] }},", r.t, r.out, bytes.join(", "));
    }
    s.push_str("];\n");
    s
}

/// A vector name from what a person typed: `kitchen join` → `KITCHEN_JOIN`.
pub fn const_name(s: &str) -> String {
    let mut n: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    if n.is_empty() || n.starts_with(|c: char| c.is_ascii_digit()) {
        n.insert_str(0, "V_");
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::{record, telegram_fields};
    use serde_json::json;

    fn ml(t: f64, kind: &str, encoded: &[u8]) -> String {
        record(t, kind, telegram_fields(encoded)).to_string()
    }

    /// A recording round-trips to the vector format the replays include, byte for byte the
    /// way `source_center_live.rs` writes its rows.
    #[test]
    fn recordings_become_vectors() {
        let connect = masterlink::Telegram::decode(&[
            0xc2, 0xc1, 0x01, 0x0b, 0x7a, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02,
            0x00,
        ])
        .unwrap()
        .encode();
        let reply =
            masterlink::Telegram::decode(&[0xc1, 0xc2, 0x01, 0x14, 0x00, 0x7a, 0x00, 0x6c, 0x01, 0x08, 0x01]).unwrap().encode();
        let jsonl = [
            record(0.0, "hello", json!({ "role": "SourceCenter" })).to_string(),
            ml(10.5, "ml_in", &connect),
            record(10.6, "log", json!({ "msg": "x" })).to_string(),
            ml(11.12, "ml_out", &reply),
            ml(30.0, "ml_in", &connect),
        ]
        .join("\n");
        let rows = rows(&jsonl, None, Some(20.0)).unwrap();
        assert_eq!(rows.len(), 2);
        let src = render("LIVE_KITCHEN", &rows, "test");
        let expected_rows = "\
pub const LIVE_KITCHEN: &[Row] = &[
    Row { t: 0.000, out: false, bytes: &[0xc2, 0xc1, 0x01, 0x0b, 0x7a, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02, 0x00] },
    Row { t: 0.620, out: true, bytes: &[0xc1, 0xc2, 0x01, 0x14, 0x00, 0x7a, 0x00, 0x6c, 0x01, 0x08, 0x01] },
];
";
        assert!(src.ends_with(expected_rows), "{src}");
        // the same lines as the captured vectors (LIVE_KITCHEN_AND_STANDBY, 1.772 s and 2.392 s)
        let captured = include_str!("../../../masterlink/tests/vectors/source_center_live.rs");
        assert!(captured.contains("bytes: &[0xc2, 0xc1, 0x01, 0x0b, 0x7a, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02, 0x00] },"));
        assert!(captured.contains("out: true, bytes: &[0xc1, 0xc2, 0x01, 0x14, 0x00, 0x7a, 0x00, 0x6c, 0x01, 0x08, 0x01] },"));
    }

    #[test]
    fn names() {
        assert_eq!(const_name("kitchen join"), "KITCHEN_JOIN");
        assert_eq!(const_name("2nd"), "V_2ND");
    }
}
