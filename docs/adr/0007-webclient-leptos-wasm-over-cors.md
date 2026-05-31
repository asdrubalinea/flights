# The webclient is a separately-served Leptos/WASM Client reaching the API via CORS

The webclient (ADR-0005 anticipated it) is a client-side **Leptos** single-page app
compiled to `wasm32` and bundled by **Trunk**, served as static assets from its own
origin (`http://127.0.0.1:8080`) — *not* from `flights-server`. It reads the loopback
REST API cross-origin, so the Server opts into a **specific-origin** CORS allowance
(`server.cors_allow_origin = "http://127.0.0.1:8080"`, never `*`). It stays a pure
Client (ADR-0005): it deserializes the *same* `flights-api` structs the TUI uses,
polls `/picture` at its own frame rate, and never dead-reckons or computes geometry —
the canvas only projects Server answers onto pixels.

Two non-obvious choices a future reader will question:

*Why WASM, not JavaScript?* A JS client would hand-mirror the wire schema and could
drift; compiling a Rust client against `flights-api` keeps both sides in lockstep by
construction — the contract crate's whole purpose (ADR-0005).

*Why a separate origin + CORS, not same-origin static-serving from `flights-server`?*
Same-origin would avoid CORS entirely and is arguably simpler, but it broadens the
Server beyond the pure read-only REST API it is today and couples asset delivery to
the engine process. We keep the Server a pure API and accept a *narrow, single-origin*
CORS allowance instead — far less exposure than the `*` ADR-0005 warned against.

## Considered Options

- **Same-origin (flights-server serves the assets)** — no CORS at all, one process;
  rejected to keep the Server a pure REST API and out of the static-file business.
- **JS/TS SPA (vanilla or a framework)** — best visual DX, but hand-copies the wire
  types (drift) and drags a node/npm toolchain into a pure-Rust, Nix-managed repo.
- **Leptos/WASM served by Trunk, CORS to a specific origin (chosen)** — lockstep
  types, no node, pure-API server; costs a wasm toolchain (Trunk + the `wasm32`
  target in the Nix shell) and a narrow CORS surface.
- **Client-side tweening for 60 fps motion** — rejected: the Client would draw
  positions the Server never asserted, against the geometry-stays-server-side rule
  (ADR-0005, CONTEXT.md). Smoothness is the poll rate instead.

## Consequences

The Server still does **no async** (ADR-0006 holds): the webclient polls like the
TUI; nothing here needs SSE/WebSocket. The dev shell gains the
`wasm32-unknown-unknown` target and `trunk`. `flights-web` is a workspace member
excluded from `default-members`, so it shares one `Cargo.lock` and the `flights-api`
path dependency but does not enter the host-side `cargo test`/clippy build. The API
stays unversioned (ADR-0005) and the webclient updates in lockstep with the crate —
true until a *hosted* webclient (a non-loopback origin) appears, which is also when
the CORS origin stops being `127.0.0.1:8080`.
