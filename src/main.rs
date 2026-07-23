use std::process::ExitCode;

use herdr_pets::herdr::LiveHerdr;
use herdr_pets::render;

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("render") => {
            let herdr = LiveHerdr::from_env();
            match render::run(&herdr) {
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
