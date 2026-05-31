//! The Server client: blocking `ureq` GETs that deserialize the `flights-api`
//! wire types. A Client never talks to a Source and never computes geometry — it
//! only reads what the Server already crunched (ADR-0005). All calls are loopback
//! and cheap, so the TUI can re-query on every frame to keep the screen current.

use std::time::Duration;

use flights_api::{FlightDetail, Meta, PictureResponse};

/// Why a request to the Server did not yield data. `Unreachable` is the one the
/// TUI surfaces as its "server unreachable" state; the rest are unexpected and
/// shown verbatim.
#[derive(Debug)]
pub enum ClientError {
    /// Transport failure — the Server is down, restarting, or the URL is wrong.
    Unreachable(String),
    /// The Server answered with an unexpected status (not 2xx, and not the 404
    /// that `flight()` handles as "left the area").
    Status(u16),
    /// The body did not deserialize into the expected wire type.
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

/// Default per-request ceiling, used for the one-off `/meta` and `/flight` calls.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Tighter ceiling for the per-frame `/picture` poll. The event loop blocks on
/// this call every frame, so a Server that is *up but hung* (accepts the socket,
/// stalls) would otherwise freeze the whole TUI — including the quit key — for the
/// full [`HTTP_TIMEOUT`]. Bounding it near a frame keeps the UI responsive; a
/// healthy loopback reply is sub-millisecond, so this only bites a stuck Server.
const PICTURE_TIMEOUT: Duration = Duration::from_millis(1500);

pub struct Client {
    /// Base URL with no trailing slash, e.g. `http://127.0.0.1:7878`.
    base_url: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            // We inspect the status ourselves so a 404 from /flight/{hex} is a
            // value ("left the area"), not a transport error.
            .http_status_as_error(false)
            .timeout_global(Some(HTTP_TIMEOUT))
            .user_agent(concat!("flights-tui/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            agent,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /meta` — the unchanging facts, fetched once at startup.
    pub fn meta(&self) -> Result<Meta, ClientError> {
        self.get_json("/meta", None)
    }

    /// `GET /picture` — the whole airspace at one instant; polled every frame.
    /// Uses the tighter [`PICTURE_TIMEOUT`] so a hung Server can't freeze the UI.
    pub fn picture(&self) -> Result<PictureResponse, ClientError> {
        self.get_json("/picture", Some(PICTURE_TIMEOUT))
    }

    /// `GET /flight/{hex}` — full detail for the popup. `Ok(None)` on a `404`,
    /// which means the flight has left the Search area (the popup's "left the
    /// area" state).
    pub fn flight(&self, hex: &str) -> Result<Option<FlightDetail>, ClientError> {
        let url = format!("{}/flight/{hex}", self.base_url);
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
                serde_json::from_str(&body)
                    .map(Some)
                    .map_err(|e| ClientError::Decode(e.to_string()))
            }
            404 => Ok(None),
            s => Err(ClientError::Status(s)),
        }
    }

    /// A GET that deserializes the JSON body. `timeout`, when set, overrides the
    /// agent's global ceiling for this one request (see [`PICTURE_TIMEOUT`]).
    fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        timeout: Option<Duration>,
    ) -> Result<T, ClientError> {
        let url = format!("{}{path}", self.base_url);
        let mut req = self.agent.get(&url);
        if let Some(t) = timeout {
            req = req.config().timeout_global(Some(t)).build();
        }
        let mut resp = req
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
