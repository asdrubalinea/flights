//! Client config — deliberately tiny next to the Server's. A Client chooses only
//! *which* Server to talk to and *how often* to redraw; it computes nothing, so
//! there is no Home, Source, or poll cadence here. fps lives Client-side (ADR-0005)
//! because re-querying the Server costs no Source call — the screen rate is purely
//! a display concern.
//!
//! Lives at `$XDG_CONFIG_HOME/flights/tui.toml` (falling back to `~/.config`),
//! alongside the Server's `config.toml`. All values default, so the TUI runs out
//! of the box against a Server on the standard loopback port.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub render: Render,
}

/// Which Server to read. The default matches the Server's default bind address.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub url: String,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:7878".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Render {
    /// Radar/list refresh rate (frames per second). Costs nothing in Source calls
    /// — each frame just re-queries the Server, which dead-reckons on read.
    pub fps: u32,
}

impl Default for Render {
    fn default() -> Self {
        Self { fps: 4 }
    }
}

/// The result of loading: the finalized config and where it came from.
pub struct Loaded {
    pub config: Config,
    pub source_path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

impl Config {
    /// One frame's duration, from the clamped fps.
    pub fn render_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.render.fps.max(1)))
    }

    /// Resolve the config path: explicit override, else
    /// `$XDG_CONFIG_HOME/flights/tui.toml`, else `$HOME/.config/flights/tui.toml`.
    pub fn path(explicit: Option<&Path>) -> PathBuf {
        if let Some(p) = explicit {
            return p.to_path_buf();
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("flights").join("tui.toml")
    }

    /// Load from the resolved path (or defaults if absent) and clamp fps.
    pub fn load(explicit: Option<&Path>) -> anyhow::Result<Loaded> {
        let path = Self::path(explicit);
        let (mut config, source_path) = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("reading client config {}: {e}", path.display()))?;
            let config: Config = toml::from_str(&text)
                .map_err(|e| anyhow::anyhow!("parsing client config {}: {e}", path.display()))?;
            (config, Some(path))
        } else {
            (Config::default(), None)
        };

        let mut warnings = Vec::new();
        if !(1..=60).contains(&config.render.fps) {
            let clamped = config.render.fps.clamp(1, 60);
            warnings.push(format!(
                "render.fps {} out of [1, 60]; clamping to {clamped}",
                config.render.fps
            ));
            config.render.fps = clamped;
        }

        Ok(Loaded {
            config,
            source_path,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_the_standard_loopback_server() {
        let cfg = Config::default();
        assert_eq!(cfg.server.url, "http://127.0.0.1:7878");
        assert_eq!(cfg.render.fps, 4);
    }

    #[test]
    fn render_interval_follows_fps() {
        let mut cfg = Config::default();
        cfg.render.fps = 4;
        assert_eq!(cfg.render_interval(), Duration::from_millis(250));
    }

    #[test]
    fn parses_partial_toml_over_defaults() {
        let cfg: Config = toml::from_str(
            r#"[server]
url = "http://10.0.0.2:9000""#,
        )
        .unwrap();
        assert_eq!(cfg.server.url, "http://10.0.0.2:9000");
        // Untouched section falls back to the default fps.
        assert_eq!(cfg.render.fps, 4);
    }
}
