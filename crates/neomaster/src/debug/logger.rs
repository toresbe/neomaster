//! Log lines into the debug stream as `log` records, next to stderr.

use log::{Log, Metadata, Record};
use serde_json::json;

use super::Tap;

/// env_logger, plus a copy of every line it prints to [`Tap::global`].
pub struct TapLogger {
    inner: env_logger::Logger,
}

impl TapLogger {
    /// Install as the global logger (instead of `env_logger::Builder::init`).
    pub fn init(builder: &mut env_logger::Builder) {
        let inner = builder.build();
        log::set_max_level(inner.filter());
        let _ = log::set_boxed_logger(Box::new(TapLogger { inner }));
    }
}

impl Log for TapLogger {
    fn enabled(&self, m: &Metadata) -> bool {
        self.inner.enabled(m)
    }

    fn log(&self, r: &Record) {
        if !self.inner.matches(r) {
            return;
        }
        self.inner.log(r);
        Tap::global().emit("log", || json!({ "level": r.level().as_str(), "target": r.target(), "msg": r.args().to_string() }));
    }

    fn flush(&self) {
        self.inner.flush()
    }
}
