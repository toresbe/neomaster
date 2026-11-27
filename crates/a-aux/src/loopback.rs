//! Carries out [`crate::Effect`]s with `alsaloop` (alsa-utils): capture from the USB card, play
//! to the BM5's output. This is what BeoSound5.exe's DirectShow graph did, and keeps neomaster
//! free of a libasound build dependency.

use std::io;
use std::process::{Child, Command, Stdio};

use log::{info, warn};

use crate::Effect;

#[derive(Clone, Debug)]
pub struct LoopbackConfig {
    /// The program, normally "alsaloop".
    pub program: String,
    /// The ALSA playback device: the output the PC2 hears (the BM5's onboard audio).
    pub playback: String,
    /// Loop latency in microseconds (alsaloop -t).
    pub latency_us: u32,
}

impl Default for LoopbackConfig {
    fn default() -> Self {
        LoopbackConfig { program: "alsaloop".into(), playback: "default".into(), latency_us: 50_000 }
    }
}

impl LoopbackConfig {
    /// The defaults, with `NEOMASTER_AUX_PLAYBACK` and `NEOMASTER_AUX_LATENCY_US` applied.
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Ok(p) = std::env::var("NEOMASTER_AUX_PLAYBACK") {
            c.playback = p;
        }
        if let Some(t) = std::env::var("NEOMASTER_AUX_LATENCY_US").ok().and_then(|t| t.parse().ok()) {
            c.latency_us = t;
        }
        c
    }

    /// The argument list for capturing from `capture`.
    pub fn args(&self, capture: &str) -> Vec<String> {
        vec!["-C".into(), capture.into(), "-P".into(), self.playback.clone(), "-t".into(), self.latency_us.to_string()]
    }
}

pub struct Loopback {
    config: LoopbackConfig,
    child: Option<Child>,
}

impl Loopback {
    pub fn new(config: LoopbackConfig) -> Self {
        Loopback { config, child: None }
    }

    /// New settings; they apply from the next start of the loop.
    pub fn set_config(&mut self, config: LoopbackConfig) {
        self.config = config;
    }

    pub fn apply(&mut self, effect: &Effect) {
        match effect {
            Effect::Start { capture } => {
                self.stop();
                match self.spawn(capture) {
                    Ok(child) => {
                        info!("A.AUX: playing {capture} through {}", self.config.playback);
                        self.child = Some(child);
                    }
                    Err(e) => warn!("A.AUX: could not start {}: {e}", self.config.program),
                }
            }
            Effect::Stop => self.stop(),
        }
    }

    fn spawn(&self, capture: &str) -> io::Result<Child> {
        Command::new(&self.config.program).args(self.config.args(capture)).stdin(Stdio::null()).stdout(Stdio::null()).spawn()
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            info!("A.AUX: stopped");
        }
    }

    /// True if a loop was running and has exited by itself since the last call.
    pub fn ended(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else { return false };
        match child.try_wait() {
            Ok(None) => false,
            Ok(Some(status)) => {
                warn!("A.AUX: {} exited ({status})", self.config.program);
                self.child = None;
                true
            }
            Err(e) => {
                warn!("A.AUX: {e}");
                self.child = None;
                true
            }
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop();
    }
}
