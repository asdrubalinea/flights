//! Webclient config — even tinier than the TUI's. A Client chooses only *which*
//! Server to read and *how often* to redraw; it computes nothing (ADR-0005). There
//! is no config file in a browser, so both come from the page URL's query string,
//! with defaults that match a Server on the standard loopback port:
//!
//! - `?server=http://host:port` — the Server base URL (default `http://127.0.0.1:7878`).
//! - `?fps=N` — radar refresh rate, clamped to `1..=60` (default `4`).
//!
//! Re-querying the Server costs no Source call, so fps is purely a display concern
//! (ADR-0005). Note the Server only answers this cross-origin page if it was
//! started with `server.cors_allow_origin = "http://127.0.0.1:8080"` (ADR-0007).

/// The Server base URL used when `?server=` is absent — the Server's own default
/// bind address.
const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:7878";
const DEFAULT_FPS: u32 = 4;

#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Server base URL, no trailing slash.
    pub server_url: String,
    /// Radar refresh rate (frames per second), already clamped to `1..=60`.
    pub fps: u32,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            fps: DEFAULT_FPS,
        }
    }
}

impl WebConfig {
    /// One frame's duration in milliseconds, from the clamped fps.
    pub fn frame_ms(&self) -> u32 {
        1000 / self.fps.max(1)
    }

    /// Read config from the current page's query string, falling back to defaults
    /// for anything missing or malformed.
    pub fn from_location() -> Self {
        let search = leptos::prelude::window().location().search().unwrap_or_default();
        Self::from_query(&search)
    }

    /// Parse a raw `?a=b&c=d` query string. Split out from [`from_location`] so it
    /// is testable without a browser. Values are taken verbatim (no percent
    /// decoding) — a loopback URL needs none.
    pub fn from_query(search: &str) -> Self {
        let mut cfg = WebConfig::default();
        for pair in search.trim_start_matches('?').split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            match key {
                "server" if !value.is_empty() => {
                    cfg.server_url = value.trim_end_matches('/').to_string();
                }
                "fps" => {
                    if let Ok(n) = value.parse::<u32>() {
                        cfg.fps = n.clamp(1, 60);
                    }
                }
                _ => {}
            }
        }
        cfg
    }
}
