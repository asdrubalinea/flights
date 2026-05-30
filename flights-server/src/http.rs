//! The HTTP daemon (ADR-0006): a synchronous `tiny_http` server exposing the
//! read-only REST contract on loopback. A small pool of worker threads share the
//! one `Arc<RwLock<Tracker>>` the poller writes; each request takes the read lock,
//! builds a response via [`crate::api`], and serves it. No async runtime, no auth
//! (loopback is what makes that safe — ADR-0005). CORS is **opt-in**: an
//! `Access-Control-Allow-Origin` header is sent only when the operator configures
//! `server.cors_allow_origin` (off by default, so a stray browser tab can't read
//! Home's coordinates); a future webclient sets it to its own origin or `*`.
//!
//! Routes:
//! - `GET /nearest`      → [`flights_api::NearestResponse`]
//! - `GET /picture`      → [`flights_api::PictureResponse`]
//! - `GET /flight/{hex}` → [`flights_api::FlightDetail`], or `404` once it leaves
//! - `GET /meta`         → [`flights_api::Meta`]

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::thread;

use tiny_http::{Header, Method, Request, Response, Server};

use flights_api as api;

use crate::api as map;
use crate::tracker::Tracker;

/// Worker threads serving requests in parallel. A handful is plenty: each request
/// is a read-lock + dead-reckon + JSON encode, and the only clients are a TUI at a
/// few fps plus a waybar one-liner. More than one lets a slow client not stall the
/// others.
const WORKERS: usize = 4;

/// Bind the API socket. Split from [`serve`] so the caller can learn the actual
/// bound address (useful with an ephemeral `:0` port) before serving.
pub fn bind(addr: SocketAddr) -> anyhow::Result<Arc<Server>> {
    Server::http(addr)
        .map(Arc::new)
        .map_err(|e| anyhow::anyhow!("failed to bind the REST API to {addr}: {e}"))
}

/// Serve forever across [`WORKERS`] threads. Blocks; returns only if every
/// worker's `recv()` loop ends (i.e. the server is torn down).
pub fn serve(
    server: Arc<Server>,
    tracker: Arc<RwLock<Tracker>>,
    meta: api::Meta,
    cors: Option<String>,
) -> anyhow::Result<()> {
    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let server = Arc::clone(&server);
        let tracker = Arc::clone(&tracker);
        let meta = meta.clone();
        let cors = cors.clone();
        handles.push(thread::spawn(move || {
            // Ends when the server is dropped or the socket dies (recv errors).
            while let Ok(request) = server.recv() {
                handle(request, &tracker, &meta, cors.as_deref());
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

fn handle(request: Request, tracker: &RwLock<Tracker>, meta: &api::Meta, cors: Option<&str>) {
    let method = request.method().clone();
    let url = request.url().to_string();
    // Routing ignores any query string — none of these endpoints take parameters.
    let path = url.split('?').next().unwrap_or(&url);

    // CORS preflight: answer any OPTIONS with the configured headers and no body.
    if method == Method::Options {
        let _ = request.respond(preflight(cors));
        return;
    }
    if method != Method::Get {
        let _ = request.respond(json(405, &error_body("method not allowed"), cors));
        return;
    }

    let response = match path {
        "/nearest" => json(200, &map::nearest(tracker), cors),
        "/picture" => json(200, &map::picture(tracker), cors),
        "/meta" => json(200, meta, cors),
        p if p.starts_with("/flight/") => {
            let hex = &p["/flight/".len()..];
            match map::flight_detail(tracker, hex) {
                Some(detail) => json(200, &detail, cors),
                None => json(404, &error_body("flight not in the area"), cors),
            }
        }
        _ => json(404, &error_body("not found"), cors),
    };
    let _ = request.respond(response);
}

/// A JSON response with the content-type header, plus the `Access-Control-Allow-Origin`
/// header when CORS is configured. Serialization of our own DTOs cannot realistically
/// fail; if it somehow did we still send valid JSON.
fn json<T: serde::Serialize>(
    status: u16,
    body: &T,
    cors: Option<&str>,
) -> Response<Cursor<Vec<u8>>> {
    let text =
        serde_json::to_string(body).unwrap_or_else(|e| format!(r#"{{"error":"serialize: {e}"}}"#));
    let mut response = Response::from_string(text)
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json; charset=utf-8"));
    if let Some(origin) = cors {
        response = response.with_header(header("Access-Control-Allow-Origin", origin));
    }
    response
}

/// The 204 answer to a CORS preflight. When CORS is disabled the headers are
/// omitted (a browser then blocks the cross-origin read, which is the point).
fn preflight(cors: Option<&str>) -> Response<std::io::Empty> {
    let mut response = Response::empty(204);
    if let Some(origin) = cors {
        response = response
            .with_header(header("Access-Control-Allow-Origin", origin))
            .with_header(header("Access-Control-Allow-Methods", "GET, OPTIONS"))
            .with_header(header("Access-Control-Allow-Headers", "*"));
    }
    response
}

fn error_body(message: &str) -> serde_json::Value {
    serde_json::json!({ "error": message })
}

fn header(name: &str, value: &str) -> Header {
    // The static names/values here are always valid header bytes.
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Flight, LatLon, SearchArea, Snapshot};
    use crate::tracker::{Tracker, TrackerConfig};
    use std::time::{Duration, Instant};

    /// Boot the real server on an ephemeral port with one known airborne flight,
    /// returning its base URL. `cors` is the configured allow-origin (or `None`).
    /// The server thread is detached; the test process exiting reaps it.
    fn boot(cors: Option<&str>) -> String {
        let area = SearchArea {
            center: LatLon::new(0.0, 0.0),
            radius_nm: 100.0,
        };
        let cfg = TrackerConfig {
            relevance_distance_nm: 30.0,
            stale_after: Duration::from_secs(120),
            max_flight_age: Duration::from_secs(120),
        };
        let flight = Flight {
            hex: "abc123".into(),
            ident: Some("TEST1".into()),
            aircraft_type: Some("B738".into()),
            model: Some("BOEING 737-800".into()),
            registration: Some("N1".into()),
            operator: None,
            position: LatLon::new(0.05, 0.0),
            altitude_ft: Some(10_000.0),
            geometric_altitude_ft: None,
            groundspeed_kt: Some(400.0),
            track_deg: Some(180.0),
            vertical_rate_fpm: Some(1200.0),
            squawk: Some("1200".into()),
            emergency: None,
            emitter_category: Some("large".into()),
            reported_age: Duration::ZERO,
            details: vec![crate::domain::DetailGroup {
                title: "Signal".into(),
                fields: vec![("RSSI".into(), "-7.4 dBFS".into())],
            }],
        };
        let mut tr = Tracker::new(area, cfg);
        tr.ingest(Snapshot::new(vec![flight], Instant::now()));
        let tracker = Arc::new(RwLock::new(tr));

        let server = bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = server
            .server_addr()
            .to_ip()
            .expect("an IP address from the ephemeral bind");
        let meta = api::Meta {
            home: api::LatLon { lat: 0.0, lon: 0.0 },
            radius_nm: 100.0,
            relevance_nm: 30.0,
            source: "test".into(),
            units: api::Units::aviation(),
            version: "test".into(),
        };
        let cors = cors.map(str::to_string);
        thread::spawn(move || {
            let _ = serve(server, tracker, meta, cors);
        });
        format!("http://{addr}")
    }

    fn get(url: &str) -> (u16, String) {
        let mut resp = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .new_agent()
            .get(url)
            .call()
            .expect("request reaches the server");
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().unwrap();
        (status, body)
    }

    /// The `Access-Control-Allow-Origin` header on a GET, if any.
    fn cors_header(url: &str) -> Option<String> {
        let resp = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .new_agent()
            .get(url)
            .call()
            .expect("request reaches the server");
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    #[test]
    fn nearest_and_picture_and_meta_and_flight_routes() {
        let base = boot(Some("*"));

        // /nearest deserializes into the wire type and is the known flight.
        let (status, body) = get(&format!("{base}/nearest"));
        assert_eq!(status, 200);
        let nearest: api::NearestResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(nearest.flight.unwrap().hex, "abc123");

        // /picture is nearest-first with the pacing hex set (the inbound flight).
        let (status, body) = get(&format!("{base}/picture"));
        assert_eq!(status, 200);
        let picture: api::PictureResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(picture.tracks.first().unwrap().hex, "abc123");
        assert_eq!(picture.pacing_hex.as_deref(), Some("abc123"));

        // /meta round-trips.
        let (status, body) = get(&format!("{base}/meta"));
        assert_eq!(status, 200);
        let meta: api::Meta = serde_json::from_str(&body).unwrap();
        assert_eq!(meta.source, "test");

        // /flight/{hex} present carries the opaque detail groups.
        let (status, body) = get(&format!("{base}/flight/abc123"));
        assert_eq!(status, 200);
        let detail: api::FlightDetail = serde_json::from_str(&body).unwrap();
        assert_eq!(detail.flight.hex, "abc123");
        assert_eq!(detail.details[0].title, "Signal");

        // /flight/{hex} absent is a 404.
        let (status, _) = get(&format!("{base}/flight/nope"));
        assert_eq!(status, 404);

        // Unknown route is a 404.
        let (status, _) = get(&format!("{base}/bogus"));
        assert_eq!(status, 404);

        // With CORS configured, the allow-origin header is echoed.
        assert_eq!(cors_header(&format!("{base}/meta")).as_deref(), Some("*"));
    }

    #[test]
    fn no_cors_header_when_disabled() {
        // Off by default: a browser tab on another origin can't read /meta.
        let base = boot(None);
        let (status, _) = get(&format!("{base}/meta"));
        assert_eq!(status, 200);
        assert_eq!(cors_header(&format!("{base}/meta")), None);
    }
}
