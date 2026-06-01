//! The Server client for the bar module: a single blocking `ureq` GET against
//! `/nearest`, deserialized into the shared `flights-api` wire type. Like every
//! Client it only *reads* what the Server already crunched and never touches a
//! Source (ADR-0005); the same `ureq` + `flights-api` pattern the TUI uses, so the
//! wire schema is checked against the contract rather than hand-mirrored in `jq`
//! (the drift ADR-0008 deviates from ADR-0005's one-liner to avoid).
//!
//! Unlike the TUI this Client fires exactly once per Waybar tick and **never starts
//! a Server** (ADR-0008): an unreachable Server is a value the caller renders as the
//! dim `error` stub, not a crash and never a spawned poller.

use std::time::Duration;

use flights_api::NearestResponse;

/// Why a `/nearest` request did not yield data. All three collapse to the same dim
/// `error` stub in the bar; the message rides the module's tooltip.
#[derive(Debug)]
pub enum ClientError {
    /// Transport failure — the Server is down, restarting, or the URL is wrong.
    Unreachable(String),
    /// The Server answered with an unexpected (non-2xx) status.
    Status(u16),
    /// The body did not deserialize into [`NearestResponse`].
    Decode(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unreachable(e) => write!(f, "server unreachable: {e}"),
            ClientError::Status(s) => write!(f, "unexpected HTTP status {s}"),
            ClientError::Decode(e) => write!(f, "decode error: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// One Waybar tick fires this once, so the ceiling sits well under a typical
/// `interval` (seconds): a Server that is *up but hung* (accepts the socket, stalls)
/// must surface as the `error` stub rather than wedge the whole bar for that tick.
/// A healthy loopback reply is sub-millisecond, so this only bites a stuck Server.
const HTTP_TIMEOUT: Duration = Duration::from_millis(1500);

pub struct Client {
    base_url: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            // We inspect the status ourselves, so a non-2xx is a value, not a panic.
            .http_status_as_error(false)
            .timeout_global(Some(HTTP_TIMEOUT))
            .user_agent(concat!("flights-waybar/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            agent,
        }
    }

    /// `GET /nearest` — the single Nearest flight (or `flight: null`) plus the
    /// Server's freshness. The module's only request per tick (ADR-0008).
    pub fn nearest(&self) -> Result<NearestResponse, ClientError> {
        let url = format!("{}/nearest", self.base_url);
        let mut resp = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        match resp.status().as_u16() {
            200..=299 => {
                let body = resp
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| ClientError::Decode(e.to_string()))?;
                serde_json::from_str(&body).map_err(|e| ClientError::Decode(e.to_string()))
            }
            s => Err(ClientError::Status(s)),
        }
    }
}
