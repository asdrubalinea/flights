# Implementation Plan

The *what* and *why* live in [`CONTEXT.md`](../CONTEXT.md) (glossary) and the ADRs
([0001](adr/0001-handroll-aeroapi-client.md), [0002](adr/0002-adaptive-cpa-paced-polling.md),
[0003](adr/0003-pluggable-data-sources.md), [0004](adr/0004-hybrid-flight-detail-model.md),
[0005](adr/0005-thick-server-rest-split.md), [0006](adr/0006-sync-tiny-http-server.md)).
This document is the *how* — workspace layout, the REST contract, dependencies, and
build order. It does not restate the ADR rationale.

## Target

A long-running **Server** that crunches **Nearest flight** / **Picture** data for a
fixed **Home** from a pluggable **Source** (first: the free, keyless
**airplanes.live** ADS-B API) and exposes it over a small loopback REST API,
consumed by thin Clients: a radar-style TUI, a waybar module, and a webclient later.
Blocking I/O, no async runtime — including the HTTP server (sync `tiny_http`,
ADR-0006). The client/server split rationale is ADR-0005; the hybrid flight-detail
data model the API carries is ADR-0004.

## REST contract (served on `127.0.0.1:7878`, no auth, CORS opt-in via `server.cors_allow_origin`)

| Endpoint        | Returns                                                              |
|-----------------|----------------------------------------------------------------------|
| `/nearest`      | `{ as_of, health, snapshot_age_s, flight \| null }` — the one Nearest|
| `/picture`      | `{ as_of, health, last_error, snapshot_age_s, pacing_hex, tracks[] }`|
| `/flight/{hex}` | one flight's full detail (promoted fields + grouped `details`), or `404` |
| `/meta`         | `{ home, radius_nm, relevance_nm, source, units, version }`          |

Conventions: snake_case with unit suffixes (`distance_nm`, `altitude_ft`,
`groundspeed_kt`, `track_deg`, `age_s`); fixed aviation units (the Client converts);
**epoch-seconds** timestamps (no date/time crate — `SystemTime` suffices); **explicit
`null`** for absent optional fields (stable schema); each flight carries both
estimated `lat`/`lon` *and* `distance_nm`/`bearing_deg`; an empty `/nearest` is
`200` with `"flight": null` (the "if any"). `tracks` is nearest-first, so the Nearest
flight is `tracks[0]`; only `pacing_hex` is called out, as pacing is not derivable
from distance order. The Picture stays one atomic `picture_at(now)` pass so the
radar, list, and status never disagree.

**Flight shape.** Each flight in `/nearest`/`/picture`/`/flight` carries the typed,
cross-provider fields promoted onto the domain `Flight` (ADR-0004): `hex`, `ident`,
`type`, `model`, `registration`, `operator`, `lat`/`lon`, `distance_nm`,
`bearing_deg`, `altitude_ft`, `geometric_altitude_ft`, `groundspeed_kt`, `track_deg`,
`vertical_rate_fpm`, `vertical_trend` (`"climb"|"descend"|"level"|"unknown"` — derived
server-side so the list glyph needs no client logic), `squawk`, `emergency`,
`emitter_category`, `age_s`, and `cpa`. The opaque long tail — the adapter-formatted
`details: [{ title, fields: [{ label, value }] }]` (Signal, Integrity, Navigation,
Provenance, Airframe) shown only in the flight-detail popup — is **not** inlined in
`/picture` (that would be ~40 strings × every flight at the Client's frame rate). It
rides `/flight/{hex}`, fetched once when the popup opens, returning the last-known
detail while the flight is still in the area and `404` once it leaves (the popup's
"left the area" state). Detail values are display-only strings, rendered verbatim,
never parsed back, and never affect the Nearest or Pacing flight.

First source — airplanes.live (and its readsb/tar1090 siblings adsb.lol, adsb.fi,
and a future local dump1090/readsb box, all one adapter by base URL):
`GET http://api.airplanes.live/v2/point/{lat}/{lon}/{radius_nm}` (≤250 nm, 1 req/s,
non-commercial). Response: `{ "ac": [ { hex, flight, lat, lon, alt_baro (number |
"ground"), alt_geom, gs, track, seen_pos, … } ], "now", … }`; keys omitted when
unavailable; `alt_baro == "ground"` ⇒ on-ground (filter out — airborne only).
Fields: <https://airplanes.live/rest-api-adsb-data-field-descriptions/>.

## Workspace layout (three crates)

```
flights-api/             – the REST wire format, serde only; the single shared home
  lib.rs                   Meta, NearestResponse, PictureResponse, WireFlight,
                           FlightDetail, DetailGroup, Units
                           (depends on serde alone — no domain, no engine)

flights-server/          – the engine + HTTP daemon            (bin `flights-server`)
  main.rs                – load config, build Source, spawn poller, serve tiny_http
  config.rs              – Home, Search area, poll, Source, bind addr/port (TOML)
  domain.rs              – LatLon, SearchArea, Flight (typed promotions + opaque
                           DetailGroups, ADR-0004), VerticalTrend, Snapshot  ← contract
  geo.rs                 – haversine, bearing, cpa()                  [pure, unit-tested]
  sources/               – trait FlightSource, SourceError, build(); readsb adapter
    readsb.rs              airplanes.live / adsb.lol / adsb.fi / local (base-URL param);
                           promotes typed fields + builds the grouped DetailGroups
    (later)                opensky.rs, aeroapi.rs …
  tracker.rs             – holds Snapshot; dead_reckon; Picture (nearest/pacing/health)
  poller.rs              – owns Source; writes the shared Tracker; adaptive cadence; backoff
  api.rs                 – map domain Picture/Track → flights-api DTOs (Instant→epoch,
                           vertical_trend, DetailGroups → FlightDetail …)
  http.rs                – tiny_http routes /nearest /picture /flight/{hex} /meta;
                           CORS; JSON encode

flights-tui/             – ratatui radar, thin client            (bin `flights`)
  main.rs                – load client config (server URL, fps), poll /picture, render
  client.rs              – ureq GET helpers + flights-api deserialization
  ui/                    – app.rs · render.rs · event.rs   (radar + list + detail
                           popup; remote data, renders DetailGroups verbatim)

waybar/                  – shell one-liner: curl 127.0.0.1:7878/nearest | jq …

scripts/
  flights-radar          – launch Server + radar TUI together (one command); starts
                           flights-server in the background, waits for /meta, runs the
                           TUI in front, and stops only a Server it started — reusing
                           any already running, never spawning a second poller.
                           `nix run .#radar`; on PATH in the dev shell.
```

Everything above `sources/` depends only on `domain` + the trait. Inside the Server
the poller and the HTTP handlers share one `Arc<RwLock<Tracker>>`: the poller thread
`ingest()`s each Snapshot (and reads it back to compute its own next delay), while
handler threads call `picture_at(now)` to answer requests. The old one-way
poller→UI `std::sync::mpsc` channel is gone — the UI is now a separate process
reached over HTTP.

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

## Dependencies (per workspace crate)

```toml
# flights-api — the wire format, nothing else
serde      = { version = "1", features = ["derive"] }

# flights-server — engine + daemon
ureq       = { version = "3", features = ["json"] }   # Source client; rustls+gzip, sync, no Tokio
tiny_http  = "0.12"                                    # sync HTTP server (ADR-0006)
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
toml       = "0.9"
anyhow     = "1"                                       # app-level
thiserror  = "2"                                       # SourceError + config errors

# flights-tui — thin client
ratatui    = "0.30"                                    # crossterm backend default
ureq       = { version = "3", features = ["json"] }    # talks to the Server, not a Source
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
toml       = "0.9"                                     # client config (server URL, fps)
anyhow     = "1"
```

Gotchas:
- **No separate `crossterm` dep** — ratatui 0.30 re-exports its matching crossterm at
  `ratatui::crossterm`; import events/terminal setup from there.
- ureq's default rustls `ring` provider needs a C compiler at build (Nix devshell has it).
- **No `serde_with`** — `alt_baro = number | "ground"` is handled by a small
  `#[serde(untagged)]` enum (number ⇒ feet, the `"ground"` string ⇒ drop as on-ground),
  so the extra crate isn't needed.

Deliberately **not** used (intentional, not oversight):
- No async/Tokio — blocking `ureq` + `std::thread` poller + a shared
  `Arc<RwLock<Tracker>>` (the old poller→UI `std::sync::mpsc` is gone with the split).
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
9. **Split** (ADR-0005): factor the wire DTOs into `flights-api`; add `api.rs`
   (domain→DTO mapping, including the promoted fields and `DetailGroup`s) and
   `http.rs` (tiny_http routes) to the Server; move the poller onto a shared
   `Arc<RwLock<Tracker>>`; stand up `flights-server` serving
   `/nearest`/`/picture`/`/flight/{hex}`/`/meta` on loopback — validate with
   `curl` + `jq` before touching any Client.
10. **`flights-tui` as a thin client**: drop the embedded engine (sources, poller,
    tracker), poll `/picture` at the configured fps, render the same radar/list/
    status (incl. the vertical-trend glyph) from the response, fetch `/flight/{hex}`
    when the detail popup opens, and add a "server unreachable" state beside
    `stale`/`no_data`.
11. **waybar module**: a shell one-liner over `/nearest`, with a client-side distance
    threshold for "show only when something is close".

## Testing

Unit-test the pure/headless pieces: geo/CPA, dead reckoning, the serde mapping
(against the step-2 golden file), config defaults/validation, and the backoff curve
(`next_delay(pacing_cpa, bounds, backoff_state)`). A live integration test against
airplanes.live should be `#[ignore]` (network/rate-limited), run on demand. For the
split, add a test pinning the wire contract (a known `Picture` → expected JSON via
`api.rs`, including a flight's promoted fields and `DetailGroup`s) and server smoke
tests (`GET /nearest`, and `GET /flight/{hex}` for both a present and an absent hex).

## Tunable config defaults (settle at build time — not architectural)

**Server** config (TOML under `$XDG_CONFIG_HOME/flights/`): Home, Search radius,
Relevance distance, staleness cap ~2×MAX, and the bind address/port (default
`127.0.0.1:7878`). **MIN poll = the active Source's `min_interval()`**
(airplanes.live = 1 s). **MAX poll must be < Search-area transit time** so a fast jet
can't cross unseen. API keys (future paid sources only) come from an **environment
variable** — never committed.

**Client** config: the TUI takes the Server URL and its own render rate (~4 fps) —
fps is a display concern and no longer lives server-side, since the screen is kept
current by re-querying the Server (which costs no Source call). The waybar module
needs only the URL and its own distance threshold.
