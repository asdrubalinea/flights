//! `flights-waybar` — the Waybar status module (ADR-0008). A **one-shot Client**:
//! on each Waybar `interval` it runs once, does a single `GET /nearest` against an
//! always-on `flights-server`, prints one Waybar JSON object (`{text, tooltip,
//! class}`), and exits. It shows the **Nearest flight** only while within a
//! Client-side **Display range**, in the project's aviation units, with the full
//! `Alt/Spd/Trk/Vr` detail in the tooltip.
//!
//! It **never starts a Server** (ADR-0008): a ~1 Hz module that spawned one would
//! swarm pollers against a rate-limited Source and blow the single-poller budget the
//! whole split protects (ADR-0005). An unreachable Server renders as the dim `error`
//! stub. The Server is expected to run as a separately-managed always-on service —
//! the flake's `homeManagerModules.default` delivers it as a systemd user service.
//!
//! Headless contract: from the moment the args parse, we always print exactly one
//! Waybar JSON line to stdout and exit `0`, so even a misconfiguration surfaces as
//! the `error` stub in the bar rather than a blank module. Warnings and error detail
//! go to stderr (Waybar's log), never stdout.

mod bar;
mod client;
mod config;

use std::path::PathBuf;
use std::process::ExitCode;

use bar::Bar;
use client::Client;
use config::Config;

const USAGE: &str = "\
flights-waybar — nearest-flight status module (thin one-shot client for flights-server)

USAGE:
    flights-waybar [OPTIONS]

Prints one Waybar JSON object ({text, tooltip, class}) for the Nearest flight, or an
empty module when the sky is quiet (or the nearest flight is beyond the Display range).

OPTIONS:
    --server <URL>          Server base URL (overrides config; default http://127.0.0.1:7878)
    --display-range <NM>    Show the nearest flight only within this many nm (default 35)
    --config <PATH>         Use an explicit client config file
    -h, --help              Show this help

Client config lives at $XDG_CONFIG_HOME/flights/waybar.toml (default $HOME/.config).
The engine runs in flights-server, which must already be running — this module never
starts one. Wire it into Waybar with a custom/flights module (see the README).";

struct Args {
    server: Option<String>,
    display_range: Option<f64>,
    config_path: Option<PathBuf>,
    help: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut server = None;
    let mut display_range = None;
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
            "--display-range" => {
                let raw = it
                    .next()
                    .ok_or_else(|| "--display-range requires a distance in nm".to_string())?;
                let nm: f64 = raw
                    .parse()
                    .map_err(|_| format!("--display-range expects a number, got {raw:?}"))?;
                if !(nm.is_finite() && nm > 0.0) {
                    return Err(format!(
                        "--display-range must be a positive distance, got {nm}"
                    ));
                }
                display_range = Some(nm);
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
        display_range,
        config_path,
        help,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            // An invocation error is the user driving the CLI by hand, not the Waybar
            // runtime — report it plainly and fail, rather than emit an error stub.
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    if args.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let bar = run(&args);
    // The single Waybar JSON line. Serialization can't fail for `Bar` (three owned
    // strings), so an error here is a bug, not a runtime condition.
    println!(
        "{}",
        serde_json::to_string(&bar).expect("Bar always serializes")
    );
    ExitCode::SUCCESS
}

/// Load config, apply the flag overrides, fire the single `/nearest`, and render —
/// every failure folded into a [`Bar`] so the caller always has one line to print.
fn run(args: &Args) -> Bar {
    let loaded = match Config::load(args.config_path.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("flights-waybar: {e:#}");
            return Bar::error(&format!("config error: {e}"));
        }
    };
    // Surface clamps/warnings to Waybar's log, but never the routine config-source
    // line: this process runs ~once a second, and an info line per tick would flood it.
    for w in &loaded.warnings {
        eprintln!("flights-waybar: warning: {w}");
    }

    let mut cfg = loaded.config;
    if let Some(url) = &args.server {
        cfg.server.url = url.clone();
    }
    if let Some(range) = args.display_range {
        cfg.display.range_nm = range;
    }

    let client = Client::new(cfg.server.url.clone());
    let outcome = client.nearest();
    bar::render(&outcome, cfg.display.range_nm)
}
