//! Client config for the bar module — tinier even than the TUI's, since a one-shot
//! status line chooses only *which* Server to read and how close a flight must be to
//! be worth a line at all. There is no Home, Source, or poll cadence here: a Client
//! computes nothing (ADR-0005).
//!
//! Lives at `$XDG_CONFIG_HOME/flights/waybar.toml` (falling back to `~/.config`),
//! beside the Server's `config.toml` and the TUI's `tui.toml`. All values default,
//! so the module runs out of the box against a Server on the standard loopback port.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The **Display range** the module stays quiet below — purely Client display policy,
/// distinct from the Server's Search radius and Relevance distance (CONTEXT.md). 35 nm
/// keeps the bar empty until a flight is genuinely overhead.
pub const DEFAULT_DISPLAY_RANGE_NM: f64 = 35.0;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub display: Display,
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

/// The module's display policy: the **Display range** beyond which the Nearest flight
/// is not shown at all (the bar stays empty and Waybar collapses the module).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Display {
    /// Ground-distance cutoff (nautical miles). The Nearest flight is shown only
    /// while its `distance_nm` is within this; else the bar stays empty.
    pub range_nm: f64,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            range_nm: DEFAULT_DISPLAY_RANGE_NM,
        }
    }
}

/// The result of loading: the finalized config and any non-fatal warnings. Unlike
/// the TUI's, this carries no config-source path — the module runs ~once a second
/// and never prints a routine "loaded from …" line that would flood Waybar's log.
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
}

impl Config {
    /// Resolve the config path: explicit override, else
    /// `$XDG_CONFIG_HOME/flights/waybar.toml`, else `$HOME/.config/flights/waybar.toml`.
    pub fn path(explicit: Option<&Path>) -> PathBuf {
        if let Some(p) = explicit {
            return p.to_path_buf();
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("flights").join("waybar.toml")
    }

    /// Load from the resolved path (or defaults if absent) and sanitise the range.
    pub fn load(explicit: Option<&Path>) -> anyhow::Result<Loaded> {
        let path = Self::path(explicit);
        let mut config = if path.exists() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("reading client config {}: {e}", path.display()))?;
            toml::from_str(&text)
                .map_err(|e| anyhow::anyhow!("parsing client config {}: {e}", path.display()))?
        } else {
            Config::default()
        };

        let mut warnings = Vec::new();
        // A non-positive or non-finite range would silence the module forever (or, for
        // a negative value, compare nonsensically); fall back to the default and say so.
        if !(config.display.range_nm.is_finite() && config.display.range_nm > 0.0) {
            warnings.push(format!(
                "display.range_nm {} is not a positive distance; using the {DEFAULT_DISPLAY_RANGE_NM} nm default",
                config.display.range_nm
            ));
            config.display.range_nm = DEFAULT_DISPLAY_RANGE_NM;
        }

        Ok(Loaded { config, warnings })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_the_standard_loopback_server_and_35nm_range() {
        let cfg = Config::default();
        assert_eq!(cfg.server.url, "http://127.0.0.1:7878");
        assert_eq!(cfg.display.range_nm, 35.0);
    }

    #[test]
    fn parses_partial_toml_over_defaults() {
        let cfg: Config = toml::from_str(
            r#"[display]
range_nm = 12.0"#,
        )
        .unwrap();
        assert_eq!(cfg.display.range_nm, 12.0);
        // Untouched section falls back to the default Server URL.
        assert_eq!(cfg.server.url, "http://127.0.0.1:7878");
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>(
            r#"[display]
range_nm = 12.0
bogus = true"#
        )
        .is_err());
    }
}
