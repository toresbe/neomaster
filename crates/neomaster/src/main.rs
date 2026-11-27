//! neomaster: open-source panel software for the B&O BeoMaster 5.
//!
//! One core task owns all state (see [`core`]); actors turn device IO into [`core::Event`]s and
//! carry out what the core decides. `--sim` replaces the panel and the screen with a desktop
//! window and the keyboard, so the UI can be built without a BM5.

mod core;
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

const USAGE: &str = "usage: neomaster --sim [--usb] [--role ROLE]
       neomaster --headless [--role ROLE]

  --sim       run the panel UI in a 1024x768 X11 window; the keyboard stands in for the panel
              (see neomaster --keys) and a simulated PC2 for the real one
  --usb       with --sim: drive the real PC2 over USB instead of the simulated one
  --headless  no panel or screen: run the Masterlink loop on the real PC2 only (link rooms,
              IR, standby), e.g. on the BM5 before the panel actor exists
  --role ROLE audio-master or link-source (default: $NEOMASTER_ML_ROLE, else link-source).
              link-source is AUDIO MASTER off: the BM5 provides sources for another
              product's audio master and sends its panel keys to the room's master
  --keys      print the simulator's key bindings
";

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

fn headless(role: LinkRole) -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_time().build()?;
    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut role = std::env::var("NEOMASTER_ML_ROLE").ok().and_then(|r| parse_role(&r)).unwrap_or(LinkRole::SourceCenter);
    let mut device = Pc2Device::Simulated;
    let mut mode = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sim" | "--headless" | "--keys" | "--help" | "-h" => mode = Some(a.as_str()),
            "--usb" => device = Pc2Device::Usb,
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
        Some("--sim") => sim::run(role, device),
        Some("--headless") => headless(role),
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
