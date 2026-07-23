use std::process::ExitCode;
use std::sync::mpsc;

use herdr_pets::herdr::LiveHerdr;
use herdr_pets::palette::Theme;
use herdr_pets::render;
use herdr_pets::socket::{RealSocket, SocketClient, socket_path};
use herdr_pets::sprite::load_species;
use herdr_pets::watcher::{RealClock, watch};

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("render") => {
            let cfg = herdr_pets::config::load();
            let species = load_species();
            let (tx, rx) = mpsc::channel();
            let cli = Box::new(LiveHerdr::from_env());
            let focus = Box::new(LiveHerdr::from_env());
            let socket: Option<Box<dyn SocketClient + Send>> = socket_path()
                .and_then(|p| RealSocket::connect(&p).ok())
                .map(|s| Box::new(s) as Box<dyn SocketClient + Send>);
            let _watcher = watch(cli, socket, Box::new(RealClock::new()), tx, 2500, 250);
            match render::run(rx, species, Theme::Dark, focus, cfg.reduced_motion) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("place") => {
            let cli = LiveHerdr::from_env();
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| "herdr-pets".to_string());
            let cwd = std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| ".".to_string());
            match herdr_pets::place::place(&cli, &self_exe, &cwd) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("control") => {
            let cfg = herdr_pets::config::load();
            if !cfg.enabled {
                eprintln!("herdr-pets: disabled by config; not starting the controller");
                return ExitCode::SUCCESS;
            }
            let cli = LiveHerdr::from_env();
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| "herdr-pets".to_string());
            let lock_path = herdr_pets::control::controller_lock_path();
            match herdr_pets::control::control(
                &cli,
                &self_exe,
                &lock_path,
                std::time::Duration::from_millis(cfg.sweep_interval_ms),
                cfg.strip_rows,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-pets: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--version") | Some("-V") => {
            println!("herdr-pets {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: herdr-pets render|place|control");
            ExitCode::FAILURE
        }
    }
}
