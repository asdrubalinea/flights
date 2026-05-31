//! The Source seam (ADR-0003): every flight data provider sits behind one
//! trait that takes a [`SearchArea`] and returns a domain [`Snapshot`]. The
//! poller, tracker, and UI never know which Source is active.
//!
//! Capability differences are absorbed *inside* adapters: a box-only Source
//! (OpenSky) would convert the radius to a bounding box and filter back to the
//! radius here; per-source auth (OAuth, paid keys) stays here too. The first and
//! only adapter today is [`readsb`], which serves the airplanes.live /
//! adsb.lol / adsb.fi / local-receiver family — one adapter differing only by
//! base URL.

mod readsb;

use std::time::Duration;

use crate::config::Config;
use crate::domain::{SearchArea, Snapshot};

/// Why a poll did not yield a Snapshot. Deliberately small and provider-neutral:
/// the poller decides how to pace and back off from these alone (see ADR-0002),
/// never from a provider's raw status code.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The Source asked us to slow down. `retry_after` is honored when present.
    #[error("rate limited{}", match .retry_after {
        Some(d) => format!(" (retry after {:.0}s)", d.as_secs_f64()),
        None => String::new(),
    })]
    RateLimited { retry_after: Option<Duration> },
    /// The Source is reachable but not serving (5xx, maintenance).
    #[error("source unavailable")]
    Unavailable,
    /// Authentication failed or is required (paid/OAuth Sources).
    #[error("authentication failed")]
    Auth,
    /// The response arrived but could not be decoded into the domain shape.
    #[error("decode error: {0}")]
    Decode(String),
    /// A transient network/transport hiccup; retrying may succeed.
    #[error("transient network error")]
    Transient,
}

/// A pluggable provider of flight Snapshots.
pub trait FlightSource: Send {
    /// Human-readable name for the status line (e.g. `"airplanes.live"`).
    fn name(&self) -> &str;

    /// The Source's own floor on poll cadence — the lower bound the adaptive
    /// poller respects. Near-zero for a local receiver, ≈1 s for airplanes.live,
    /// a cost ceiling for a paid API.
    fn min_interval(&self) -> Duration;

    /// Fetch the current Snapshot of airborne flights within the Search area.
    fn fetch(&self, area: &SearchArea) -> Result<Snapshot, SourceError>;
}

/// Build the active Source from config. The readsb-family kinds share one
/// adapter; only the base URL and rate-limit floor differ. (Future kinds —
/// `opensky`, `aeroapi` — would branch to their own adapters here, reading any
/// secret from the environment rather than config.)
pub fn build(cfg: &Config) -> anyhow::Result<Box<dyn FlightSource>> {
    let (name, default_base, default_min): (&str, &str, Duration) = match cfg.source.kind.as_str() {
        "airplanes_live" => (
            "airplanes.live",
            "https://api.airplanes.live/v2",
            Duration::from_secs(1),
        ),
        "adsb_lol" => (
            "adsb.lol",
            "https://api.adsb.lol/v2",
            Duration::from_secs(1),
        ),
        "adsb_fi" => (
            "adsb.fi",
            "https://opendata.adsb.fi/api/v2",
            Duration::from_secs(1),
        ),
        // A local dump1090/readsb box has no rate limit; poll it fast.
        "readsb" | "local" => ("local receiver", "", Duration::from_millis(250)),
        other => anyhow::bail!(
            "unknown source.kind {other:?} (expected one of: \
                 airplanes_live, adsb_lol, adsb_fi, readsb)"
        ),
    };

    let base_url = cfg
        .source
        .base_url
        .clone()
        .unwrap_or_else(|| default_base.to_string());
    anyhow::ensure!(
        !base_url.is_empty(),
        "source.kind {:?} requires source.base_url to be set (e.g. http://localhost/re-api)",
        cfg.source.kind
    );

    let min_interval = cfg
        .source
        .min_interval_ms
        .map(Duration::from_millis)
        .unwrap_or(default_min);

    Ok(Box::new(readsb::ReadsbSource::new(
        name,
        base_url,
        min_interval,
    )))
}
