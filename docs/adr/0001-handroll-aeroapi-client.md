# Hand-roll a thin AeroAPI client instead of generating from the OpenAPI spec

_Update: this decision now generalizes to every data source (see
[ADR-0003](0003-pluggable-data-sources.md)). The first Source — the keyless
**airplanes.live** ADS-B API — has no OpenAPI spec at all, so a hand-rolled `ureq`
client is the only sensible option regardless. AeroAPI is now just one possible
future adapter; the reasoning below is what we would apply to it or any other
Source._

FlightAware publishes an AeroAPI OpenAPI spec, so generating a client is the
obvious path — which is why this needs explaining. We instead hand-roll a thin
synchronous `ureq` call to `/flights/search` with `serde` structs covering only
the fields we render, because `flights` is a size-optimized, single-endpoint
app: full codegen would compile all ~60 endpoints into the binary
(against our `strip`/`lto`/`panic=abort` profile), and the tooling fights our
setup — `openapi-generator` needs a JVM in the Nix build path, and the
Rust-native `progenitor` is async-only, making Tokio dead weight in a
short-lived CLI.

## Considered Options

- **openapi-generator** — full typed client, can emit blocking reqwest, but
  requires a JVM build dependency and emits a large surface needing cleanup.
- **progenitor** — Rust-native, no JVM, but async-only (Tokio) and brittle on
  spec quirks.
- **Hand-rolled (chosen)** — smallest binary, no build-time codegen, no unused
  surface; the cost is that we maintain the request/response types by hand.

For the HTTP call itself we use `ureq` rather than `reqwest::blocking`: both are
blocking, but `reqwest::blocking` spins up a hidden internal Tokio runtime and a
larger dependency tree, whereas `ureq` is genuinely synchronous and rustls-based
— a better fit for a size-optimized binary making one GET per poll.

## Consequences

Revisit this if `flights` grows to consume many AeroAPI endpoints — at that
point the maintenance cost of hand-written types flips the trade-off back toward
codegen.
