//! A one-slot channel where the newest value wins: the core puts each new scene here and the
//! render thread takes whatever is newest when it is ready for the next frame.
//!
//! A bounded queue is wrong for this in both directions. When the renderer is behind, a full
//! queue makes the sender drop the *newest* scene, so the screen can end up showing a stale
//! one; and a deeper queue just stores lag. Here a send never blocks and replaces any scene the
//! renderer has not taken yet.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};

struct State<T> {
    value: Option<T>,
    /// Values replaced before the receiver took them, since its last take.
    replaced: u32,
    senders: usize,
    receiver: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

pub struct Sender<T>(Arc<Shared<T>>);
pub struct Receiver<T>(Arc<Shared<T>>);

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State { value: None, replaced: 0, senders: 1, receiver: true }),
        ready: Condvar::new(),
    });
    (Sender(shared.clone()), Receiver(shared))
}

impl<T> Sender<T> {
    /// Put `value` in the slot, replacing one not yet taken. Gives it back if the receiver is
    /// gone.
    pub fn send(&self, value: T) -> Result<(), T> {
        let mut st = self.0.lock();
        if !st.receiver {
            return Err(value);
        }
        if st.value.replace(value).is_some() {
            st.replaced += 1;
        }
        drop(st);
        self.0.ready.notify_one();
        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.0.lock().senders += 1;
        Sender(self.0.clone())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.0.lock().senders -= 1;
        self.0.ready.notify_one();
    }
}

impl<T> Receiver<T> {
    /// Wait for a value. Returns it with the number of values it replaced unseen, or `None` once
    /// the slot is empty and every sender is gone.
    pub fn recv(&self) -> Option<(T, u32)> {
        let mut st = self.0.lock();
        loop {
            if let Some(v) = st.value.take() {
                let replaced = std::mem::take(&mut st.replaced);
                return Some((v, replaced));
            }
            if st.senders == 0 {
                return None;
            }
            st = self.0.ready.wait(st).unwrap_or_else(|p| p.into_inner());
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut st = self.0.lock();
        st.receiver = false;
        st.value = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_wins_and_counts_what_it_replaced() {
        let (tx, rx) = channel();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();
        assert_eq!(rx.recv(), Some((3, 2)));
        tx.send(4).unwrap();
        assert_eq!(rx.recv(), Some((4, 0)));
    }

    #[test]
    fn ends_when_the_senders_are_gone() {
        let (tx, rx) = channel();
        let tx2 = tx.clone();
        tx.send(1).unwrap();
        drop(tx);
        drop(tx2);
        assert_eq!(rx.recv(), Some((1, 0)));
        assert_eq!(rx.recv(), None);
    }

    #[test]
    fn wakes_a_waiting_receiver() {
        let (tx, rx) = channel();
        let t = std::thread::spawn(move || rx.recv());
        std::thread::sleep(std::time::Duration::from_millis(20));
        tx.send(7).unwrap();
        assert_eq!(t.join().unwrap(), Some((7, 0)));
    }

    #[test]
    fn send_fails_without_a_receiver() {
        let (tx, rx) = channel();
        drop(rx);
        assert_eq!(tx.send(1), Err(1));
    }
}
