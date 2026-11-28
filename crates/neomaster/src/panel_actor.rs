//! The real BeoSound 5 panel (USB 0cd4:1112): its IN reports become [`Event::PanelReport`], and
//! the core's click and backlight reports go out to it as SET_REPORT, as BeoSound5.exe sends
//! them.
//!
//! Two plain threads, because the panel is a small blocking device: one opens the panel, takes
//! it from usbhid and reads its interrupt endpoint, and reopens it every couple of seconds when
//! it is missing or goes away; the other sends OUT reports to whatever panel is open, so a slow
//! control transfer never holds up the core. Reports for a missing panel are dropped.

use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use log::{debug, info, warn};
use nusb::transfer::{ControlOut, ControlType, In, Interrupt, Recipient, TransferError};
use nusb::MaybeFuture;
use tokio::sync::mpsc;

use crate::core::{Event, PanelOut};

/// How long to wait before looking for a missing panel again.
const RETRY: Duration = Duration::from_secs(2);

type Shared = Arc<Mutex<Option<nusb::Interface>>>;

/// The core's handle on the panel: OUT reports go to the writer thread.
pub struct UsbPanel {
    out: std_mpsc::Sender<[u8; 2]>,
}

impl PanelOut for UsbPanel {
    fn write(&mut self, report: [u8; 2]) {
        let _ = self.out.send(report);
    }
}

/// Where the core's panel output goes in `--sim`: the real panel with `--panel`, else nowhere.
pub enum Panel {
    Usb(UsbPanel),
    None,
}

impl PanelOut for Panel {
    fn write(&mut self, report: [u8; 2]) {
        match self {
            Panel::Usb(p) => p.write(report),
            Panel::None => log::trace!("panel OUT {report:02x?}"),
        }
    }
}

/// Start the reader and writer threads. The panel need not be there yet.
pub fn spawn(events: mpsc::UnboundedSender<Event>) -> std::io::Result<UsbPanel> {
    let iface: Shared = Arc::new(Mutex::new(None));
    let (out_tx, out_rx) = std_mpsc::channel::<[u8; 2]>();
    let w = iface.clone();
    std::thread::Builder::new().name("panel-out".into()).spawn(move || writer(w, out_rx))?;
    std::thread::Builder::new().name("panel-in".into()).spawn(move || reader(iface, events))?;
    Ok(UsbPanel { out: out_tx })
}

fn writer(iface: Shared, rx: std_mpsc::Receiver<[u8; 2]>) {
    while let Ok(report) = rx.recv() {
        let Some(i) = iface.lock().ok().and_then(|g| g.clone()) else {
            debug!("panel OUT {report:02x?} dropped: no panel");
            continue;
        };
        let r = set_report(&i, report);
        if let Err(e) = r {
            warn!("panel OUT {report:02x?}: {e}");
        }
    }
}

/// SET_REPORT (output report 0) on interface 0, as BeoSound5.exe sends it.
fn set_report(i: &nusb::Interface, report: [u8; 2]) -> Result<(), TransferError> {
    i.control_out(
        ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: 0x09,
            value: 0x0200,
            index: 0,
            data: &report,
        },
        Duration::from_millis(500),
    )
    .wait()
}

fn reader(iface: Shared, events: mpsc::UnboundedSender<Event>) {
    let mut missing_logged = false;
    loop {
        match open() {
            Ok((i, ep)) => {
                missing_logged = false;
                // Backlight on: a panel left blanked (by the stock software, or by us before a
                // replug) shows nothing. The core's own state catches up on its next effect.
                for r in bs5_panel::Output::new().wake() {
                    if let Err(e) = set_report(&i, r) {
                        warn!("panel wake {r:02x?}: {e}");
                    }
                }
                *iface.lock().unwrap_or_else(|p| p.into_inner()) = Some(i);
                let gone = read(ep, &events);
                *iface.lock().unwrap_or_else(|p| p.into_inner()) = None;
                if !gone {
                    return; // the core has stopped
                }
                warn!("panel went away; looking for it again");
            }
            Err(e) => {
                if !missing_logged {
                    warn!("panel: {e}; retrying every {} s", RETRY.as_secs());
                    missing_logged = true;
                }
            }
        }
        std::thread::sleep(RETRY);
    }
}

fn open() -> Result<(nusb::Interface, nusb::Endpoint<Interrupt, In>), String> {
    let info = nusb::list_devices()
        .wait()
        .map_err(|e| format!("listing USB devices: {e}"))?
        .find(|d| d.vendor_id() == bs5_panel::VENDOR_ID && d.product_id() == bs5_panel::PRODUCT_ID)
        .ok_or_else(|| "no BeoSound 5 panel (USB 0cd4:1112) found".to_string())?;
    let device = info.open().wait().map_err(|e| format!("open: {e} (run as root, or add a udev rule)"))?;
    // usbhid holds it on a stock kernel
    let iface = device.detach_and_claim_interface(0).wait().map_err(|e| format!("claim interface 0: {e}"))?;
    let mut ep = iface.endpoint::<Interrupt, In>(bs5_panel::ENDPOINT_IN).map_err(|e| format!("endpoint 81: {e}"))?;
    let size = ep.max_packet_size();
    while ep.pending() < 4 {
        let b = ep.allocate(size);
        ep.submit(b);
    }
    info!("Panel on USB bus {} address {}", info.bus_id(), info.device_address());
    Ok((iface, ep))
}

/// Read reports until the panel goes away (true) or the core stops listening (false).
fn read(mut ep: nusb::Endpoint<Interrupt, In>, events: &mpsc::UnboundedSender<Event>) -> bool {
    loop {
        let Some(c) = ep.wait_next_complete(Duration::from_secs(1)) else {
            if events.is_closed() {
                return false;
            }
            continue;
        };
        match c.status {
            Ok(()) => {
                let bytes = &c.buffer[..c.actual_len];
                if let Ok(r) = <[u8; bs5_panel::REPORT_LEN]>::try_from(bytes) {
                    if events.send(Event::PanelReport(r)).is_err() {
                        return false;
                    }
                } else {
                    debug!("panel IN: {} bytes, ignored", bytes.len());
                }
            }
            Err(TransferError::Disconnected) => return true,
            Err(e) => {
                warn!("panel IN: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let mut buf = c.buffer;
        buf.clear();
        buf.set_requested_len(ep.max_packet_size());
        ep.submit(buf);
    }
}
