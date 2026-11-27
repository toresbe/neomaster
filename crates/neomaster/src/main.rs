//! neomaster: open-source panel software for the B&O BeoMaster 5.
//!
//! One core task owns all state (see [`core`]); actors turn device IO into [`core::Event`]s and
//! carry out what the core decides. `--sim` replaces the panel and the screen with a desktop
//! window and the keyboard, so the UI can be built without a BM5.

mod core;
mod debug;
mod link;
mod pc2_actor;
mod sim;
mod sim_pc2;

use std::process::ExitCode;
use std::sync::mpsc as std_mpsc;

use masterlink::LinkRole;
use tokio::sync::mpsc;

use crate::core::{Core, Event};
use crate::link::{Link, Pc2Cmd};

const USAGE: &str = "usage: neomaster --sim [--usb] [--role ROLE] [DEBUG OPTIONS]
       neomaster --headless [--role ROLE] [DEBUG OPTIONS]
       neomaster debug COMMAND   (the debug client; neomaster debug --help)

  --sim       run the panel UI in a 1024x768 X11 window; the keyboard stands in for the panel
              (see neomaster --keys) and a simulated PC2 for the real one
  --usb       with --sim: drive the real PC2 over USB instead of the simulated one
  --headless  no panel or screen: run the Masterlink loop on the real PC2 only (link rooms,
              IR, standby), e.g. on the BM5 before the panel actor exists
  --role ROLE audio-master or link-source (default: $NEOMASTER_ML_ROLE, else link-source).
              link-source is AUDIO MASTER off: the BM5 provides sources for another
              product's audio master and sends its panel keys to the room's master
  --keys      print the simulator's key bindings

Debug connection (off by default; see doc/debug-connection.md):
  --debug             serve the debug socket ($NEOMASTER_DEBUG=1); it streams panel reports,
                      Masterlink telegrams, state and timings, and takes injected input
  --debug-socket PATH serve it at PATH ($NEOMASTER_DEBUG_SOCKET; default
                      $XDG_RUNTIME_DIR/neomaster-debug.sock or /tmp/neomaster-debug.sock)
  --debug-tcp ADDR    also serve it on TCP, e.g. 0.0.0.0:7155 or just 7155 ($NEOMASTER_DEBUG_TCP);
                      read-only unless --debug-tcp-inject ($NEOMASTER_DEBUG_TCP_INJECT=1)
  --debug-record FILE record everything from start-up to a JSON Lines file
                      ($NEOMASTER_DEBUG_RECORD)

Stock look (B&O's font and textures, not shipped with neomaster):
  neomaster import-stock-assets [FROM [TO]]
              copy them from the BM5's Windows partition, mounted at FROM (default: found in
              /proc/mounts), to TO (default: /var/lib/neomaster/stock-assets as root, else
              ~/.local/share/neomaster/stock-assets), where the UI picks them up
  NEOMASTER_STOCK_ASSETS=DIR  read them from DIR instead (the Windows disk itself works too);
              without either the UI draws look-alikes
";

/// `neomaster import-stock-assets [FROM [TO]]`: copy B&O's UI files off the Windows partition.
fn import_stock_assets(args: &[String]) -> ExitCode {
    if args.len() > 2 || args.iter().any(|a| a.is_empty() || a.starts_with('-')) {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    }
    let from = match args.first() {
        Some(f) => std::path::PathBuf::from(f),
        None => match ui::assets::find_windows_disks().as_slice() {
            [one] => one.clone(),
            [] => {
                eprintln!(
                    "neomaster: no mounted BeoMaster 5 Windows partition found; mount it (read-only is enough, \
                     e.g. mount -o ro /dev/sda1 /mnt/A) or name it: neomaster import-stock-assets /mnt/A"
                );
                return ExitCode::FAILURE;
            }
            many => {
                let list: Vec<String> = many.iter().map(|p| p.display().to_string()).collect();
                eprintln!("neomaster: several candidates ({}); name one", list.join(", "));
                return ExitCode::FAILURE;
            }
        },
    };
    let to = args.get(1).map(std::path::PathBuf::from).unwrap_or_else(ui::assets::default_dir);
    match ui::assets::import(&from, &to) {
        Ok(files) => {
            let mut copied = 0;
            for f in &files {
                match f {
                    ui::assets::Imported::Copied(p) => {
                        copied += 1;
                        println!("copied  {}", p.display());
                    }
                    ui::assets::Imported::Missing(name) => println!("missing {name}"),
                }
            }
            println!("{copied} of {} files from {} in {}", files.len(), from.display(), to.display());
            if copied == 0 {
                return ExitCode::FAILURE;
            }
            if args.get(1).is_some() && std::env::var_os(ui::assets::STOCK_ASSETS_ENV).is_none() {
                println!("(not the default folder: set {}={} to use them)", ui::assets::STOCK_ASSETS_ENV, to.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("neomaster: writing {}: {e}", to.display());
            ExitCode::FAILURE
        }
    }
}

/// Which PC2 the core talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pc2Device {
    Usb,
    Simulated,
}

fn parse_role(s: &str) -> Option<LinkRole> {
    match s {
        "audio-master" | "audio_master" | "am" => Some(LinkRole::AudioMaster),
        // source_center: the C++ neomaster's NEOMASTER_ML_ROLE value
        "link-source" | "link_source" | "source-center" | "source_center" | "sc" => Some(LinkRole::SourceCenter),
        _ => None,
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

fn headless(role: LinkRole, debug_cfg: &debug::Config) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    debug::server::start(&rt, debug_cfg, event_tx.clone(), debug_hello("headless", role, Pc2Device::Usb))?;
    let pc2 = spawn_pc2(&rt, Pc2Device::Usb, role, event_tx.clone());
    // No screen: scenes go nowhere.
    let (scene_tx, scene_rx) = std_mpsc::sync_channel(1);
    std::thread::spawn(move || while scene_rx.recv().is_ok() {});
    struct NoPanel;
    impl core::PanelOut for NoPanel {
        fn write(&mut self, _: [u8; 2]) {}
    }
    let ui = ui::Ui::new(ui::fake::Library::generate());
    let link = Link::new(link::Config::new(role));
    rt.spawn(core::poll_aux_card(event_tx));
    let aux_out = a_aux::loopback::Loopback::new(a_aux::loopback::LoopbackConfig::from_env());
    log::info!("Running headless as {role:?}; Ctrl-C to quit");
    rt.block_on(Core::new(ui, NoPanel, aux_out, link, pc2, scene_tx).run(event_rx));
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "debug") {
        return debug::client::main(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "import-stock-assets") {
        return import_stock_assets(&args[1..]);
    }
    debug::logger::TapLogger::init(&mut env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")));
    let mut debug_cfg = match debug::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("neomaster: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut role = std::env::var("NEOMASTER_ML_ROLE").ok().and_then(|r| parse_role(&r)).unwrap_or(LinkRole::SourceCenter);
    let mut device = Pc2Device::Simulated;
    let mut mode = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sim" | "--headless" | "--keys" | "--help" | "-h" => mode = Some(a.as_str()),
            "--usb" => device = Pc2Device::Usb,
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
            "--role" => match it.next().and_then(|r| parse_role(r)) {
                Some(r) => role = r,
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
    let result = match mode {
        Some("--sim") => sim::run(role, device, &debug_cfg),
        Some("--headless") => headless(role, &debug_cfg),
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
