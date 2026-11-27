//! `--debug-record FILE`: the whole stream (raw USB included) to a JSON Lines file from
//! start-up, so the first telegrams after the PC2 opens are not missed. The first line is the
//! `hello` record.

use std::io::Write;
use std::path::Path;

use log::warn;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

use super::Tap;

/// Start writing (on a thread of its own, so a slow disk never holds up the core).
pub fn start(tap: &Tap, path: &Path, hello: Value) -> std::io::Result<()> {
    let Some(mut rx) = tap.subscribe() else { return Ok(()) };
    let file = std::fs::File::create(path)?;
    let mut w = std::io::BufWriter::new(file);
    writeln!(w, "{}", tap.line("hello", hello).json)?;
    w.flush()?;
    let tap = tap.clone();
    std::thread::Builder::new().name("debug-record".into()).spawn(move || loop {
        let line = match rx.blocking_recv() {
            Ok(l) => l.json,
            Err(RecvError::Lagged(n)) => tap.line("lagged", serde_json::json!({ "missed": n })).json,
            Err(RecvError::Closed) => return,
        };
        // flush per record: the box may freeze or lose power, and the tail is what matters
        if writeln!(w, "{line}").and_then(|_| w.flush()).is_err() {
            warn!("Debug recording: write failed; recording stopped");
            return;
        }
    })?;
    Ok(())
}
