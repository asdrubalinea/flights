//! Configuration: where Home is, how big the Search area is, which Source to
//! poll, and where to bind the REST API. Everything but secrets lives in a TOML
//! file under `$XDG_CONFIG_HOME/flights/config.toml` (falling back to `~/.config`).
//! API keys for future paid Sources come from the environment only and are never
//! written here (the readsb-family Sources are keyless).
//!
//! This is the **Server** config. Render rate is no longer here — fps is a display
//! concern that moved to the Client with the split (ADR-0005), since the screen is
//! kept current by re-querying the Server, which costs no Source call.
//!
//! All values have defaults, so the Server runs out of the box; the only thing
//! worth setting is `[home]`. Values are validated and gently clamped on load,
//! with the reasons surfaced as warnings rather than hard failures where it is safe.

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::domain::{LatLon, SearchArea};

/// A fast jet's groundspeed ceiling (knots), used to bound the **max** poll
/// interval: polling slower than the time it takes such a jet to cross the
/// Search area would let it traverse the area unseen.
pub const MAX_GROUNDSPEED_KT: f64 = 650.0;

/// The airplanes.live family caps the Search radius at 250 nm.
const READSB_MAX_RADIUS_NM: f64 = 250.0;

/// Hard ceiling on the Search radius: half the Earth's circumference (~10 800 nm)
/// already covers the whole globe, so anything larger is meaningless — and the
/// [`Config::transit_time`] arithmetic would overflow a `Duration` and panic on
/// it. Reject such a radius up front rather than panic mid-`finalize`.
const RADIUS_CEILING_NM: f64 = 180.0 * 60.0;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading config {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid config: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub home: Home,
    pub search: Search,
    pub poll: Poll,
    pub source: Source,
    pub server: Server,
}

/// **Home** — the single fixed point all distances are measured from. Defaults to
/// near London Heathrow purely so a first run shows *something*; set your own.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Home {
    pub lat: f64,
    pub lon: f64,
}

impl Default for Home {
    fn default() -> Self {
        Self {
            lat: 28.1529,
            lon: -15.4316,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Search {
    /// Radius of the Search area, nautical miles.
    pub radius_nm: f64,
    /// Relevance distance: the CPA-distance cutoff for pacing. Bounded by
    /// `radius_nm` (we cannot pace on what we cannot see).
    pub relevance_distance_nm: f64,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            radius_nm: 100.0,
            relevance_distance_nm: 30.0,
        }
    }
}

/// Where the REST API listens. Loopback by default — the unauthenticated API is
/// safe precisely *because* it is bound to `127.0.0.1` (ADR-0005); exposing it
/// off-machine would require revisiting that.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    /// `host:port` to bind the REST API to. Prefer a literal IP (the default is
    /// `127.0.0.1:7878`): a hostname resolving to several addresses binds only the
    /// first one resolved, so a dual-stack `localhost` could end up listening on
    /// just one of v4/v6 while a Client reaches for the other.
    pub bind: String,
    /// The value sent in `Access-Control-Allow-Origin`, or `None` to send no CORS
    /// header at all (the default). Loopback only stops the *network* from reaching
    /// the API; a permissive CORS header additionally lets any website the user
    /// visits read the responses — including Home's coordinates from `/meta` — via
    /// a browser `fetch`. So CORS is **off by default** and the future webclient
    /// opts in to a specific origin (e.g. `"https://flights.example"`) or `"*"`.
    pub cors_allow_origin: Option<String>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:7878".to_string(),
            cors_allow_origin: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Poll {
    /// The slowest we ever poll — used when the airspace is quiet. Must stay
    /// below the Search-area transit time; the **min** comes from the Source.
    pub max_interval_secs: f64,
}

impl Default for Poll {
    fn default() -> Self {
        Self {
            max_interval_secs: 60.0,
        }
    }
}

/// The active **Source** and its per-source settings. `kind` selects the adapter;
/// the readsb family (airplanes.live, adsb.lol, adsb.fi, a local receiver) is one
/// adapter differing only by `base_url`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Source {
    pub kind: String,
    /// Override the adapter's base URL — set this to point the readsb adapter at
    /// adsb.lol/adsb.fi or a local dump1090/readsb box.
    pub base_url: Option<String>,
    /// Override the Source's declared minimum poll interval (ms). A local
    /// receiver has no rate limit and can be polled far faster.
    pub min_interval_ms: Option<u64>,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            kind: "airplanes_live".to_string(),
            base_url: None,
            min_interval_ms: None,
        }
    }
}

/// The result of loading: the finalized config, where it came from, and any
/// non-fatal warnings raised while normalizing it.
pub struct Loaded {
    pub config: Config,
    /// `Some(path)` if a file was read; `None` if built-in defaults were used.
    pub source_path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

impl Config {
    pub fn home(&self) -> LatLon {
        LatLon::new(self.home.lat, self.home.lon)
    }

    pub fn search_area(&self) -> SearchArea {
        SearchArea {
            center: self.home(),
            radius_nm: self.search.radius_nm,
        }
    }

    /// The configured bind address, resolved to a concrete socket. Validated on
    /// load (see [`Config::finalize`]), so this won't error there.
    pub fn bind_addr(&self) -> Result<SocketAddr, ConfigError> {
        resolve_bind(&self.server.bind)
    }

    pub fn max_poll(&self) -> Duration {
        Duration::from_secs_f64(self.poll.max_interval_secs)
    }

    /// How long a `MAX_GROUNDSPEED_KT` jet takes to cross the Search area
    /// diameter — the hard ceiling the max poll interval must stay under.
    pub fn transit_time(&self) -> Duration {
        Duration::from_secs_f64(2.0 * self.search.radius_nm / MAX_GROUNDSPEED_KT * 3600.0)
    }

    /// Resolve the config file path: explicit override, else
    /// `$XDG_CONFIG_HOME/flights/config.toml`, else `$HOME/.config/flights/config.toml`.
    pub fn path(explicit: Option<&Path>) -> PathBuf {
        if let Some(p) = explicit {
            return p.to_path_buf();
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("flights").join("config.toml")
    }

    /// Load and finalize config from the resolved path (or defaults if absent).
    pub fn load(explicit: Option<&Path>) -> Result<Loaded, ConfigError> {
        let path = Self::path(explicit);
        let (config, source_path) = if path.exists() {
            let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
                path: path.clone(),
                source,
            })?;
            let config: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
                path: path.clone(),
                source,
            })?;
            (config, Some(path))
        } else {
            (Config::default(), None)
        };

        let (config, warnings) = config.finalize()?;
        Ok(Loaded {
            config,
            source_path,
            warnings,
        })
    }

    /// Validate and clamp into a usable state, returning non-fatal warnings.
    /// Hard-invalid values (a zero/negative radius, a nonsense Home) error out.
    fn finalize(mut self) -> Result<(Self, Vec<String>), ConfigError> {
        let mut warnings = Vec::new();

        let lat_ok = self.home.lat.is_finite() && (-90.0..=90.0).contains(&self.home.lat);
        let lon_ok = self.home.lon.is_finite() && (-180.0..=180.0).contains(&self.home.lon);
        if !(lat_ok && lon_ok) {
            return Err(ConfigError::Invalid(format!(
                "home lat/lon out of range: ({}, {})",
                self.home.lat, self.home.lon
            )));
        }

        if !(self.search.radius_nm.is_finite() && self.search.radius_nm > 0.0) {
            return Err(ConfigError::Invalid(format!(
                "search.radius_nm must be positive, got {}",
                self.search.radius_nm
            )));
        }
        // A radius this large is physically meaningless and would overflow the
        // transit-time arithmetic below; reject it rather than panic.
        if self.search.radius_nm > RADIUS_CEILING_NM {
            return Err(ConfigError::Invalid(format!(
                "search.radius_nm {} exceeds the maximum meaningful radius of {RADIUS_CEILING_NM} nm \
                 (half the Earth's circumference)",
                self.search.radius_nm
            )));
        }
        if self.search.radius_nm > READSB_MAX_RADIUS_NM {
            warnings.push(format!(
                "search.radius_nm {} exceeds the airplanes.live cap of {READSB_MAX_RADIUS_NM} nm; \
                 the adapter will clamp it",
                self.search.radius_nm
            ));
        }

        if !(self.search.relevance_distance_nm.is_finite()
            && self.search.relevance_distance_nm > 0.0)
        {
            return Err(ConfigError::Invalid(format!(
                "search.relevance_distance_nm must be positive, got {}",
                self.search.relevance_distance_nm
            )));
        }
        // Relevance distance is bounded by the Search area — we cannot pace on a
        // flight we cannot see.
        if self.search.relevance_distance_nm > self.search.radius_nm {
            warnings.push(format!(
                "search.relevance_distance_nm {} exceeds the Search radius {}; clamping to the radius",
                self.search.relevance_distance_nm, self.search.radius_nm
            ));
            self.search.relevance_distance_nm = self.search.radius_nm;
        }

        // The bind address must resolve now, so a typo fails on load with a clear
        // message rather than deep inside server startup.
        let addr = resolve_bind(&self.server.bind)?;
        // The no-auth model leans on a loopback bind (ADR-0005). A non-loopback
        // bind exposes the unauthenticated API — including Home's coordinates — to
        // the network; allow it (an explicit choice) but never let it pass silently.
        if !addr.ip().is_loopback() {
            warnings.push(format!(
                "server.bind {} is not loopback; the unauthenticated API (and Home's \
                 coordinates via /meta) will be reachable from the network",
                self.server.bind
            ));
        }

        // A configured CORS origin is sent verbatim as the `Access-Control-Allow-Origin`
        // header value, which must be ASCII with no control characters. Reject a bad
        // one here, on load, rather than letting the per-request header build fail and
        // panic (ADR-0005): under `panic = "abort"` that would take the whole Server
        // down on every CORS response. An origin is a URL or `*`, so requiring visible
        // ASCII (no spaces) is strict on purpose, and it also forecloses header injection.
        if let Some(origin) = &self.server.cors_allow_origin {
            if origin.is_empty() || origin.bytes().any(|b| !(0x21..=0x7e).contains(&b)) {
                return Err(ConfigError::Invalid(format!(
                    "server.cors_allow_origin {origin:?} is not a valid header value \
                     (expected an origin URL or \"*\", visible ASCII with no spaces)"
                )));
            }
        }

        // The max poll interval must stay below the Search-area transit time, so a
        // fast jet cannot cross the area between polls unseen.
        let transit = self.transit_time().as_secs_f64();
        if !(self.poll.max_interval_secs.is_finite() && self.poll.max_interval_secs > 0.0) {
            return Err(ConfigError::Invalid(format!(
                "poll.max_interval_secs must be positive, got {}",
                self.poll.max_interval_secs
            )));
        }
        let ceiling = 0.8 * transit;
        if self.poll.max_interval_secs >= ceiling {
            warnings.push(format!(
                "poll.max_interval_secs {:.0}s is not safely below the Search-area transit time \
                 ({:.0}s for a {:.0} kt jet); clamping to {:.0}s",
                self.poll.max_interval_secs, transit, MAX_GROUNDSPEED_KT, ceiling
            ));
            self.poll.max_interval_secs = ceiling;
        }

        Ok((self, warnings))
    }

    /// A human-readable, secret-free summary for `--print-config` and startup logs.
    pub fn summary(&self) -> String {
        format!(
            "Home          {:.4}, {:.4}\n\
             Search area   {:.0} nm radius (relevance {:.0} nm)\n\
             Poll          max {:.0}s (transit ceiling {:.0}s)\n\
             Bind          {} (CORS {})\n\
             Source        {}{}",
            self.home.lat,
            self.home.lon,
            self.search.radius_nm,
            self.search.relevance_distance_nm,
            self.poll.max_interval_secs,
            self.transit_time().as_secs_f64(),
            self.server.bind,
            match &self.server.cors_allow_origin {
                Some(origin) => origin.as_str(),
                None => "off",
            },
            self.source.kind,
            match &self.source.base_url {
                Some(url) => format!(" ({url})"),
                None => String::new(),
            },
        )
    }
}

/// Resolve a `host:port` bind string to a concrete [`SocketAddr`], erroring (not
/// panicking) on a malformed or unresolvable address so config load can report it.
/// Takes the first resolved address; see the [`Server`] `bind` note on hostnames.
fn resolve_bind(bind: &str) -> Result<SocketAddr, ConfigError> {
    bind.to_socket_addrs()
        .map_err(|e| {
            ConfigError::Invalid(format!("server.bind {bind:?} is not a valid address: {e}"))
        })?
        .next()
        .ok_or_else(|| ConfigError::Invalid(format!("server.bind {bind:?} resolved to no address")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_warning_free() {
        let (cfg, warnings) = Config::default().finalize().unwrap();
        assert!(warnings.is_empty(), "default config warned: {warnings:?}");
        assert_eq!(cfg.search.radius_nm, 100.0);
        assert_eq!(cfg.source.kind, "airplanes_live");
    }

    #[test]
    fn parses_a_partial_toml_over_defaults() {
        let toml = r#"
            [home]
            lat = 40.6413
            lon = -73.7781

            [search]
            radius_nm = 50.0
            relevance_distance_nm = 20.0
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let (cfg, warnings) = cfg.finalize().unwrap();
        assert!(warnings.is_empty());
        assert_eq!(cfg.home.lat, 40.6413);
        assert_eq!(cfg.search.radius_nm, 50.0);
        // Untouched sections fall back to defaults.
        assert_eq!(cfg.server.bind, "127.0.0.1:7878");
        assert_eq!(cfg.source.kind, "airplanes_live");
    }

    #[test]
    fn relevance_is_clamped_to_the_search_radius() {
        let cfg = Config {
            search: Search {
                radius_nm: 40.0,
                relevance_distance_nm: 100.0,
            },
            ..Config::default()
        };
        let (cfg, warnings) = cfg.finalize().unwrap();
        assert_eq!(cfg.search.relevance_distance_nm, 40.0);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn max_poll_is_clamped_below_transit_time() {
        // 250 nm radius → transit ≈ 2769 s; a 1-hour max poll is far too slow.
        let cfg = Config {
            search: Search {
                radius_nm: 250.0,
                ..Search::default()
            },
            poll: Poll {
                max_interval_secs: 3600.0,
            },
            ..Config::default()
        };
        let (cfg, warnings) = cfg.finalize().unwrap();
        assert!(cfg.poll.max_interval_secs < cfg.transit_time().as_secs_f64());
        assert!(warnings.iter().any(|w| w.contains("transit")));
    }

    #[test]
    fn rejects_out_of_range_home() {
        let cfg = Config {
            home: Home {
                lat: 99.0,
                lon: 0.0,
            },
            ..Config::default()
        };
        assert!(matches!(cfg.finalize(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_absurd_radius_instead_of_panicking() {
        // A finite, positive, but globe-dwarfing radius once overflowed
        // `transit_time()` and panicked inside `finalize`; it must error cleanly.
        let cfg = Config {
            search: Search {
                radius_nm: 1e308,
                ..Search::default()
            },
            ..Config::default()
        };
        assert!(matches!(cfg.finalize(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_unknown_keys() {
        let toml = r#"
            [search]
            radius_nm = 50.0
            bogus = true
        "#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }

    #[test]
    fn default_bind_resolves_to_loopback() {
        let cfg = Config::default();
        let addr = cfg.bind_addr().unwrap();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 7878);
    }

    #[test]
    fn rejects_a_malformed_bind_address() {
        let cfg = Config {
            server: Server {
                bind: "not a socket".into(),
                ..Server::default()
            },
            ..Config::default()
        };
        assert!(matches!(cfg.finalize(), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn cors_is_off_by_default() {
        assert_eq!(Config::default().server.cors_allow_origin, None);
    }

    #[test]
    fn rejects_a_cors_origin_that_is_not_a_valid_header_value() {
        // A non-ASCII origin would make the per-request header build fail; caught
        // on load it is a clear ConfigError instead of a panic under panic=abort.
        for bad in ["http://exämple.com", "has space", "with\nnewline", ""] {
            let cfg = Config {
                server: Server {
                    cors_allow_origin: Some(bad.into()),
                    ..Server::default()
                },
                ..Config::default()
            };
            assert!(
                matches!(cfg.finalize(), Err(ConfigError::Invalid(_))),
                "cors_allow_origin {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_a_valid_cors_origin() {
        for ok in ["*", "https://flights.example", "http://127.0.0.1:5173"] {
            let cfg = Config {
                server: Server {
                    cors_allow_origin: Some(ok.into()),
                    ..Server::default()
                },
                ..Config::default()
            };
            assert!(
                cfg.finalize().is_ok(),
                "cors_allow_origin {ok:?} should be accepted"
            );
        }
    }

    #[test]
    fn warns_but_accepts_a_non_loopback_bind() {
        let cfg = Config {
            server: Server {
                bind: "0.0.0.0:7878".into(),
                ..Server::default()
            },
            ..Config::default()
        };
        let (_cfg, warnings) = cfg.finalize().unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("not loopback")),
            "a non-loopback bind should warn: {warnings:?}"
        );
    }
}
