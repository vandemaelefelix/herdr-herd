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
            let species = load_species();
            let (tx, rx) = mpsc::channel();
            let cli = Box::new(LiveHerdr::from_env());
            let focus = Box::new(LiveHerdr::from_env());
            let socket: Option<Box<dyn SocketClient + Send>> = socket_path()
                .and_then(|p| RealSocket::connect(&p).ok())
                .map(|s| Box::new(s) as Box<dyn SocketClient + Send>);
            let _watcher = watch(cli, socket, Box::new(RealClock::new()), tx, 2500, 250);
            match render::run(rx, species, Theme::Dark, focus) {
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
            eprintln!("usage: herdr-pets render");
            ExitCode::FAILURE
        }
    }
}
