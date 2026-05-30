//! `flights` — the radar-style TUI Client (ADR-0005). It renders the airspace the
//! `flights-server` computes and never touches a Source itself. On launch it loads
//! its small client config (which Server, what fps), confirms the Server is
//! reachable by fetching `/meta`, then runs the radar.

mod client;
mod config;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use client::Client;
use config::Config;
use ui::App;

const USAGE: &str = "\
flights — nearest-flight radar (thin client for flights-server)

USAGE:
    flights [OPTIONS]

OPTIONS:
    --server <URL>     Server base URL (overrides config; default http://127.0.0.1:7878)
    --config <PATH>    Use an explicit client config file
    -h, --help         Show this help

Client config lives at $XDG_CONFIG_HOME/flights/tui.toml (default $HOME/.config).
The engine runs in flights-server; start that first.";

/// How many times to try reaching the Server at startup before giving up — a
/// short grace window so launching the TUI right after the Server still connects.
const STARTUP_ATTEMPTS: u32 = 5;
const STARTUP_BACKOFF: Duration = Duration::from_millis(400);

struct Args {
    server: Option<String>,
    config_path: Option<PathBuf>,
    help: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut server = None;
    let mut config_path = None;
    let mut help = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => help = true,
            "--server" => {
                server = Some(
                    it.next()
                        .ok_or_else(|| "--server requires a URL".to_string())?,
                );
            }
            "--config" => {
                config_path = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| "--config requires a path".to_string())?,
                ));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        server,
        config_path,
        help,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    if args.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let loaded = match Config::load(args.config_path.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    for w in &loaded.warnings {
        eprintln!("warning: {w}");
    }
    match &loaded.source_path {
        Some(p) => eprintln!("client config: {}", p.display()),
        None => eprintln!("client config: using built-in defaults (no config file found)"),
    }

    let mut cfg = loaded.config;
    if let Some(url) = args.server {
        cfg.server.url = url;
    }

    let client = Client::new(cfg.server.url.clone());

    // Confirm the Server is reachable and pull the static Meta before entering raw
    // mode — the radar needs the Search radius and labels from it, and failing here
    // (rather than inside a blank TUI) gives the user a clear, scrollable message.
    let meta = match connect(&client) {
        Ok(meta) => meta,
        Err(e) => {
            eprintln!(
                "error: could not reach flights-server at {} ({e}).\n\
                 Is it running? Start it with `flights-server`.",
                client.base_url()
            );
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "connected to {} (source: {}, {} nm radius)",
        client.base_url(),
        meta.source,
        meta.radius_nm
    );

    let app = App::new(client, meta, cfg.render_interval());
    match ui::run(app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Try to fetch `/meta`, retrying a few times so a TUI launched alongside the
/// Server still connects once the Server finishes binding.
fn connect(client: &Client) -> Result<flights_api::Meta, client::ClientError> {
    let mut last = None;
    for attempt in 0..STARTUP_ATTEMPTS {
        match client.meta() {
            Ok(meta) => return Ok(meta),
            Err(e) => {
                last = Some(e);
                if attempt + 1 < STARTUP_ATTEMPTS {
                    thread::sleep(STARTUP_BACKOFF);
                }
            }
        }
    }
    Err(last.expect("at least one attempt was made"))
}
