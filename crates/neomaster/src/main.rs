//! neomaster: open-source panel software for the B&O BeoMaster 5.
//!
//! One core task owns all state (see [`core`]); actors turn device IO into [`core::Event`]s and
//! carry out what the core decides. `--sim` replaces the panel and the screen with a desktop
//! window and the keyboard, so the UI can be built without a BM5.

mod core;
mod debug;
mod link;
mod music;
mod panel_actor;
mod pc2_actor;
mod radio_actor;
mod settings_rt;
mod sim;
mod sim_pc2;

use std::process::ExitCode;

use std::path::PathBuf;

use masterlink::LinkRole;
use tokio::sync::{mpsc, watch};

use crate::core::{Core, Event};
use crate::link::{Link, Pc2Cmd};

const USAGE: &str = "usage: neomaster --sim [--usb] [--panel] [--role ROLE] [--config FILE] [DEBUG OPTIONS]
       neomaster --headless [--role ROLE] [--config FILE] [DEBUG OPTIONS]
       neomaster debug COMMAND   (the debug client; neomaster debug --help)

  --sim       run the panel UI in a 1024x768 X11 window; the keyboard stands in for the panel
              (see neomaster --keys) and a simulated PC2 for the real one
  --usb       with --sim: drive the real PC2 over USB instead of the simulated one
  --panel     with --sim: read the real BeoSound 5 panel (USB 0cd4:1112) as well as the
              keyboard, and send it clicks and backlight; on the BM5, run fullscreen on the
              panel's own display. Needs root or a udev rule; takes the panel from usbhid
  --headless  no panel or screen: run the Masterlink loop on the real PC2 only (link rooms,
              IR, standby), e.g. on the BM5 before the panel actor exists
  --role ROLE audio-master or link-source, for this run only (default: Settings > Roles >
              AUDIO MASTER, or $NEOMASTER_ML_ROLE). link-source is AUDIO MASTER off: the BM5
              provides sources for another product's audio master and sends its panel keys
              to the room's master
  --config FILE  the settings file (default: $NEOMASTER_CONFIG, else
              /etc/neomaster/settings.toml as root, else ~/.config/neomaster/settings.toml)
  --keys      print the simulator's key bindings

AirPlay (on by default; see doc/airplay.md): neomaster runs shairport-sync and takes the
source when a device streams to it. NEOMASTER_AIRPLAY=0 turns it off; NEOMASTER_AIRPLAY_NAME,
_DEVICE (ALSA output), _BACKEND, _PORT, _COMMAND and _DIR configure it.

Debug connection (off by default; see doc/debug-connection.md):
  --debug             serve the debug socket ($NEOMASTER_DEBUG=1); it streams panel reports,
                      Masterlink telegrams, state and timings, and takes injected input
  --debug-socket PATH serve it at PATH ($NEOMASTER_DEBUG_SOCKET; default
                      $XDG_RUNTIME_DIR/neomaster-debug.sock or /tmp/neomaster-debug.sock)
  --debug-tcp ADDR    also serve it on TCP, e.g. 0.0.0.0:7155 or just 7155 ($NEOMASTER_DEBUG_TCP);
                      read-only unless --debug-tcp-inject ($NEOMASTER_DEBUG_TCP_INJECT=1)
  --debug-record FILE record everything from start-up to a JSON Lines file
                      ($NEOMASTER_DEBUG_RECORD)

  MA_URL, MA_TOKEN  Music Assistant: browse its library, play on the BM5's built-in Sendspin
              player (see doc/n-music.md for the other NEOMASTER_* player settings); without
              them the panel browses a made-up library and nothing plays

  NEOMASTER_STOCK_ASSETS=DIR  with --sim: draw with the BM5's own font and textures, read from
              DIR (its Windows disk, e.g. /mnt/A); without it the UI draws look-alikes
";

/// Which PC2 the core talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pc2Device {
    Usb,
    Simulated,
}

/// Everything the core needs from the settings, loaded once at start.
pub struct Startup {
    pub settings: settings::Settings,
    pub persister: settings_rt::Persister,
}

impl Startup {
    pub fn role(&self) -> LinkRole {
        self.settings.roles.link_role()
    }

    /// Wire the settings into a core and start the A.AUX card poll that follows them.
    pub fn attach<P: core::PanelOut, A: core::AuxOut, Q: core::Pc2Out>(
        self,
        rt: &tokio::runtime::Runtime,
        core: Core<P, A, Q>,
        events: mpsc::UnboundedSender<Event>,
    ) -> Core<P, A, Q> {
        let (card_tx, card_rx) = watch::channel(self.settings.aux.card.clone());
        rt.spawn(core::poll_aux_card_choice(events.clone(), card_rx));
        core.with_settings(self.settings, Box::new(self.persister), events, Some(card_tx))
    }
}

/// Start the PC2 actor on the runtime: returns the core's command sender.
pub fn spawn_pc2(
    rt: &tokio::runtime::Runtime,
    device: Pc2Device,
    role: LinkRole,
    events: mpsc::UnboundedSender<Event>,
) -> mpsc::UnboundedSender<Pc2Cmd> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Pc2Cmd>();
    let mode = link::Config::new(role).pc2_mode();
    let _guard = rt.enter();
    match device {
        Pc2Device::Usb => {
            rt.spawn(pc2_actor::supervise(mode, 0, pc2_actor::open_usb, cmd_rx, events));
        }
        Pc2Device::Simulated => {
            // One simulated PC2 that never goes away
            let mut sim = Some(sim_pc2::spawn(None).0);
            let open = move || {
                let pipes = sim.take().ok_or(pc2_actor::OpenError::NotFound);
                async move { pipes }
            };
            rt.spawn(pc2_actor::supervise(mode, 0, open, cmd_rx, events));
        }
    }
    cmd_tx
}

/// What a debug client's `hello` says about this run.
pub fn debug_hello(mode: &str, role: LinkRole, device: Pc2Device) -> serde_json::Value {
    serde_json::json!({ "mode": mode, "role": format!("{role:?}"), "pc2_device": format!("{device:?}") })
}

fn headless(startup: Startup, debug_cfg: &debug::Config) -> Result<(), Box<dyn std::error::Error>> {
    let role = startup.role();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    debug::server::start(&rt, debug_cfg, event_tx.clone(), debug_hello("headless", role, Pc2Device::Usb))?;
    let pc2 = spawn_pc2(&rt, Pc2Device::Usb, role, event_tx.clone());
    // No screen: scenes go nowhere.
    let (scene_tx, scene_rx) = render::mailbox::channel();
    std::thread::spawn(move || while scene_rx.recv().is_some() {});
    struct NoPanel;
    impl core::PanelOut for NoPanel {
        fn write(&mut self, _: [u8; 2]) {}
    }
    let (music, ma_queue) = music::spawn(&rt, false, &startup.settings.music_assistant, event_tx.clone());
    let lib = if music.is_some() { ui::Library::empty() } else { ui::fake::Library::generate() };
    let ui = ui::Ui::new(lib);
    let link = Link::new(link::Config::from_settings(&startup.settings));
    let (tuner, radio_tx) = radio_actor::spawn(&rt, &startup.settings.music_assistant, ma_queue, event_tx.clone());
    let aux_out = a_aux::loopback::Loopback::new(settings_rt::loopback_config(&startup.settings.aux));
    log::info!("Running headless as {role:?}; Ctrl-C to quit");
    let airplay: Box<dyn core::AirPlayOut> = match core::start_airplay(event_tx.clone(), &startup.settings.aux.playback) {
        Some(r) => Box::new(r),
        None => Box::new(core::NoAirPlay),
    };
    let core = startup.attach(&rt, Core::new(ui, NoPanel, aux_out, link, pc2, scene_tx), event_tx).with_radio(tuner, radio_tx);
    let core = match music {
        Some(m) => core.with_music(m),
        None => core,
    };
    rt.block_on(core.with_airplay(airplay).run(event_rx));
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "debug") {
        return debug::client::main(&args[1..]);
    }
    // Built at debug level so DEBUG LOGGING can raise the cap later (settings_rt).
    debug::logger::TapLogger::init(&mut env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")));
    settings_rt::set_debug_logging(false);
    let mut debug_cfg = match debug::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("neomaster: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut role = None;
    let mut config = None;
    let mut device = Pc2Device::Simulated;
    let mut mode = None;
    let mut real_panel = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sim" | "--headless" | "--keys" | "--help" | "-h" => mode = Some(a.as_str()),
            "--usb" => device = Pc2Device::Usb,
            "--panel" => real_panel = true,
            "--debug" => debug_cfg.socket = Some(debug_cfg.socket.take().unwrap_or_else(debug::default_socket)),
            "--debug-tcp-inject" => debug_cfg.tcp_inject = true,
            "--debug-socket" | "--debug-tcp" | "--debug-record" => {
                let Some(v) = it.next() else {
                    eprint!("{USAGE}");
                    return ExitCode::FAILURE;
                };
                match a.as_str() {
                    "--debug-socket" => debug_cfg.socket = Some(v.into()),
                    "--debug-record" => debug_cfg.record = Some(v.into()),
                    _ => match debug::server::parse_tcp(v) {
                        Ok(addr) => debug_cfg.tcp = Some(addr),
                        Err(e) => {
                            eprintln!("neomaster: --debug-tcp: {e}");
                            return ExitCode::FAILURE;
                        }
                    },
                }
            }
            "--config" => match it.next() {
                Some(p) => config = Some(PathBuf::from(p)),
                None => {
                    eprint!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            "--role" => match it.next().and_then(|r| settings::env::parse_role(r)) {
                Some(r) => role = Some(r),
                None => {
                    eprint!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            _ => {
                eprint!("{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let startup = || {
        let (settings, persister) = settings_rt::load(config.clone().unwrap_or_else(settings::store::default_path), role);
        Startup { settings, persister }
    };
    let result = match mode {
        Some("--sim") => sim::run(startup(), device, real_panel, &debug_cfg),
        Some("--headless") => headless(startup(), &debug_cfg),
        Some("--keys") => {
            print!("{}", sim::KEYS);
            return ExitCode::SUCCESS;
        }
        Some(_) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        None => {
            eprint!("{USAGE}");
            eprintln!("\nThe panel actor arrives with milestone M4; --headless runs the Masterlink loop on the PC2.");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}
