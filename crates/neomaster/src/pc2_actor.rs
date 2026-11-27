//! The PC2 actor: owns the USB device and the [`pc2::Pc2`] state machine (framing, pacing, fan
//! loop), turns IN packets into [`Event::Pc2`] for the core and [`Pc2Cmd`]s from the core into
//! OUT packets.
//!
//! USB is two pipes ([`UsbPipes`]): packets in, transfers out. [`open_usb`] connects them to the
//! real PC2 (vendor class 0cd4:0101, interrupt endpoints 0x81 / 0x01, 9-byte packets) with
//! nusb; [`crate::sim_pc2::spawn`] connects them to a simulated one. [`supervise`] reopens the
//! device after it goes away, and re-runs the PC2 start-up each time.

use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
use nusb::transfer::{In, Interrupt, Out, TransferError};
use tokio::sync::mpsc;
use tokio::time::sleep_until;

use crate::core::Event;
use crate::debug::{self, Tap};
use crate::link::Pc2Cmd;

/// Packets from the device and transfers to it. When the device goes away `rx` closes.
pub struct UsbPipes {
    pub rx: mpsc::Receiver<Vec<u8>>,
    pub tx: mpsc::Sender<Vec<u8>>,
}

/// How long to wait before looking for the PC2 again.
pub const REOPEN_DELAY: Duration = Duration::from_secs(2);
/// IN transfers kept in flight.
const IN_FLIGHT: usize = 4;

#[derive(Debug)]
pub enum OpenError {
    NotFound,
    Usb(nusb::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotFound => write!(f, "no PC2 (USB {:04x}:{:04x}) found", pc2::VENDOR_ID, pc2::PRODUCT_ID),
            OpenError::Usb(e) => write!(f, "PC2: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<nusb::Error> for OpenError {
    fn from(e: nusb::Error) -> Self {
        OpenError::Usb(e)
    }
}

/// Open the PC2 and start the reader and writer tasks.
pub async fn open_usb() -> Result<UsbPipes, OpenError> {
    let info = nusb::list_devices()
        .await?
        .find(|d| d.vendor_id() == pc2::VENDOR_ID && d.product_id() == pc2::PRODUCT_ID)
        .ok_or(OpenError::NotFound)?;
    info!("PC2 on USB bus {} address {}", info.bus_id(), info.device_address());
    let device = info.open().await?;
    // Vendor class: no kernel driver should hold it, but take it from one if it does.
    let iface = match device.claim_interface(0).await {
        Ok(i) => i,
        Err(_) => device.detach_and_claim_interface(0).await?,
    };
    let mut ep_in = iface.endpoint::<Interrupt, In>(pc2::ENDPOINT_IN)?;
    let mut ep_out = iface.endpoint::<Interrupt, Out>(pc2::ENDPOINT_OUT)?;

    let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(64);
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);

    tokio::spawn(async move {
        let size = ep_in.max_packet_size();
        while ep_in.pending() < IN_FLIGHT {
            let buf = ep_in.allocate(size);
            ep_in.submit(buf);
        }
        loop {
            let c = ep_in.next_complete().await;
            match c.status {
                Ok(()) => {
                    if in_tx.send(c.buffer[..c.actual_len].to_vec()).await.is_err() {
                        return; // the actor is gone
                    }
                }
                Err(TransferError::Disconnected) => {
                    warn!("PC2 disconnected");
                    return;
                }
                Err(TransferError::Cancelled) => return,
                Err(e) => {
                    warn!("PC2 IN transfer failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            let mut buf = c.buffer;
            buf.clear();
            buf.set_requested_len(size);
            ep_in.submit(buf);
        }
    });

    tokio::spawn(async move {
        while let Some(packet) = out_rx.recv().await {
            ep_out.submit(packet.into());
            let c = ep_out.next_complete().await;
            if let Err(e) = c.status {
                error!("PC2 OUT transfer failed: {e}");
                if e == TransferError::Disconnected {
                    return;
                }
            }
        }
    });

    Ok(UsbPipes { rx: in_rx, tx: out_tx })
}

/// Why a session with the device ended.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionEnd {
    /// The device went away; look for it again.
    Gone,
    /// The core dropped its command sender: shut down.
    Shutdown,
}

/// Run one session with an open device: start it up as `mode`, then pump packets both ways
/// until it goes away or the core stops.
pub async fn run_session(
    mode: &mut pc2::Mode,
    node_address: u8,
    mut usb: UsbPipes,
    cmds: &mut mpsc::UnboundedReceiver<Pc2Cmd>,
    events: &mpsc::UnboundedSender<Event>,
) -> SessionEnd {
    let epoch = Instant::now();
    let now = || epoch.elapsed();
    let mut pc2 = pc2::Pc2::new(*mode, node_address);
    let tap = Tap::global();
    tap.emit("pc2_session", || serde_json::json!({ "state": "open", "mode": format!("{mode:?}") }));
    // Commands queued while there was no device are stale (routing for a PC2 that rebooted).
    while cmds.try_recv().is_ok() {}
    pc2.init(now());
    if events.send(Event::Pc2Connected).is_err() {
        return SessionEnd::Shutdown;
    }
    let end = loop {
        if flush(&mut pc2, &usb.tx, &tap).await.is_err() {
            break SessionEnd::Gone;
        }
        let deadline = pc2.next_deadline().map(|d| tokio::time::Instant::from_std(epoch + d));
        tokio::select! {
            packet = usb.rx.recv() => {
                let Some(packet) = packet else { break SessionEnd::Gone };
                tap.emit("usb_in", || serde_json::json!({ "raw": debug::hex(&packet) }));
                for ev in pc2.on_usb_packet(&packet, now()) {
                    if events.send(Event::Pc2(ev)).is_err() {
                        break;
                    }
                }
            }
            cmd = cmds.recv() => match cmd {
                Some(Pc2Cmd::Message(m)) => pc2.send(m, now()),
                Some(Pc2Cmd::Telegram(t)) => {
                    // Refusals are logged by the PC2 state machine
                    if let Err(e) = pc2.send_telegram(&t, now()) {
                        tap.emit("pc2_refused", || {
                            let mut v = debug::telegram_fields(&t);
                            v["reason"] = serde_json::json!(e.to_string());
                            v
                        });
                    }
                }
                Some(Pc2Cmd::Restart(m)) => {
                    info!("PC2: starting up again as {m:?}");
                    tap.emit("pc2_session", || serde_json::json!({ "state": "restart", "mode": format!("{m:?}") }));
                    pc2.close(now());
                    if flush(&mut pc2, &usb.tx, &tap).await.is_err() {
                        break SessionEnd::Gone;
                    }
                    *mode = m;
                    pc2 = pc2::Pc2::new(m, node_address);
                    pc2.init(now());
                    if events.send(Event::Pc2Connected).is_err() {
                        break SessionEnd::Shutdown;
                    }
                }
                None => {
                    pc2.close(now());
                    let _ = flush(&mut pc2, &usb.tx, &tap).await;
                    break SessionEnd::Shutdown;
                }
            },
            _ = async { sleep_until(deadline.unwrap()).await }, if deadline.is_some() => pc2.tick(now()),
        }
    };
    tap.emit("pc2_session", || serde_json::json!({ "state": format!("{end:?}") }));
    if end == SessionEnd::Gone {
        let _ = events.send(Event::Pc2Disconnected);
    }
    end
}

async fn flush(pc2: &mut pc2::Pc2, tx: &mpsc::Sender<Vec<u8>>, tap: &Tap) -> Result<(), ()> {
    while let Some(p) = pc2.poll_transmit() {
        debug!("PC2 OUT {p:02x?}");
        tap.emit("usb_out", || serde_json::json!({ "raw": debug::hex(&p) }));
        tx.send(p).await.map_err(|_| ())?;
    }
    Ok(())
}

/// Keep a session with the PC2 running: open it with `open`, run it, and when it goes away look
/// for it again every [`REOPEN_DELAY`]. Ends when the core drops its command sender.
pub async fn supervise<F, Fut>(
    mut mode: pc2::Mode,
    node_address: u8,
    mut open: F,
    mut cmds: mpsc::UnboundedReceiver<Pc2Cmd>,
    events: mpsc::UnboundedSender<Event>,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<UsbPipes, OpenError>>,
{
    let mut warned = false;
    loop {
        match open().await {
            Ok(usb) => {
                warned = false;
                info!("PC2 open, starting up as {mode:?}");
                if run_session(&mut mode, node_address, usb, &mut cmds, &events).await == SessionEnd::Shutdown {
                    return;
                }
            }
            Err(e) if !warned => {
                warn!("{e}; retrying every {} s", REOPEN_DELAY.as_secs());
                warned = true;
            }
            Err(_) => {}
        }
        // Wait, but notice the core going away meanwhile; drop commands for the missing device.
        let wake = tokio::time::Instant::now() + REOPEN_DELAY;
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    None => return,
                    // No device: remember the mode for the next open.
                    Some(Pc2Cmd::Restart(m)) => mode = m,
                    Some(_) => {}
                },
                _ = sleep_until(wake) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_pc2::{self, SimInput};

    async fn next_event(events: &mut mpsc::UnboundedReceiver<Event>) -> Event {
        tokio::time::timeout(Duration::from_secs(30), events.recv()).await.expect("an event in time").expect("actor alive")
    }

    /// The actor against the simulated PC2: start-up, telegrams out with pacing, IR and
    /// Masterlink in, unplug, reopen, shutdown.
    #[tokio::test(start_paused = true)]
    async fn actor_runs_against_the_simulated_pc2() {
        let (obs_tx, mut obs) = mpsc::unbounded_channel();
        let (pipes, sim) = sim_pc2::spawn(Some(obs_tx.clone()));
        let mut devices = vec![pipes];
        let (event_tx, mut events) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let open = move || {
            let r = devices.pop().ok_or(OpenError::NotFound);
            async move { r }
        };
        let actor = tokio::spawn(supervise(pc2::Mode::SourceCenter, 0, open, cmd_rx, event_tx));

        assert!(matches!(next_event(&mut events).await, Event::Pc2Connected));
        assert_eq!(obs.recv().await.unwrap(), vec![0xf1]);
        assert_eq!(obs.recv().await.unwrap(), vec![0x80, 0x29, 0x00]);
        assert_eq!(obs.recv().await.unwrap()[0], 0xf6);
        // the ML START reply carries the node address back: nothing new to report

        // two telegrams: the second goes out after the PC2's answer to the first
        let t = masterlink::Telegram::decode(&[0x80, 0xc2, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x11, 0x00, 0x01]).unwrap().encode();
        cmd_tx.send(Pc2Cmd::Telegram(t.clone())).unwrap();
        cmd_tx.send(Pc2Cmd::Telegram(t.clone())).unwrap();
        let mut e0 = t.clone();
        e0.insert(0, 0xe0);
        assert_eq!(obs.recv().await.unwrap(), e0);
        assert_eq!(obs.recv().await.unwrap(), e0);

        sim.send(SimInput::Ir { kind: 0x01, code: 0x94 }).unwrap();
        // (the ML START reply, IN 41, comes first as an Other event)
        let ev = loop {
            match next_event(&mut events).await {
                Event::Pc2(pc2::Event::Other(_)) => continue,
                e => break e,
            }
        };
        assert!(matches!(ev, Event::Pc2(pc2::Event::Beo4Key { kind: 0x01, code: 0x94 })), "{ev:?}");
        // a 20-byte telegram arrives in three USB packets and is put back together
        let connect = [
            0xc2, 0xc1, 0x01, 0x0b, 0x7a, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02,
            0x00,
        ];
        let encoded = masterlink::Telegram::decode(&connect).unwrap().encode();
        sim.send(SimInput::Telegram(encoded.clone())).unwrap();
        match next_event(&mut events).await {
            Event::Pc2(pc2::Event::Masterlink(bytes)) => assert_eq!(bytes, encoded),
            e => panic!("unexpected {e:?}"),
        }

        // unplug: the core hears about it, the actor looks for the device again
        sim.send(SimInput::Unplug).unwrap();
        assert!(matches!(next_event(&mut events).await, Event::Pc2Disconnected));
        tokio::time::sleep(REOPEN_DELAY * 3).await;
        assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

        // the core stops: the actor ends
        drop(cmd_tx);
        tokio::time::timeout(Duration::from_secs(30), actor).await.expect("actor ends").unwrap();
    }

    /// When the core stops while the device is open, the PC2 gets its close message (a7).
    #[tokio::test(start_paused = true)]
    async fn shutdown_closes_the_pc2() {
        let (obs_tx, mut obs) = mpsc::unbounded_channel();
        let (pipes, _sim) = sim_pc2::spawn(Some(obs_tx));
        let (event_tx, mut events) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
        let session =
            tokio::spawn(async move { run_session(&mut pc2::Mode::AudioMaster, 0, pipes, &mut cmd_rx, &event_tx).await });
        assert!(matches!(next_event(&mut events).await, Event::Pc2Connected));
        drop(cmd_tx);
        assert_eq!(session.await.unwrap(), SessionEnd::Shutdown);
        let mut last = None;
        while let Ok(m) = obs.try_recv() {
            last = Some(m);
        }
        // the observer may lag the session by a scheduling round
        if last.is_none() {
            last = obs.recv().await;
        }
        let mut all = vec![last.unwrap()];
        while let Ok(Some(m)) = tokio::time::timeout(Duration::from_millis(10), obs.recv()).await {
            all.push(m);
        }
        assert_eq!(all.last().unwrap(), &vec![0xa7]);
    }

    /// AUDIO MASTER changed in the settings: the PC2 is closed and started up again in the new
    /// mode on the same device, and the core hears Pc2Connected again.
    #[tokio::test(start_paused = true)]
    async fn restart_reinitialises_in_the_new_mode() {
        let (obs_tx, mut obs) = mpsc::unbounded_channel();
        let (pipes, _sim) = sim_pc2::spawn(Some(obs_tx));
        let (event_tx, mut events) = mpsc::unbounded_channel();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
        let session = tokio::spawn(async move {
            let mut mode = pc2::Mode::SourceCenter;
            let end = run_session(&mut mode, 0, pipes, &mut cmd_rx, &event_tx).await;
            (end, mode)
        });
        assert!(matches!(next_event(&mut events).await, Event::Pc2Connected));
        assert_eq!(obs.recv().await.unwrap(), vec![0xf1]);
        let sc_address = obs.recv().await.unwrap();
        obs.recv().await.unwrap(); // option
        cmd_tx.send(Pc2Cmd::Restart(pc2::Mode::AudioMaster)).unwrap();
        // (the first start-up's replies may still arrive as Pc2 events)
        loop {
            match next_event(&mut events).await {
                Event::Pc2(_) => continue,
                e => break assert!(matches!(e, Event::Pc2Connected), "{e:?}"),
            }
        }
        let mut seen = Vec::new();
        while let Ok(Some(m)) = tokio::time::timeout(Duration::from_millis(50), obs.recv()).await {
            seen.push(m);
        }
        let close = seen.iter().position(|m| m == &vec![0xa7]).expect("close sent");
        let f1 = seen.iter().position(|m| m == &vec![0xf1]).expect("init again");
        assert!(close < f1, "{seen:02x?}");
        let am_address = &seen[f1 + 1];
        assert_ne!(am_address, &sc_address, "the default node address follows the mode");
        drop(cmd_tx);
        let (end, mode) = session.await.unwrap();
        assert_eq!(end, SessionEnd::Shutdown);
        assert_eq!(mode, pc2::Mode::AudioMaster);
    }
}
