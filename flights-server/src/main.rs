//! `flights-server` — the long-running engine and HTTP daemon (ADR-0005). It owns
//! the Source, polls it on its self-chosen cadence (ADR-0002), holds the latest
//! Snapshot in a shared [`Tracker`], and answers every airspace question over a
//! small loopback REST API. Thin Clients (the TUI, the waybar module) only render
//! what it computes; none of them touch a Source.
//!
//! Entry point: load config, read any Source secret from the environment, build
//! the Source, spawn the poller onto a shared `Arc<RwLock<Tracker>>`, and serve
//! the REST API. `--once`/`--print-config` are headless smoke modes.

mod api;
mod config;
mod domain;
mod geo;
mod http;
mod poller;
mod sources;
mod tracker;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, RwLock};

use config::Config;
use domain::Flight;
use poller::PollBounds;
use sources::FlightSource;
use tracker::{Track, Tracker, TrackerConfig};

const USAGE: &str = "\
flights-server — nearest-flight engine + REST API

USAGE:
    flights-server [OPTIONS]

OPTIONS:
    --serve            Poll the Source and serve the REST API (default)
    --once             Fetch a single snapshot, print nearest/pacing, exit
    --print-config     Print the resolved config and exit
    --config <PATH>    Use an explicit config file
    --cors-allow-origin <ORIGIN>
                       Send Access-Control-Allow-Origin: <ORIGIN> (overrides config;
                       e.g. http://127.0.0.1:8080 for the webclient, or *)
    -h, --help         Show this help

Config lives at $XDG_CONFIG_HOME/flights/config.toml (default $HOME/.config).
Set [home] lat/lon to your location; everything else has sensible defaults.";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Serve,
    Once,
    PrintConfig,
    Help,
}

struct Args {
    mode: Mode,
    config_path: Option<PathBuf>,
    /// `--cors-allow-origin <ORIGIN>` override, applied onto the loaded config.
    cors_allow_origin: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut mode = None;
    let mut config_path = None;
    let mut cors_allow_origin = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let new_mode = match arg.as_str() {
            "--serve" => Some(Mode::Serve),
            "--once" => Some(Mode::Once),
            "--print-config" => Some(Mode::PrintConfig),
            "-h" | "--help" => Some(Mode::Help),
            "--config" => {
                config_path = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| "--config requires a path".to_string())?,
                ));
                None
            }
            "--cors-allow-origin" => {
                cors_allow_origin = Some(
                    it.next()
                        .ok_or_else(|| "--cors-allow-origin requires an origin".to_string())?,
                );
                None
            }
            other => return Err(format!("unknown argument: {other}")),
        };
        if let Some(m) = new_mode {
            if matches!(mode, Some(prev) if prev != m) {
                return Err("only one mode may be given".to_string());
            }
            mode = Some(m);
        }
    }
    Ok(Args {
        mode: mode.unwrap_or(Mode::Serve),
        config_path,
        cors_allow_origin,
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

    if args.mode == Mode::Help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let loaded = match Config::load(args.config_path.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    for w in &loaded.warnings {
        eprintln!("warning: {w}");
    }
    match &loaded.source_path {
        Some(p) => eprintln!("config: {}", p.display()),
        None => eprintln!("config: using built-in defaults (no config file found)"),
    }

    let mut cfg = loaded.config;
    // A `--cors-allow-origin` flag overrides the config (an explicit per-run opt-in).
    if let Some(origin) = args.cors_allow_origin {
        if let Err(e) = cfg.override_cors(origin) {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
        eprintln!(
            "CORS: allowing origin {}",
            cfg.server.cors_allow_origin.as_deref().unwrap_or("")
        );
    }
    let result = match args.mode {
        Mode::PrintConfig => {
            println!("{}", cfg.summary());
            Ok(())
        }
        Mode::Once => run_once(&cfg),
        Mode::Serve => run_serve(&cfg),
        Mode::Help => unreachable!("handled above"),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// The poll-interval window: the Source's floor below, the configured quiet
/// cadence above (never below the floor).
fn poll_bounds(cfg: &Config, source: &dyn FlightSource) -> PollBounds {
    let min = source.min_interval();
    PollBounds {
        min,
        max: cfg.max_poll().max(min),
    }
}

/// Tracker tunables derived from config: the relevance cutoff, and a staleness
/// cap of ~2× the max poll interval (both the snapshot-stale flag and the
/// per-flight drop age).
fn tracker_cfg(cfg: &Config) -> TrackerConfig {
    let cap = cfg.max_poll() * 2;
    TrackerConfig {
        relevance_distance_nm: cfg.search.relevance_distance_nm,
        stale_after: cap,
        max_flight_age: cap,
    }
}

/// `--serve` (default): build the Source, spawn the poller onto a shared Tracker,
/// and serve the REST API on the configured bind address. Blocks until killed.
fn run_serve(cfg: &Config) -> anyhow::Result<()> {
    let addr = cfg.bind_addr()?;
    let source = sources::build(cfg)?;
    let area = cfg.search_area();
    let bounds = poll_bounds(cfg, &*source);
    let source_name = source.name().to_string();
    let meta = api::build_meta(cfg, &source_name);

    // Bind before spawning the poller so a port conflict fails fast and cleanly.
    let server = http::bind(addr)?;

    eprintln!(
        "source: {source_name} | poll window {:.2}s–{:.0}s | serving REST API on http://{addr}",
        bounds.min.as_secs_f64(),
        bounds.max.as_secs_f64()
    );

    let tracker = Arc::new(RwLock::new(Tracker::new(area, tracker_cfg(cfg))));
    // The poller writes the shared Tracker; HTTP handler threads read it. Both
    // bindings keep their leading-underscore names (not a bare `_`, which would
    // drop `Shutdown` at once and kill the poller); they live until the blocking
    // `serve` below returns, i.e. until the process is killed.
    let (_poller, _shutdown) = poller::spawn(source, area, bounds, Arc::clone(&tracker));

    http::serve(server, tracker, meta, cfg.server.cors_allow_origin.clone())
}

/// `--once`: one synchronous fetch, printed. A live smoke test of the Source.
fn run_once(cfg: &Config) -> anyhow::Result<()> {
    let source = sources::build(cfg)?;
    let area = cfg.search_area();
    eprintln!("source: {} (polling once)", source.name());

    let snapshot = source.fetch(&area)?;
    let now = snapshot.taken_at;
    let count = snapshot.flights.len();

    let mut tracker = Tracker::new(area, tracker_cfg(cfg));
    tracker.ingest(snapshot);

    println!(
        "{count} airborne flights within {:.0} nm of Home.",
        area.radius_nm
    );
    match tracker.nearest_at(now) {
        Some(t) => println!("Nearest: {}", fmt_position(&t)),
        None => println!("Nearest: (none)"),
    }
    match tracker.pacing_at(now) {
        Some(t) => println!("Pacing : {}", fmt_pacing(&t)),
        None => println!("Pacing : (none — airspace quiet)"),
    }
    Ok(())
}

fn label(f: &Flight) -> String {
    f.ident.clone().unwrap_or_else(|| format!("[{}]", f.hex))
}

fn fmt_position(t: &Track) -> String {
    let alt = t
        .flight
        .altitude_ft
        .map(|a| format!("{a:.0} ft"))
        .unwrap_or_else(|| "alt ?".into());
    let kind = t
        .flight
        .aircraft_type
        .as_deref()
        .map(|k| format!(" {k}"))
        .unwrap_or_default();
    format!(
        "{}{kind} — {:.1} nm @ {:03.0}° ({alt})",
        label(&t.flight),
        t.distance_nm,
        t.bearing_from_home
    )
}

fn fmt_pacing(t: &Track) -> String {
    match t.cpa {
        Some(c) => format!(
            "{} — CPA {:.1} nm in {:.0}s",
            label(&t.flight),
            c.cpa_distance_nm,
            c.time_to_cpa_s
        ),
        None => label(&t.flight),
    }
}
