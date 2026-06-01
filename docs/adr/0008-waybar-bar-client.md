# The waybar module is a one-shot Rust Client against an always-on Server

`flights-waybar` is the bar Client ADR-0005 was named for. It is a **host Rust
binary** (a `default-members` workspace crate, so the existing `flights` derivation
carries it next to `flights`/`flights-server` — no new derivation, unlike the wasm
`flights-web`). On each Waybar `interval` it runs once, does one `GET /nearest`,
and prints a Waybar JSON object (`{text, tooltip, class}`) — the **Nearest flight**
shown only while within a Client-side **Display range** (35 nm default), in the
project's aviation units, with the full `Alt/Spd/Trk/Vr` detail in the tooltip.

Two choices a future reader will question:

*Why a Rust binary, not the `curl | jq` one-liner ADR-0005 imagined?* ADR-0005
predates ADR-0007, which made lockstep-via-`flights-api` load-bearing: a `jq` line
hand-mirrors the wire schema (`distance_nm`, the `type` rename, the `state` enum
strings) with nothing checking it against the contract — exactly the drift ADR-0007
rejected the JS webclient to avoid. So we deliberately deviate from ADR-0005's
one-liner and reuse the TUI's `ureq` + `flights-api` Client pattern instead.

*Why must the Server be a separately-managed always-on service, when `flights-radar`
and `flights-web` start-or-reuse one themselves?* Those launchers run once and tear
their Server down on exit. A one-shot module firing at ~1 Hz cannot do that: if each
tick spawned a Server it would swarm pollers against a 1-req/s Source and blow the
single-poller Source budget the whole split exists to protect (ADR-0005). So the bar
inverts the rule — `flights-waybar` **never** starts a Server; it reads one or shows
a dim `error` stub. The Server runs as a **systemd user service** delivered by the
flake's `homeManagerModules.default`, started with `graphical-session.target` in the
same session as Waybar.

A lost Nearest flight is **retained and badged** (`class="lost"`), never hard-dropped,
so a one-poll ADS-B coverage gap does not flicker the module off — the same
flicker-avoidance ADR-0007 established for the retained-track redesign.

## Considered Options

- **`curl | jq` one-liner (ADR-0005's vision)** — zero new crate, instantly editable;
  rejected because it hand-mirrors the wire schema, the drift ADR-0007 was written to
  prevent.
- **Reuse-or-spawn the Server like `flights-radar`** — rejected: a ~1 Hz one-shot that
  spawns Servers contends pollers and violates the single-poller invariant.
- **A long-lived streaming process (`return-type: json`, one line per tick)** —
  rejected: reconnect/lifecycle complexity for no gain, since loopback re-polling is
  effectively free (ADR-0005) and Waybar already owns the cadence via `interval`.
- **NixOS system service** — viable, but the bar lives in the user's graphical
  session, so a systemd *user* service (Home Manager) pairs more naturally and needs
  no root.

## Consequences

The Server gains its first **always-on** deployment mode; its single-poller invariant
now also rests on the user not pointing an on-demand launcher at the same Source
concurrently — the same reuse discipline `flights-radar` already follows. The Client
`class` taxonomy a user styles in CSS is: default (in contact), `lost`, `stale`, and
`error` (Server unreachable / no data); an empty sky emits empty `text` and Waybar
collapses the module. The flake exports `homeManagerModules.default` (the user
service + `flights-waybar` on PATH); `programs.waybar` stays the user's, wired from a
documented `custom/flights` snippet rather than auto-managed. The API stays
unversioned (ADR-0005): `flights-waybar` updates in lockstep with `flights-api`.
