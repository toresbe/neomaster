//! Hardware bring-up kit for the BeoMaster 5 (milestone M0 of the architecture sketch).
//!
//! Three programs, cross-built for i686 and run on the box by `tools/bringup/bringup.sh`:
//!
//! - `nm-xbench`: what the panel UI will ask of the screen (cover-arc composites at the baked
//!   sizes, alpha blends, glyph runs, cover-strip and full-screen redraws paced at 60 fps, cover
//!   uploads, how many covers fit in video memory), through XRender and through a software
//!   blitter, so the render backend can be chosen on numbers.
//! - `nm-panel-probe`: reads the BeoSound 5 panel over USB, walks a person through every
//!   control, and records the reports in the debug connection's format so
//!   `neomaster debug replay` can feed them back into neomaster.
//! - `nm-player-bench`: the Sendspin player's CPU, memory and start latency on the C7.
//!
//! Everything prints a table for people and writes JSON Lines for `host/report.py`.

pub mod scene;
pub mod stats;
pub mod sw;
pub mod xr;

use std::io::Write;

use scene::{CoverFmt, Frame};

/// Something that can draw the benchmark's frames.
pub trait Backend {
    fn name(&self) -> String;
    /// What it runs on (server vendor, depths, formats, ...), for the results.
    fn info(&self) -> serde_json::Value;
    fn supports(&self, fmt: CoverFmt) -> bool;
    /// Upload the cover pool for (fmt, size); later [`Backend::draw`] calls may use it.
    fn prepare(&mut self, fmt: CoverFmt, size: u16) -> Result<(), String>;
    /// Queue a frame (it may still be in flight when this returns).
    fn draw(&mut self, frame: &Frame) -> Result<(), String>;
    /// Wait until everything drawn so far has reached the pixels.
    fn sync(&mut self) -> Result<(), String>;
    /// The XRender backend, for the tests only it can run (uploads, video memory).
    fn as_xr(&mut self) -> Option<&mut xr::XrBackend> {
        None
    }
}

/// Append one JSON object as a line to `out` (if any) and flush, so a crash keeps what ran.
pub fn emit(out: &mut Option<std::fs::File>, v: &serde_json::Value) {
    if let Some(f) = out {
        let _ = writeln!(f, "{v}").and_then(|_| f.flush());
    }
}

/// Process CPU time (user + system) in seconds, from /proc/self/stat.
pub fn process_cpu_seconds() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the parenthesised command name; utime and stime are fields 14 and 15.
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let utime: f64 = f.get(11)?.parse().ok()?;
    let stime: f64 = f.get(12)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    Some((utime + stime) / hz.max(1.0))
}

/// Whole-system (busy, total) jiffies from the first line of /proc/stat.
pub fn system_jiffies() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    let total: u64 = v.iter().sum();
    // idle + iowait
    let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// Resident set size in kB, from /proc/self/status.
pub fn rss_kb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()
}
