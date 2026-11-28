//! `--sim`: the panel UI in a desktop window. The keyboard stands in for the panel and produces
//! the same raw HID reports the real panel sends, so the core, the panel decoder and the UI run
//! exactly as they will on the BM5. Rendering uses the same XRender backend.
//!
//! Threads: the X11 input thread (blocking on events), the render thread (draws the newest
//! scene), and a small tokio runtime for the core.

use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use bs5_panel::{Report, BUTTON_GO, BUTTON_LEFT, BUTTON_RIGHT, COUNTS_PER_STEP, POINTER_MAX};
use log::{info, warn};
use render::xrender::XRenderer;
use render::{Renderer, Scene, SCREEN_HEIGHT, SCREEN_WIDTH};
use tokio::sync::mpsc;
use x11rb::connection::Connection;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, KeyButMask, Keycode};
use x11rb::protocol::Event as XEvent;
use x11rb::rust_connection::RustConnection;

use crate::core::{Core, Event};
use crate::link::{self, Link};
use crate::Pc2Device;

pub const KEYS: &str = "\
neomaster --sim keys (after BeoSound5.exe's own keyboard emulation of the panel):

  Up / Down, mouse wheel   menu wheel, one list row per press
  A / Z                    pointer arc +1 / -1 (hold to sweep); PgUp / PgDn: +6 / -6
  1 .. 8                   jump the pointer to a zone: now playing, queue, favourites, titles,
                           albums, artists, covers, mode
  Left / Right             LEFT (drill in) / RIGHT (back)
  G or Return              GO; hold for the context menu
  + / -                    volume wheel
  S                        standby (this room); Shift+S: long standby (everything off)
  Esc or Q                 quit
";

pub fn run(
    startup: crate::Startup,
    device: Pc2Device,
    real_panel: bool,
    debug_cfg: &crate::debug::Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let role = startup.role();
    let mut renderer = XRenderer::open_window("neomaster --sim", SCREEN_WIDTH, SCREEN_HEIGHT)?;
    // The stock look, where $NEOMASTER_STOCK_ASSETS points at the BM5's disk (see ui::assets).
    let assets = ui::Assets::from_env();
    renderer.set_font(assets.font())?;
    let conn = renderer.connection();
    let window = renderer.window();
    let wm_delete = renderer.wm_delete_window;
    info!("Simulator window open. Keys:\n{KEYS}");

    // Held keys repeat as presses without releases, so GO can be held for the context menu.
    if conn.xkb_use_extension(1, 0)?.reply()?.supported {
        let flag = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
        let _ =
            conn.xkb_per_client_flags(xkb::ID::USE_CORE_KBD.into(), flag, flag, 0u32.into(), 0u32.into(), 0u32.into())?.reply();
    } else {
        warn!("No XKB: holding GO will autorepeat instead of opening the context menu");
    }

    let (scene_tx, scene_rx) = std_mpsc::sync_channel::<Scene>(2);
    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();

    let render_thread = std::thread::Builder::new().name("render".into()).spawn(move || {
        while let Ok(mut scene) = scene_rx.recv() {
            while let Ok(newer) = scene_rx.try_recv() {
                scene = newer;
            }
            if let Err(e) = renderer.render(&scene) {
                log::error!("render: {e}");
                return;
            }
        }
    })?;

    // Start on ARTISTS. The first report tells the decoder where the pointer is.
    let start = ui::n_music_zones().iter().find(|z| z.name == "ARTISTS").map(|z| (z.lo + z.hi) / 2).unwrap_or(64);
    let panel = SimPanel { pointer: start, buttons: 0 };
    event_tx.send(panel.report(0, 0))?;

    let input_conn = conn.clone();
    let input_tx = event_tx.clone();
    std::thread::Builder::new().name("x11-input".into()).spawn(move || {
        if let Err(e) = input_loop(&input_conn, window, wm_delete, &input_tx, panel) {
            log::error!("x11 input: {e}");
        }
        let _ = input_tx.send(Event::Quit);
    })?;

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    // N.MUSIC: Music Assistant if configured (the library arrives from the mirror), else a
    // made-up library
    let (music, ma_queue) = crate::music::spawn(&rt, true, &startup.settings.music_assistant, event_tx.clone());
    let lib = if music.is_some() { ui::Library::empty() } else { ui::fake::Library::generate() };
    let ui = ui::Ui::with_assets(lib, assets);
    crate::debug::server::start(&rt, debug_cfg, event_tx.clone(), crate::debug_hello("sim", role, device))?;
    let aux_out = a_aux::loopback::Loopback::new(crate::settings_rt::loopback_config(&startup.settings.aux));
    info!("Masterlink role {role:?}, {device:?} PC2");
    let pc2 = crate::spawn_pc2(&rt, device, role, event_tx.clone());
    let link = Link::new(link::Config::from_settings(&startup.settings));
    let (tuner, radio_tx) = crate::radio_actor::spawn(&rt, &startup.settings.music_assistant, ma_queue, event_tx.clone());
    let airplay: Box<dyn crate::core::AirPlayOut> =
        match crate::core::start_airplay(event_tx.clone(), &startup.settings.aux.playback) {
            Some(r) => Box::new(r),
            None => Box::new(crate::core::NoAirPlay),
        };
    let panel = if real_panel {
        crate::panel_actor::Panel::Usb(crate::panel_actor::spawn(event_tx.clone())?)
    } else {
        crate::panel_actor::Panel::None
    };
    let core =
        startup.attach(&rt, Core::new(ui, panel, aux_out, link, pc2, scene_tx), event_tx.clone()).with_radio(tuner, radio_tx);
    let core = match music {
        Some(m) => core.with_music(m),
        None => core,
    };
    rt.block_on(core.with_airplay(airplay).run(event_rx));
    drop(rt);
    let _ = render_thread.join();
    Ok(())
}

// X keysyms used below
const XK_RETURN: u32 = 0xff0d;
const XK_ESCAPE: u32 = 0xff1b;
const XK_LEFT: u32 = 0xff51;
const XK_UP: u32 = 0xff52;
const XK_RIGHT: u32 = 0xff53;
const XK_DOWN: u32 = 0xff54;
const XK_PAGE_UP: u32 = 0xff55;
const XK_PAGE_DOWN: u32 = 0xff56;
const XK_KP_ADD: u32 = 0xffab;
const XK_KP_SUBTRACT: u32 = 0xffad;

struct Keymap {
    min: u8,
    per: usize,
    syms: Vec<u32>,
}

impl Keymap {
    fn load(conn: &RustConnection) -> Result<Keymap, Box<dyn std::error::Error>> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let r = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        Ok(Keymap { min, per: r.keysyms_per_keycode as usize, syms: r.keysyms })
    }

    /// The unshifted keysym, letters lower-cased.
    fn keysym(&self, code: Keycode) -> u32 {
        let i = (code.saturating_sub(self.min)) as usize * self.per;
        let s = self.syms.get(i).copied().unwrap_or(0);
        if (b'A' as u32..=b'Z' as u32).contains(&s) {
            s + 32
        } else {
            s
        }
    }
}

/// The simulated panel: what a real report would carry.
struct SimPanel {
    pointer: u8,
    buttons: u8,
}

impl SimPanel {
    fn report(&self, menu: i32, volume: i32) -> Event {
        Event::PanelReport(
            Report {
                menu: menu.clamp(-127, 127) as i8,
                volume: volume.clamp(-127, 127) as i8,
                pointer: self.pointer,
                buttons: self.buttons,
            }
            .encode(),
        )
    }
}

fn button_for(sym: u32) -> Option<u8> {
    match sym {
        XK_LEFT => Some(BUTTON_LEFT),
        XK_RIGHT => Some(BUTTON_RIGHT),
        XK_RETURN => Some(BUTTON_GO),
        s if s == b'g' as u32 => Some(BUTTON_GO),
        _ => None,
    }
}

fn input_loop(
    conn: &Arc<RustConnection>,
    window: u32,
    wm_delete: u32,
    tx: &mpsc::UnboundedSender<Event>,
    mut panel: SimPanel,
) -> Result<(), Box<dyn std::error::Error>> {
    let keymap = Keymap::load(conn)?;
    let zones = ui::n_music_zones();
    let send = |e: Event| tx.send(e).map_err(|_| "core stopped");
    loop {
        match conn.wait_for_event()? {
            XEvent::KeyPress(k) if k.event == window => {
                let sym = keymap.keysym(k.detail);
                if let Some(b) = button_for(sym) {
                    if panel.buttons & b == 0 {
                        panel.buttons |= b;
                        send(panel.report(0, 0))?;
                    }
                    continue;
                }
                let ch = char::from_u32(sym).unwrap_or('\0');
                match (sym, ch) {
                    (XK_ESCAPE, _) | (_, 'q') => return Ok(()),
                    (XK_UP, _) => send(panel.report(-COUNTS_PER_STEP, 0))?,
                    (XK_DOWN, _) => send(panel.report(COUNTS_PER_STEP, 0))?,
                    (_, 'a') | (XK_PAGE_UP, _) | (_, 'z') | (XK_PAGE_DOWN, _) => {
                        let d: i32 = match ch {
                            'a' => 1,
                            'z' => -1,
                            _ if sym == XK_PAGE_UP => 6,
                            _ => -6,
                        };
                        panel.pointer = (panel.pointer as i32 + d).clamp(0, POINTER_MAX as i32) as u8;
                        send(panel.report(0, 0))?;
                    }
                    (_, '1'..='8') => {
                        // zone order on the arc, skipping the second now-playing range
                        let i = ch as usize - '1' as usize;
                        if let Some(z) = zones.get(i) {
                            panel.pointer = (z.lo + z.hi) / 2;
                            send(panel.report(0, 0))?;
                        }
                    }
                    (XK_KP_ADD, _) | (_, '+') | (_, '=') => send(panel.report(0, 1))?,
                    (XK_KP_SUBTRACT, _) | (_, '-') => send(panel.report(0, -1))?,
                    (_, 's') if u16::from(k.state) & u16::from(KeyButMask::SHIFT) != 0 => send(Event::AllStandby)?,
                    (_, 's') => send(Event::Standby)?,
                    _ => {}
                }
            }
            XEvent::KeyRelease(k) if k.event == window => {
                if let Some(b) = button_for(keymap.keysym(k.detail)) {
                    panel.buttons &= !b;
                    send(panel.report(0, 0))?;
                }
            }
            XEvent::ButtonPress(b) if b.event == window => match b.detail {
                4 => send(panel.report(-COUNTS_PER_STEP, 0))?,
                5 => send(panel.report(COUNTS_PER_STEP, 0))?,
                _ => {}
            },
            XEvent::Expose(_) => send(Event::Redraw)?,
            XEvent::ClientMessage(m) if m.data.as_data32()[0] == wm_delete => return Ok(()),
            XEvent::Error(e) => warn!("X11 error: {e:?}"),
            _ => {}
        }
    }
}
