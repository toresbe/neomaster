//! Stop-and-wait sender for Masterlink telegrams (PC2 messages starting with 0xE0).
//!
//! The PC2 answers every Masterlink telegram it is given with a status message (type 0x01, e.g.
//! `60 04 01 01 01 02 61`). In USB captures of B&O's own software (BeoIpcML), the next telegram is
//! never sent before that answer has arrived. This reproduces that: queued telegrams go out one at
//! a time, the next when [`TelegramPacer::acknowledge`] is called or, if no answer comes, after the
//! timeout. Port of libpc2 `PC2TelegramPacer`, with its worker thread replaced by `now`/`tick`.

use std::collections::VecDeque;
use std::time::Duration;

use log::{debug, warn};

use crate::{Message, Time};

pub struct TelegramPacer {
    timeout: Duration,
    queue: VecDeque<Message>,
    /// Deadline for the PC2's answer to the telegram in flight.
    awaiting_ack: Option<Time>,
    timeout_count: u32,
}

impl TelegramPacer {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

    pub fn new(timeout: Duration) -> Self {
        TelegramPacer { timeout, queue: VecDeque::new(), awaiting_ack: None, timeout_count: 0 }
    }

    /// Queue a telegram. Returns it right back if nothing is in flight: send it now.
    pub fn enqueue(&mut self, telegram: Message, now: Time) -> Option<Message> {
        self.queue.push_back(telegram);
        self.next(now)
    }

    /// Call when the PC2 reports Masterlink status (message type 0x01). Returns the next telegram
    /// to send, if any.
    pub fn acknowledge(&mut self, now: Time) -> Option<Message> {
        if self.awaiting_ack.take().is_none() {
            debug!("Masterlink status received with no telegram outstanding");
            return None;
        }
        self.next(now)
    }

    /// Call periodically: releases the next telegram if the PC2 never answered.
    pub fn tick(&mut self, now: Time) -> Option<Message> {
        match self.awaiting_ack {
            Some(deadline) if now >= deadline => {
                self.awaiting_ack = None;
                self.timeout_count += 1;
                warn!("No Masterlink status from PC2 within {} ms; sending next telegram anyway", self.timeout.as_millis());
                self.next(now)
            }
            _ => None,
        }
    }

    fn next(&mut self, now: Time) -> Option<Message> {
        if self.awaiting_ack.is_some() {
            return None;
        }
        let t = self.queue.pop_front()?;
        self.awaiting_ack = Some(now + self.timeout);
        Some(t)
    }

    /// When the telegram in flight times out, if one is.
    pub fn deadline(&self) -> Option<Time> {
        self.awaiting_ack
    }
    /// Number of telegrams not yet sent.
    pub fn pending(&self) -> usize {
        self.queue.len()
    }
    /// Number of telegrams whose acknowledgement timed out (for diagnostics/tests).
    pub fn timeouts(&self) -> u32 {
        self.timeout_count
    }
}

impl Default for TelegramPacer {
    fn default() -> Self {
        Self::new(Self::DEFAULT_TIMEOUT)
    }
}
