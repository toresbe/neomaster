//! neomaster: open-source panel software for the B&O BeoMaster 5.
//!
//! One core task owns all state (see [`core`]); actors turn device IO into [`core::Event`]s and
//! carry out what the core decides. `--sim` replaces the panel and the screen with a desktop
//! window and the keyboard, so the UI can be built without a BM5.

mod core;
mod sim;

use std::process::ExitCode;

const USAGE: &str = "usage: neomaster --sim

  --sim     run the panel UI in a 1024x768 X11 window; the keyboard stands in for the panel
            (see neomaster --keys)
  --keys    print the simulator's key bindings
";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--sim") => match sim::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                log::error!("{e}");
                ExitCode::FAILURE
            }
        },
        Some("--keys") => {
            print!("{}", sim::KEYS);
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            eprintln!("\nThe hardware actors (panel, PC2, Masterlink) arrive with milestone M4; only --sim runs today.");
            ExitCode::FAILURE
        }
    }
}
