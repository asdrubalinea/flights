//! The Server client: async `fetch` GETs (via `gloo-net`) that deserialize the
//! *same* `flights-api` wire types the TUI uses — the whole reason the webclient
//! is Rust/WASM rather than hand-mirrored JS (ADR-0007). A Client never talks to a
//! Source and never computes geometry; it only reads what the Server crunched
//! (ADR-0005). These reads are cross-origin (ADR-0007), so they succeed only when
//! the Server opted this origin into CORS.

use flights_api::{FlightDetail, Meta, PictureResponse};
use gloo_net::http::Request;
use serde::de::DeserializeOwned;

/// Why a request to the Server did not yield data. `Unreachable` is the state the
/// UI surfaces as "server unreachable" — which over CORS also covers a Server that
/// is up but never allowed this origin (the browser blocks the read, and `fetch`
/// reports it as a network error indistinguishable from down).
#[derive(Debug, Clone)]
pub enum ApiError {
    /// Transport/CORS failure — the Server is down, the URL is wrong, or this
    /// origin is not in `server.cors_allow_origin`.
    Unreachable(String),
    /// The Server answered with an unexpected status (not 2xx, and not the 404
    /// that `flight()` handles as "left the area").
    Status(u16),
    /// The body did not deserialize into the expected wire type.
    Decode(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unreachable(e) => write!(f, "server unreachable: {e}"),
            ApiError::Status(s) => write!(f, "unexpected HTTP status {s}"),
            ApiError::Decode(e) => write!(f, "decode error: {e}"),
        }
    }
}

/// A handle to one Server. Cheap to clone (just a base URL), so each poll tick can
/// take its own copy into an async task.
#[derive(Debug, Clone)]
pub struct ApiClient {
    /// Base URL with no trailing slash, e.g. `http://127.0.0.1:7878`.
    base_url: String,
}

impl ApiClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// `GET /meta` — the unchanging facts, fetched once at startup.
    pub async fn meta(&self) -> Result<Meta, ApiError> {
        self.get_json("/meta").await
    }

    /// `GET /picture` — the whole airspace at one instant; polled every frame.
    pub async fn picture(&self) -> Result<PictureResponse, ApiError> {
        self.get_json("/picture").await
    }

    /// `GET /flight/{hex}` — full detail for the popup. `Ok(None)` on a `404`, which
    /// means the flight has left the Search area (the popup's "left the area" state).
    pub async fn flight(&self, hex: &str) -> Result<Option<FlightDetail>, ApiError> {
        let url = format!("{}/flight/{hex}", self.base_url);
        let resp = Request::get(&url)
            .send()
            .await
            .map_err(|e| ApiError::Unreachable(e.to_string()))?;
        match resp.status() {
            200..=299 => resp
                .json::<FlightDetail>()
                .await
                .map(Some)
                .map_err(|e| ApiError::Decode(e.to_string())),
            404 => Ok(None),
            s => Err(ApiError::Status(s)),
        }
    }

    /// A GET that deserializes the JSON body into a wire type.
    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let url = format!("{}{path}", self.base_url);
        let resp = Request::get(&url)
            .send()
            .await
            .map_err(|e| ApiError::Unreachable(e.to_string()))?;
        match resp.status() {
            200..=299 => resp
                .json::<T>()
                .await
                .map_err(|e| ApiError::Decode(e.to_string())),
            s => Err(ApiError::Status(s)),
        }
    }
}
