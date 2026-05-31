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

#[cfg(test)]
mod tests {
    use super::*;

    fn with_fps(fps: u32) -> WebConfig {
        WebConfig {
            fps,
            ..WebConfig::default()
        }
    }

    #[test]
    fn empty_query_is_all_defaults() {
        let cfg = WebConfig::from_query("");
        assert_eq!(cfg.server_url, DEFAULT_SERVER_URL);
        assert_eq!(cfg.fps, DEFAULT_FPS);
    }

    #[test]
    fn reads_server_and_fps() {
        let cfg = WebConfig::from_query("?server=http://10.0.0.2:9000&fps=30");
        assert_eq!(cfg.server_url, "http://10.0.0.2:9000");
        assert_eq!(cfg.fps, 30);
    }

    #[test]
    fn leading_question_mark_is_optional() {
        assert_eq!(WebConfig::from_query("fps=15").fps, 15);
    }

    #[test]
    fn trailing_slashes_are_trimmed_from_server() {
        assert_eq!(
            WebConfig::from_query("?server=http://host:7878/").server_url,
            "http://host:7878"
        );
    }

    #[test]
    fn fps_is_clamped_to_1_through_60() {
        assert_eq!(WebConfig::from_query("?fps=0").fps, 1);
        assert_eq!(WebConfig::from_query("?fps=1").fps, 1);
        assert_eq!(WebConfig::from_query("?fps=60").fps, 60);
        assert_eq!(WebConfig::from_query("?fps=1000").fps, 60);
    }

    #[test]
    fn malformed_or_empty_values_fall_back_to_defaults() {
        // Non-numeric fps and an empty server= are both ignored.
        let cfg = WebConfig::from_query("?fps=fast&server=");
        assert_eq!(cfg.fps, DEFAULT_FPS);
        assert_eq!(cfg.server_url, DEFAULT_SERVER_URL);
    }

    #[test]
    fn unknown_keys_and_valueless_pairs_are_skipped() {
        let cfg = WebConfig::from_query("?foo=bar&flag&fps=10");
        assert_eq!(cfg.fps, 10);
        assert_eq!(cfg.server_url, DEFAULT_SERVER_URL);
    }

    #[test]
    fn frame_ms_is_integer_division_of_one_second() {
        assert_eq!(with_fps(4).frame_ms(), 250);
        assert_eq!(with_fps(60).frame_ms(), 16); // 1000 / 60 truncates
        assert_eq!(with_fps(1).frame_ms(), 1000);
    }
}
