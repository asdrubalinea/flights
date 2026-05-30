//! `flights` — track the nearest airborne flight to a fixed Home, pacing the
//! polls itself so it stays within a Source's limits and keeps the screen smooth.
//!
//! Entry point: load config, read any Source secret from the environment, build
//! the Source, spawn the poller, and run the chosen mode (TUI by default).

mod config;
mod domain;
mod geo;
mod poller;
mod sources;
mod tracker;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Instant;

use config::Config;
use domain::Flight;
use poller::{PollBounds, PollUpdate};
use sources::FlightSource;
use tracker::{Track, Tracker, TrackerConfig};

const USAGE: &str = "\
flights — nearest-flight radar

USAGE:
    flights [OPTIONS]

OPTIONS:
    --tui              Run the radar TUI (default)
    --headless         Run the poller and print snapshots + chosen cadence
    --once             Fetch a single snapshot, print nearest/pacing, exit
    --print-config     Print the resolved config and exit
    --config <PATH>    Use an explicit config file
    -h, --help         Show this help

Config lives at $XDG_CONFIG_HOME/flights/config.toml (default $HOME/.config).
Set [home] lat/lon to your location; everything else has sensible defaults.";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Tui,
    Headless,
    Once,
    PrintConfig,
    Help,
}

struct Args {
    mode: Mode,
    config_path: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut mode = None;
    let mut config_path = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let new_mode = match arg.as_str() {
            "--tui" => Some(Mode::Tui),
            "--headless" => Some(Mode::Headless),
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
        mode: mode.unwrap_or(Mode::Tui),
        config_path,
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

    let cfg = loaded.config;
    let result = match args.mode {
        Mode::PrintConfig => {
            println!("{}", cfg.summary());
            Ok(())
        }
        Mode::Once => run_once(&cfg),
        Mode::Headless => run_headless(&cfg),
        Mode::Tui => ui::run(&cfg),
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

/// `--once`: one synchronous fetch, printed. Doubles as a live smoke test.
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

/// `--headless`: spawn the poller and print each update with the cadence it
/// chose, proving the rate/cost behavior before any UI. Runs until interrupted.
fn run_headless(cfg: &Config) -> anyhow::Result<()> {
    let source = sources::build(cfg)?;
    let area = cfg.search_area();
    let bounds = poll_bounds(cfg, &*source);
    let tcfg = tracker_cfg(cfg);

    eprintln!(
        "source: {} | poll window {:.2}s–{:.0}s | Ctrl-C to stop",
        source.name(),
        bounds.min.as_secs_f64(),
        bounds.max.as_secs_f64()
    );

    let (tx, rx) = mpsc::channel();
    let (handle, _shutdown) = poller::spawn(source, area, bounds, tcfg, tx);
    let mut tracker = Tracker::new(area, tcfg);
    let start = Instant::now();

    for update in rx {
        let t = start.elapsed().as_secs_f64();
        match update {
            PollUpdate::Snapshot {
                snapshot,
                next_interval,
            } => {
                let now = Instant::now();
                let count = snapshot.flights.len();
                tracker.ingest(snapshot);
                let nearest = tracker
                    .nearest_at(now)
                    .map(|t| fmt_position(&t))
                    .unwrap_or_else(|| "(none)".into());
                let pacing = tracker
                    .pacing_at(now)
                    .map(|t| fmt_pacing(&t))
                    .unwrap_or_else(|| "(quiet)".into());
                println!(
                    "[{t:>7.1}s] {count:>3} flights | nearest {nearest} | pacing {pacing} | next +{:.1}s",
                    next_interval.as_secs_f64()
                );
            }
            PollUpdate::Error { error, retry_in } => {
                tracker.note_error(error.to_string());
                eprintln!(
                    "[{t:>7.1}s] poll error: {error} | retry +{:.1}s",
                    retry_in.as_secs_f64()
                );
            }
        }
    }

    let _ = handle.join();
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
