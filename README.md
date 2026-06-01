<div align="center">

# ✈ Flights

### A radar scope for the sky above your house.

**Flights** tracks the nearest aircraft to a fixed home location in real time —
a long-running engine that polls a free ADS-B feed, decides *for itself* how
often to look, and serves one self-consistent picture of the airspace to as many
thin clients as you like: a radar-style terminal UI, a status-bar module, and
(soon) a web client.

[![License: MIT](https://img.shields.io/badge/License-MIT-success.svg)](#license)
[![Rust](https://img.shields.io/badge/Rust-2021-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Built with ratatui](https://img.shields.io/badge/TUI-ratatui-blueviolet.svg)](https://ratatui.rs)
[![No async](https://img.shields.io/badge/runtime-sync%20%C2%B7%20no%20Tokio-informational.svg)](docs/adr/0006-sync-tiny-http-server.md)
[![Data: airplanes.live](https://img.shields.io/badge/data-airplanes.live-blue.svg)](https://airplanes.live)

</div>

---

```
┌─ radar · north-up ───────────────────────────┐ ┌─ flights · 6 ────────────────────┐
│                      · N                      │ │▶ RYR4GH    3.1nm 012°  37000ft↑ … │
│                  ╭──────────╮                 │ │  BAW283   11.4nm 248°  24000ft↓ … │
│              ╭───┤   · ·     ├───╮            │ │  EZY88QK  18.7nm 305°  FL310 –    │
│          ╭───┤   │      ╲RYR4GH │   ├───╮     │ │  [a1b2c3] 22.0nm 119°  ?          │
│      W ───┤   │   │       ⌂      │   │   ├─ E  │ │  WZZ1123  41.9nm 077°  18000ft↑ … │
│          ╰───┤   │   ╲          │   ├───╯     │ │  N292WN   62.3nm 201°  9000ft↓  … │
│              ╰───┤  INBOUND→    ├───╯         │ └──────────────────────────────────┘
│                  ╰──────────╯                 │ ┌─ status ─────────────────────────┐
│                      · S                      │ │ source airplanes.live   [LIVE]    │
│                                               │ │ 6 flights · 2s ago                │
│   ⌂ home   • approaching   • receding         │ │ nearest  RYR4GH — 3.1 nm @ 012°   │
│   ● nearest  ● pacing      ● selected         │ │ pacing   INBOUND — CPA 1.8nm/97s  │
│                                               │ │ relevance 30nm                    │
│                                               │ │ ↑/↓ select · Enter detail · q quit│
└───────────────────────────────────────────────┘ └──────────────────────────────────┘
```

> *The radar above is an illustration of the live TUI — north-up scope with range
> rings on the left, the flight list and a status block on the right.*

---

## Why it's different

Most plane-spotting tools are either a stateless script that hammers an API on a
fixed timer, or a heavy web stack. Flights takes a different shape:

- **🛰 It paces itself.** The engine watches the *closest point of approach* of
  every inbound flight and polls faster when something interesting is about to
  pass, slower when the sky is quiet — always inside the data source's rate
  limit. No fixed timer, no wasted calls. ([ADR-0002](docs/adr/0002-adaptive-cpa-paced-polling.md))
- **🧮 It computes once, renders many.** One process owns the data and does all
  the geometry — distance, bearing, dead-reckoning, CPA. Every client just draws
  the answer. Open the TUI *and* the status-bar module and you still poll the
  source **once**. ([ADR-0005](docs/adr/0005-thick-server-rest-split.md))
- **🎯 It dead-reckons between polls.** Blips glide smoothly at whatever frame
  rate you like, because the screen is refreshed by re-asking the local server —
  which extrapolates each aircraft from its last known heading and speed — not by
  spending an API call.
- **🔌 It's source-agnostic.** airplanes.live today; adsb.lol, adsb.fi, or your
  own dump1090 / readsb receiver by changing one line of config. A local receiver
  has no rate limit, so the engine simply polls it faster.
- **🦀 It's small and sharp.** Pure synchronous Rust — no async runtime, no
  database, a size-optimized release binary. The server is a few worker threads
  around a single shared snapshot. ([ADR-0006](docs/adr/0006-sync-tiny-http-server.md))

---

## How it works

```
   ADS-B sources                  the one engine                       thin clients
 ─────────────────        ────────────────────────────         ──────────────────────────

  airplanes.live  ┐       ┌────────────────────────────┐       ┌─────────────────────────┐
  adsb.lol        │       │       flights-server        │   ┌──▶│  radar TUI  (flights)   │
  adsb.fi         ├──poll▶│  • owns the Source          │   │   ├─────────────────────────┤
  local readsb /  │       │  • CPA-paced self-scheduling│───┼──▶│  waybar status module   │
  dump1090        ┘       │  • holds one Snapshot       │   │   ├─────────────────────────┤
                          │  • dead-reckons on read     │   └──▶│  web client   (planned) │
                          │  • loopback REST API :7878  │       └─────────────────────────┘
                          └────────────────────────────┘
                                     GET /picture · /nearest · /flight/{hex} · /meta
```

The **server** is the only thing that ever touches a data source. It holds the
latest *Snapshot* of the airspace, dead-reckons it to the instant of each request,
and answers a small read-only REST API on loopback. **Clients** only render — they
choose *what* to show and *how often* to ask, never *how* the numbers are computed,
so the radar, the list, and the status line can never disagree.

See **[CONTEXT.md](CONTEXT.md)** for the full domain vocabulary and
**[docs/adr/](docs/adr/)** for the architecture decisions behind each of these
choices.

---

## Quick start

### With Nix (recommended)

```sh
# Launch the engine and the radar together, one command:
nix run github:you/flights#radar          # or: nix run .#radar  in a checkout

# Or just the TUI (expects a flights-server already running):
nix run .
```

`#radar` starts `flights-server` in the background, waits for it to bind, then
opens the radar in the foreground. If a server is already running (say, for your
status bar) it's **reused** — never a second poller against a rate-limited feed.

### With Cargo

```sh
# Terminal 1 — the engine + REST API
cargo run -p flights-server

# Terminal 2 — the radar TUI
cargo run -p flights-tui

# …or both at once via the launcher script:
./scripts/flights-radar
```

That's it — it runs out of the box. The **one** thing worth setting is where you
live (see [Configuration](#configuration)).

---

## The clients

### 📟 Radar TUI (`flights`)

A north-up radar scope with range rings beside a live flight list and a status
block. Everything comes from a single picture the server computed, so it's always
internally consistent.

| Key            | Action                                        |
| -------------- | --------------------------------------------- |
| `↑` / `↓` (`j`/`k`) | Select a flight in the list             |
| `Enter`        | Open the full flight-detail popup             |
| `Esc`          | Clear selection · close the popup             |
| `PgUp`/`PgDn`  | Scroll the detail popup                        |
| `q` / `Ctrl-C` | Quit                                          |

Blips are colour-coded — **nearest**, **pacing** (the flight setting the poll
cadence), **selected**, plus approaching vs. receding — and carry a short heading
vector scaled to groundspeed. The detail popup shows everything the source knows
about an aircraft, including opaque per-source telemetry rendered verbatim.

### 📊 waybar status module (`flights-waybar`)

A one-liner for your status bar showing just the **nearest flight** — but only while
it sits within a **display range** you set (35 nm by default), so the module stays
empty until something is genuinely overhead. On each Waybar tick it does a single
`GET /nearest` and prints a Waybar JSON object; the full `Alt/Spd/Trk/Vr` detail
rides the tooltip.

It's a small Rust binary, not a `curl | jq` line: it reuses the same `flights-api`
wire types as the TUI, so the schema is checked against the contract rather than
hand-mirrored ([ADR-0008](docs/adr/0008-waybar-bar-client.md)). And unlike the TUI
and web launchers it **never starts a server** — a module firing every few seconds
can't spawn a poller each tick without blowing the single-poller budget. It reads an
**always-on** `flights-server` (see below) or, failing that, shows a dim error stub.

Wire it into your Waybar config:

```jsonc
// ~/.config/waybar/config.jsonc
"custom/flights": {
  "exec": "flights-waybar",
  "return-type": "json",
  "interval": 5
}
```

Style the states in your Waybar CSS — the module sets a `class` of `lost` (the
nearest flight froze: retained and badged, never dropped), `stale` (the whole picture
has aged), or `error` (server unreachable / no data). An empty sky emits empty text
and Waybar collapses the module:

```css
#custom-flights.lost  { color: #928374; }
#custom-flights.stale { color: #d79921; }
#custom-flights.error { color: #cc241d; }
```

#### Always-on server via Home Manager

The flake exports `homeManagerModules.default`, which runs `flights-server` as a
**systemd user service** (started with your graphical session, beside Waybar) and
puts `flights-waybar` (and `flights`) on your PATH:

```nix
# home.nix — with the flake added to your inputs as `flights`
{ inputs, ... }:
{
  imports = [ inputs.flights.homeManagerModules.default ];
  services.flights-server.enable = true;
}
```

`programs.waybar` stays yours — add the `custom/flights` snippet above to it. Set
`[home]` in `~/.config/flights/config.toml` so the server measures from your location.

### 🌐 Web client — *planned*

The REST contract was designed for it; CORS is already opt-in on the server.

---

## Configuration

Everything but secrets lives in TOML under `$XDG_CONFIG_HOME/flights/`
(falling back to `~/.config/flights/`). **Every value has a default**, so the
only thing worth setting is `[home]`.

### Server — `config.toml`

```toml
[home]                       # the fixed point all distances are measured from
lat = 51.4700
lon = -0.4543

[search]
radius_nm = 100.0            # how far out to look
relevance_distance_nm = 30.0 # CPA cutoff that gates pacing (never the display)

[poll]
max_interval_secs = 60.0     # slowest cadence, when the sky is quiet

[server]
bind = "127.0.0.1:7878"      # loopback by default — that's what makes no-auth safe
# cors_allow_origin = "https://flights.example"   # off by default

[source]
kind = "airplanes_live"      # or point base_url at adsb.lol / adsb.fi / a local box
# base_url = "http://192.168.1.50/data/aircraft.json"
# min_interval_ms = 250      # a local receiver has no rate limit — poll it fast
```

Values are validated and gently clamped on load, with reasons surfaced as
warnings. Run `flights-server --print-config` to see the resolved configuration.

### TUI — `tui.toml`

```toml
[server]
url = "http://127.0.0.1:7878"  # which engine to read

[render]
fps = 4                        # redraw rate — costs zero API calls
```

### waybar module — `waybar.toml`

```toml
[server]
url = "http://127.0.0.1:7878"  # which engine to read

[display]
range_nm = 35.0                # stay empty until the nearest flight is this close
```

Both can be overridden per-run: `flights-waybar --server <URL> --display-range <NM>`.

---

## REST API

Read-only, loopback, unauthenticated by design. All units are fixed aviation
units (nautical miles, feet, knots, degrees, fpm); timestamps are epoch seconds.

| Method & path        | Returns                                                            |
| -------------------- | ----------------------------------------------------------------- |
| `GET /picture`       | The whole airspace: every track nearest-first, the pacing flight, freshness |
| `GET /nearest`       | Just the single nearest flight (what the status bar reads)        |
| `GET /flight/{hex}`  | One flight's full detail, including opaque per-source telemetry — `404` once it leaves the area |
| `GET /meta`          | Static facts: Home, radius, relevance, active source, units, build version |

```sh
curl -s http://127.0.0.1:7878/nearest | jq
```

```jsonc
{
  "as_of": 1780000000.0,
  "health": "live",
  "snapshot_age_s": 1.8,
  "flight": {
    "hex": "4ca853",
    "ident": "RYR4GH",
    "type": "B738",
    "model": "BOEING 737-800",
    "distance_nm": 3.1,
    "bearing_deg": 12.0,
    "altitude_ft": 37000.0,
    "groundspeed_kt": 421.0,
    "track_deg": 197.0,
    "vertical_trend": "climb",
    "cpa": { "time_to_cpa_s": 97.0, "cpa_distance_nm": 1.8 }
    // …every field is present, with explicit null for what the source omits
  }
}
```

Each flight carries **both** an estimated `lat`/`lon` *and* a derived
`distance_nm`/`bearing_deg`, so a client never recomputes geometry. The wire
format is the single shared contract between server and clients — see the
[`flights-api`](flights-api/src/lib.rs) crate.

---

## Data sources

One adapter serves the whole **readsb family** — they speak the same JSON shape,
differing only by base URL and rate limit:

| Source             | Notes                                                  |
| ------------------ | ------------------------------------------------------ |
| **airplanes.live** | Default. Free, community-run, ~1 req/s, 250 nm cap.    |
| **adsb.lol**       | Set `source.base_url`.                                 |
| **adsb.fi**        | Set `source.base_url`.                                 |
| **local readsb / dump1090** | Point at your own receiver — no rate limit, poll fast. |

New kinds (OpenSky's bounding-box API, a paid AeroAPI) slot in behind the same
`FlightSource` trait without the poller, tracker, or UI noticing.
([ADR-0003](docs/adr/0003-pluggable-data-sources.md))

---

## Project layout

```
flights/
├── flights-api/      shared REST wire types — the contract between server & clients
├── flights-server/   the engine: source adapters, poller, tracker, geometry, HTTP
├── flights-tui/      the radar TUI client (binary: `flights`)
├── flights-waybar/   the status-bar client — one-shot /nearest reader (binary: `flights-waybar`)
├── flights-web/      the Leptos/WASM webclient (built by Trunk)
├── scripts/          flights-radar / flights-web — the server+client launchers
├── docs/adr/         architecture decision records
└── CONTEXT.md        the domain language (read this first)
```

---

## Building & developing

```sh
nix develop          # drops you into a shell with the pinned toolchain + tools
cargo build          # debug build of the whole workspace
cargo test           # the test suite (engine, wire format, HTTP, TUI + waybar rendering)
cargo run -p flights-server -- --once   # one live fetch, print nearest/pacing, exit
```

Release builds are size-optimized (`lto`, `strip`, `panic = "abort"`). The Nix
flake pins the exact `rustc`/`cargo` from [`rust-toolchain.toml`](rust-toolchain.toml)
for both the dev shell and the package build, so they never drift.

---

## License

MIT (declared in [`Cargo.toml`](Cargo.toml)) © the Flights authors.

<div align="center">
<sub>Built with 🦀 Rust · <a href="https://ratatui.rs">ratatui</a> · <a href="https://airplanes.live">airplanes.live</a></sub>
</div>
