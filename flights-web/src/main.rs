//! `flights-web` — the radar webclient (ADR-0007): a client-side Leptos/WASM app,
//! bundled by Trunk and served from its own origin (`http://127.0.0.1:8080`). It is
//! a thin Client (ADR-0005) — it deserializes the *same* `flights-api` structs the
//! TUI uses, polls the Server's loopback REST API cross-origin, and only *renders*
//! the Picture the Server computes; it never dead-reckons or derives geometry.
//!
//! The Server answers this cross-origin page only when it was started with
//! `server.cors_allow_origin = "http://127.0.0.1:8080"` (off by default — ADR-0007).
//!
//! Run it from the dev shell: `cd flights-web && trunk serve`, then open
//! <http://127.0.0.1:8080>. Point it at a non-default Server with `?server=` and
//! set the refresh rate with `?fps=` (see [`config`]).

mod app;
mod client;
mod config;
mod detail;
mod display;
mod panel;
mod radar;

use leptos::prelude::*;

fn main() {
    mount_to_body(|| view! { <app::App /> });
}
