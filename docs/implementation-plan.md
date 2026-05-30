# Implementation Plan

The *what* and *why* live in [`CONTEXT.md`](../CONTEXT.md) (glossary) and the ADRs
([0001](adr/0001-handroll-aeroapi-client.md), [0002](adr/0002-adaptive-cpa-paced-polling.md),
[0003](adr/0003-pluggable-data-sources.md)). This document is the *how* — module
layout, dependencies, and build order. It does not restate the ADR rationale.

## Target

A long-running radar-style TUI showing the **Nearest flight** to **Home**, sourced
from a pluggable **Source** (first: the free, keyless **airplanes.live** ADS-B API),
eventually feeding a waybar module. Blocking I/O, no async runtime.

First source — airplanes.live (and its readsb/tar1090 siblings adsb.lol, adsb.fi,
and a future local dump1090/readsb box, all one adapter by base URL):
`GET http://api.airplanes.live/v2/point/{lat}/{lon}/{radius_nm}` (≤250 nm, 1 req/s,
non-commercial). Response: `{ "ac": [ { hex, flight, lat, lon, alt_baro (number |
"ground"), alt_geom, gs, track, seen_pos, … } ], "now", … }`; keys omitted when
unavailable; `alt_baro == "ground"` ⇒ on-ground (filter out — airborne only).
Fields: <https://airplanes.live/rest-api-adsb-data-field-descriptions/>.

## Module layout (binary crate)

```
src/
  main.rs      – load config, read source secrets from env, build Source, spawn poller, run UI
  config.rs    – Config; active Source + per-source settings (TOML under $XDG_CONFIG_HOME)
  domain.rs    – LatLon, SearchArea{center, radius_nm}, Flight, Snapshot   ← stable contract
  geo.rs       – haversine, bearing, cpa()                                 [pure, unit-tested]
  sources/
    mod.rs     – trait FlightSource, enum SourceError, build(cfg) factory
    readsb.rs  – airplanes.live / adsb.lol / adsb.fi / local dump1090 (base-URL param)
    (later)    – opensky.rs, aeroapi.rs, fr24.rs …
  tracker.rs   – holds Snapshot; dead_reckon; nearest; pacing; staleness
  poller.rs    – Box<dyn FlightSource>; adaptive cadence (≥ source.min_interval()); backoff
  ui/          – app.rs · render.rs · event.rs   (ratatui canvas radar + list + status)
  output.rs    – (deferred) waybar mode
```

Everything above `sources/` depends only on `domain` + the trait. Poller→UI is
one-way over `std::sync::mpsc` (`PollUpdate::{Snapshot, Error}`); the poller retains
the last snapshot to compute its own next delay (no back-channel).

## The source seam

```rust
pub struct SearchArea { pub center: LatLon, pub radius_nm: f64 }   // domain speaks radius
pub enum SourceError { RateLimited { retry_after: Option<Duration> }, Unavailable, Auth, Decode(String), Transient }
pub trait FlightSource: Send {
    fn name(&self) -> &str;
    fn min_interval(&self) -> Duration;                       // source's own floor (ADR-0002 lower bound)
    fn fetch(&self, area: &SearchArea) -> Result<Snapshot, SourceError>;
}
```

Box-only sources (OpenSky) convert radius→bbox and filter back inside their adapter.
Per-source auth (OAuth, paid keys) stays inside the adapter.

## Dependencies (7 core crates)

```toml
ureq       = { version = "3", features = ["json"] }   # rustls+gzip default; sync, no Tokio
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
ratatui    = "0.30"  # crossterm backend default
toml       = "0.9"
anyhow     = "1"      # app-level
thiserror  = "2"      # SourceError + config errors
```

Gotchas:
- **No separate `crossterm` dep** — ratatui 0.30 re-exports its matching crossterm at
  `ratatui::crossterm`; import events/terminal setup from there.
- ureq's default rustls `ring` provider needs a C compiler at build (Nix devshell has it).
- **No `serde_with`** — `alt_baro = number | "ground"` is handled by a small
  `#[serde(untagged)]` enum (number ⇒ feet, the `"ground"` string ⇒ drop as on-ground),
  so the extra crate isn't needed.

Deliberately **not** used (intentional, not oversight):
- No async/Tokio — blocking `ureq` + `std::thread` poller + `std::sync::mpsc`.
- No date/time crate — airplanes.live gives relative `seen_pos` + epoch `now`, so
  `std::time::{Instant, Duration}` suffices. A future RFC3339 source adds its own.
- No `geo` crate — haversine/CPA are hand-rolled in `geo.rs`.
- No `backoff` crate — hand-rolled exponential backoff with `Duration`.
- `directories` optional — `std::env` `XDG_CONFIG_HOME` → `$HOME/.config` is enough.
- Logging deferred — stdout is the UI; when needed, log to a file via
  `tracing` + `tracing-subscriber` + `tracing-appender`.

## Build order (de-risk the non-TUI parts first)

1. Config + env-key plumbing; print resolved config (minus secrets).
2. airplanes.live adapter + **validate JSON against a live payload**; save a golden
   file for parse tests.
3. `geo.rs` (haversine, CPA) with table-driven unit tests.
4. `tracker.rs` + dead reckoning, unit-tested (advance time → position moves along
   heading at groundspeed).
5. Poller thread + adaptive scheduler + backoff — **headless run** printing
   cadence/nearest, proving rate/cost behavior before any UI.
6. TUI shell: layout + event loop + quit; render list + status first.
7. Radar canvas: Home, range rings, dead-reckoned blips, heading vectors, labels on
   flagged flights (north-up, range = search radius).
8. Robustness: offline/stale flags, anonymous blips for blocked idents, selection
   highlight, panic-hook terminal restore.
9. (Deferred) waybar output mode.

## Testing

Unit-test the pure/headless pieces: geo/CPA, dead reckoning, the serde mapping
(against the step-2 golden file), config defaults/validation, and the backoff curve
(`next_delay(pacing_cpa, bounds, backoff_state)`). A live integration test against
airplanes.live should be `#[ignore]` (network/rate-limited), run on demand.

## Tunable config defaults (settle at build time — not architectural)

Search radius, Relevance distance, render ~4 fps, staleness cap ~2×MAX. **MIN poll =
the active Source's `min_interval()`** (airplanes.live = 1 s). **MAX poll must be <
Search-area transit time** so a fast jet can't cross unseen. API keys (future paid
sources only) come from an **environment variable** — never committed; other settings
live in TOML under `$XDG_CONFIG_HOME/flights/`.
