use std::process::ExitCode;
use std::sync::mpsc;

use herdr_herd::herdr::{HerdFeed, LiveHerdr};
use herdr_herd::palette::Theme;
use herdr_herd::render;
use herdr_herd::socket::{RealSocket, RpcClient, SocketClient, UnixRpcClient, socket_path};
use herdr_herd::sprite::load_species;
use herdr_herd::watcher::{RealClock, Timings, watch};

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("render") => {
            let cfg = herdr_herd::config::load();
            let species = load_species();
            let (tx, rx) = mpsc::channel();
            let focus = Box::new(LiveHerdr::from_env());
            // The refresh goes over the control socket; the CLI stays wired in
            // behind it so a socket failure degrades the herd instead of
            // stopping it.
            let rpc = UnixRpcClient::from_env().map(|c| Box::new(c) as Box<dyn RpcClient + Send>);
            let feed = HerdFeed::new(rpc, Box::new(LiveHerdr::from_env()));
            let socket: Option<Box<dyn SocketClient + Send>> = socket_path()
                .and_then(|p| RealSocket::connect(&p).ok())
                .map(|s| Box::new(s) as Box<dyn SocketClient + Send>);
            let _watcher = watch(
                feed,
                socket,
                Box::new(RealClock::new()),
                tx,
                Timings::default(),
            );
            match render::run(
                rx,
                species,
                Theme::Dark,
                focus,
                cfg.reduced_motion,
                cfg.renderer,
                cfg.member_scale,
                cfg.sounds,
                Box::new(herdr_herd::sound::SystemSoundPlayer),
                cfg.agent_icon,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-herd: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("place") => {
            let cli = LiveHerdr::from_env();
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| "herdr-herd".to_string());
            let cwd = std::env::current_dir()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| ".".to_string());
            match herdr_herd::place::place(&cli, &self_exe, &cwd) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-herd: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("control") => {
            let cfg = herdr_herd::config::load();
            if !cfg.enabled {
                eprintln!("herdr-herd: disabled by config; not starting the controller");
                return ExitCode::SUCCESS;
            }
            let cli = LiveHerdr::from_env();
            // Reads go over the control socket when there is one; the CLI stays
            // wired in behind it for the mutations and as the fallback.
            let rpc = UnixRpcClient::from_env();
            let self_exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.to_str().map(String::from))
                .unwrap_or_else(|| "herdr-herd".to_string());
            // Resolved once here, not re-read deep in the sweep: the strips
            // this controller spawns get their own fresh shell, which does
            // not inherit this process's env, so without forwarding it every
            // strip would fall back to the real installed plugin's config.
            let config_dir_override = herdr_herd::config::config_dir_override(
                std::env::var("HERDR_HERD_CONFIG_DIR").ok(),
            )
            .and_then(|p| p.to_str().map(String::from));
            let lock_path = herdr_herd::control::controller_lock_path();
            match herdr_herd::control::control(
                rpc.as_ref().map(|c| c as &dyn RpcClient),
                &cli,
                &self_exe,
                config_dir_override.as_deref(),
                &lock_path,
                std::time::Duration::from_millis(cfg.sweep_interval_ms.max(250)),
                cfg.strip_rows,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("herdr-herd: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--version") | Some("-V") => {
            match herdr_herd::marker::build_marker() {
                Some(marker) => println!("herdr-herd {} [{marker}]", env!("CARGO_PKG_VERSION")),
                None => println!("herdr-herd {}", env!("CARGO_PKG_VERSION")),
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: herdr-herd render|place|control");
            ExitCode::FAILURE
        }
    }
}
