# Split into a thick server and thin clients over a local REST API

`flights` began as a single TUI binary with the polling engine embedded. To drive a
waybar module (and later a webclient) alongside the TUI, we split it into one
long-running **Server** that owns the engine — the Source, the adaptive poller
(ADR-0002), and the shared `Tracker` — and thin **Clients** that only render. The
Server **dead-reckons on every read** and exposes interpreted answers (`/nearest`,
`/picture`, `/meta`) over a small REST API; Clients never compute geometry and never
talk to a Source.

The non-obvious part is *why the Server* dead-reckons rather than the Client — the
TUI used to do it itself. The cost ADR-0002 guards (rate-limited or billed Source
polls) is entirely server-side and unaffected by the split, while the new
client↔server hop is cheap loopback I/O. So re-deriving the `Picture` per request is
effectively free, keeps the geometry in exactly one place, lets the waybar module be
a `curl | jq` one-liner, and reduces the TUI to a pure renderer.

## Considered Options

- **Thin server (serve the raw `Snapshot`, Clients dead-reckon)** — rejected: every
  Client, including a shell waybar module, would have to embed the geo/tracker
  logic, duplicating it three ways.
- **Push (SSE/WebSocket) with client-side dead reckoning** — rejected: reintroduces
  client-side geometry and forces an async server, against the no-async stance
  (ADR-0006).
- **Thick server, dead-reckon on read (chosen)** — one engine, dumb Clients; the
  only cost is a dead-reckoning pass per request, trivial on loopback.

## Consequences

The engine lives only in the Server, so there is a single poller and a single Source
budget — two processes polling a 1-req/s Source would blow it. The API is
unauthenticated and bound to loopback (`127.0.0.1`), which is what makes "no auth"
safe; exposing it off-machine would require revisiting that. Loopback only stops the
*network*, though — a permissive `Access-Control-Allow-Origin` would additionally let
any website the user visits read the responses (Home's coordinates from `/meta`
included) via a browser `fetch`. So CORS is **off by default** and opted into per
origin (`server.cors_allow_origin`) when the webclient ships. The wire contract lives
in a serde-only `flights-api` crate shared by both sides, so Clients update in
lockstep and the API is unversioned until an independently-deployed consumer (a
hosted webclient) appears.
