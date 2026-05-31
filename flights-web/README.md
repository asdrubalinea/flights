# flights-web

The radar **webclient** (ADR-0007): a client-side [Leptos](https://leptos.dev)/WASM
app, bundled by [Trunk](https://trunkrs.dev), served from its own origin. It is a
thin **Client** (ADR-0005) — it deserializes the *same* `flights-api` structs the
TUI uses, polls the Server's loopback REST API, and only renders the **Picture**
the Server computes. It never dead-reckons or derives geometry; the canvas only
projects Server answers onto pixels (CONTEXT.md).

## Running

First, let the Server's loopback API be read by the webclient's origin over CORS —
it's **off by default** (ADR-0005/0007), and without it the browser blocks the
cross-origin reads and the page shows `UNREACHABLE`. In `~/.config/flights/config.toml`:

```toml
[server]
cors_allow_origin = "http://127.0.0.1:8080"
```

Then start the Server and the webclient together with one command (it starts or
reuses the Server, checks the CORS allowance, and serves the client):

```sh
nix run .#web        # bundles the wasm toolchain; build from a repo checkout
flights-web          # equivalent, inside `nix develop` / direnv
```

Or drive the two halves yourself from the dev shell (which provides the
`wasm32-unknown-unknown` target, `trunk`, and a matching `wasm-bindgen-cli`):

```sh
flights-server
cd flights-web && trunk serve   # → http://127.0.0.1:8080
```

## Configuration

There is no config file in a browser; both knobs come from the page URL:

- `?server=http://host:port` — the Server base URL (default `http://127.0.0.1:7878`).
- `?fps=N` — radar refresh rate, clamped to `1..=60` (default `4`).

e.g. <http://127.0.0.1:8080/?server=http://127.0.0.1:7878&fps=10>

## Why it isn't built by `cargo build`

`flights-web` targets `wasm32-unknown-unknown` and is kept out of the workspace's
`default-members`, so host-side `cargo build` / `test` / `clippy` (and the Nix
`buildRustPackage`) never try to compile it for the host. Build it with Trunk, or
`cargo check -p flights-web --target wasm32-unknown-unknown`.
