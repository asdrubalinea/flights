//! Adapter for the readsb/tar1090-style JSON shared by **airplanes.live**,
//! **adsb.lol**, **adsb.fi**, and a local **dump1090/readsb** box — the same wire
//! shape differing only by base URL (ADR-0003).
//!
//! Endpoint: `GET {base}/point/{lat}/{lon}/{radius_nm}` (radius ≤ 250 nm for the
//! public APIs, ≈1 req/s, non-commercial). The response is
//! `{ "ac": [ { hex, flight, lat, lon, alt_baro: number | "ground", gs, track,
//! seen_pos, … } ], "now", … }`, with keys omitted when unavailable. We keep only
//! the fields the app reasons about, drop on-ground aircraft (`alt_baro ==
//! "ground"`) and any without a position, and map the rest into domain
//! [`Flight`]s.

use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::domain::{Flight, LatLon, Snapshot};

use super::{FlightSource, SourceError};

/// The public APIs cap the Search radius at 250 nm.
const MAX_RADIUS_NM: i64 = 250;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ReadsbSource {
    name: &'static str,
    /// Base URL with no trailing slash, e.g. `https://api.airplanes.live/v2`.
    base_url: String,
    min_interval: Duration,
    agent: ureq::Agent,
}

impl ReadsbSource {
    pub fn new(name: &'static str, base_url: String, min_interval: Duration) -> Self {
        // Don't treat 4xx/5xx as transport errors — we inspect the status code
        // ourselves so we can read Retry-After and distinguish rate-limiting from
        // outages.
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(HTTP_TIMEOUT))
            .user_agent(concat!("flights/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Self {
            name,
            base_url: base_url.trim_end_matches('/').to_string(),
            min_interval,
            agent,
        }
    }
}

impl FlightSource for ReadsbSource {
    fn name(&self) -> &str {
        self.name
    }

    fn min_interval(&self) -> Duration {
        self.min_interval
    }

    fn fetch(&self, area: &crate::domain::SearchArea) -> Result<Snapshot, SourceError> {
        let radius = (area.radius_nm.round() as i64).clamp(1, MAX_RADIUS_NM);
        let url = format!(
            "{}/point/{:.6}/{:.6}/{}",
            self.base_url, area.center.lat, area.center.lon, radius
        );

        let mut resp = self.agent.get(&url).call().map_err(map_transport_error)?;

        let status = resp.status().as_u16();
        match status {
            200..=299 => {
                let body = resp
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| SourceError::Decode(e.to_string()))?;
                parse_snapshot(&body, Instant::now())
            }
            429 => Err(SourceError::RateLimited {
                retry_after: retry_after(&resp),
            }),
            401 | 403 => Err(SourceError::Auth),
            500..=599 => Err(SourceError::Unavailable),
            _ => Err(SourceError::Decode(format!(
                "unexpected HTTP status {status}"
            ))),
        }
    }
}

/// Map a ureq transport/timeout error to a domain [`SourceError`]. Status codes
/// never reach here — we disabled `http_status_as_error`.
fn map_transport_error(err: ureq::Error) -> SourceError {
    match err {
        ureq::Error::Timeout(_)
        | ureq::Error::Io(_)
        | ureq::Error::ConnectionFailed
        | ureq::Error::HostNotFound => SourceError::Transient,
        other => SourceError::Decode(other.to_string()),
    }
}

/// Parse a `Retry-After` header expressed as an integer number of seconds.
fn retry_after(resp: &ureq::http::Response<ureq::Body>) -> Option<Duration> {
    let secs: f64 = resp
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Parse a readsb point-query body into a domain [`Snapshot`], filtering out
/// on-ground aircraft and any without a position. Pure and `&str`-based so it can
/// be tested against the golden fixture without a network call.
fn parse_snapshot(body: &str, taken_at: Instant) -> Result<Snapshot, SourceError> {
    let wire: WireResponse =
        serde_json::from_str(body).map_err(|e| SourceError::Decode(e.to_string()))?;
    let flights = wire.ac.into_iter().filter_map(to_flight).collect();
    Ok(Snapshot::new(flights, taken_at))
}

#[derive(Deserialize)]
struct WireResponse {
    #[serde(default)]
    ac: Vec<WireAircraft>,
}

#[derive(Deserialize)]
struct WireAircraft {
    hex: String,
    #[serde(default)]
    flight: Option<String>,
    /// ICAO aircraft type designator, e.g. `"B738"`.
    #[serde(default)]
    t: Option<String>,
    /// Human-readable model description, e.g. `"BOEING 737-800"`.
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lon: Option<f64>,
    #[serde(default)]
    alt_baro: Option<AltBaro>,
    #[serde(default)]
    gs: Option<f64>,
    #[serde(default)]
    track: Option<f64>,
    #[serde(default)]
    seen_pos: Option<f64>,
}

/// `alt_baro` is a number of feet *or* the string `"ground"`. An untagged enum
/// maps both without a custom deserializer: a JSON number becomes `Feet`, the
/// string falls through to `Label`.
#[derive(Deserialize)]
#[serde(untagged)]
enum AltBaro {
    Feet(f64),
    Label(String),
}

/// Map one wire aircraft into a domain [`Flight`], or `None` if it should be
/// dropped (on the ground, or no position to place it).
fn to_flight(w: WireAircraft) -> Option<Flight> {
    let (lat, lon) = (w.lat?, w.lon?);

    let altitude_ft = match &w.alt_baro {
        Some(AltBaro::Feet(ft)) => Some(*ft),
        // On the ground — not an airborne flight.
        Some(AltBaro::Label(s)) if s.eq_ignore_ascii_case("ground") => return None,
        // Some other label, or no altitude reported: airborne, altitude unknown.
        Some(AltBaro::Label(_)) | None => None,
    };

    Some(Flight {
        hex: w.hex,
        ident: trimmed_nonempty(w.flight),
        aircraft_type: trimmed_nonempty(w.t),
        model: trimmed_nonempty(w.desc),
        position: LatLon::new(lat, lon),
        altitude_ft,
        groundspeed_kt: w.gs,
        track_deg: w.track,
        reported_age: Duration::from_secs_f64(w.seen_pos.unwrap_or(0.0).max(0.0)),
    })
}

/// Trim a wire string and treat the empty result as absent — the readsb feeds
/// pad fields with spaces (`"SWA157  "`) and omit-vs-blank are equivalent here.
fn trimmed_nonempty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../../tests/fixtures/airplanes_live_point.json");

    #[test]
    fn parses_golden_payload_filtering_ground_and_positionless() {
        let snap = parse_snapshot(GOLDEN, Instant::now()).expect("golden must parse");
        // 166 aircraft in the payload, 61 on the ground → 105 airborne, all with
        // a position (see the live-payload analysis when the fixture was saved).
        assert_eq!(snap.flights.len(), 105);

        // No on-ground aircraft survived the filter.
        assert!(snap
            .flights
            .iter()
            .all(|f| f.position.lat != 0.0 || f.position.lon != 0.0));

        // Aircraft type / model are carried through and trimmed (e.g. SWA157).
        let swa = snap
            .flights
            .iter()
            .find(|f| f.hex == "a2fee9")
            .expect("SWA157 is airborne in the fixture");
        assert_eq!(swa.aircraft_type.as_deref(), Some("B737"));
        assert_eq!(swa.model.as_deref(), Some("BOEING 737-700"));

        // Idents are trimmed; exactly one aircraft in this payload is anonymous.
        let anonymous = snap.flights.iter().filter(|f| f.ident.is_none()).count();
        assert_eq!(anonymous, 1);
        assert!(snap
            .flights
            .iter()
            .filter_map(|f| f.ident.as_deref())
            .all(|id| id == id.trim() && !id.is_empty()));
    }

    #[test]
    fn alt_baro_ground_string_drops_aircraft() {
        let body = r#"{"ac":[
            {"hex":"aaa","lat":1.0,"lon":2.0,"alt_baro":"ground","gs":5.0},
            {"hex":"bbb","lat":3.0,"lon":4.0,"alt_baro":12000,"gs":300.0,"track":90.0,"seen_pos":2.5,"t":"B738","desc":"BOEING 737-800"}
        ]}"#;
        let snap = parse_snapshot(body, Instant::now()).unwrap();
        assert_eq!(snap.flights.len(), 1);
        let f = &snap.flights[0];
        assert_eq!(f.hex, "bbb");
        assert_eq!(f.altitude_ft, Some(12000.0));
        assert_eq!(f.track_deg, Some(90.0));
        assert_eq!(f.reported_age, Duration::from_secs_f64(2.5));
        assert_eq!(f.aircraft_type.as_deref(), Some("B738"));
        assert_eq!(f.model.as_deref(), Some("BOEING 737-800"));
    }

    #[test]
    fn missing_type_and_model_are_none() {
        // A flight with neither `t` nor `desc` (common for uncatalogued craft).
        let body = r#"{"ac":[{"hex":"ddd","lat":1.0,"lon":2.0,"alt_baro":9000}]}"#;
        let snap = parse_snapshot(body, Instant::now()).unwrap();
        let f = &snap.flights[0];
        assert_eq!(f.aircraft_type, None);
        assert_eq!(f.model, None);
    }

    #[test]
    fn aircraft_without_position_is_dropped() {
        let body = r#"{"ac":[{"hex":"ccc","alt_baro":30000,"gs":400.0}]}"#;
        let snap = parse_snapshot(body, Instant::now()).unwrap();
        assert!(snap.flights.is_empty());
    }

    #[test]
    fn empty_or_missing_ac_array_is_an_empty_snapshot() {
        assert!(parse_snapshot(r#"{"now":1.0}"#, Instant::now())
            .unwrap()
            .flights
            .is_empty());
        assert!(parse_snapshot(r#"{"ac":[]}"#, Instant::now())
            .unwrap()
            .flights
            .is_empty());
    }

    #[test]
    fn garbage_body_is_a_decode_error() {
        assert!(matches!(
            parse_snapshot("not json", Instant::now()),
            Err(SourceError::Decode(_))
        ));
    }

    /// Live smoke test against the real airplanes.live API. Ignored by default
    /// (network + rate-limited); run on demand with `cargo test -- --ignored`.
    #[test]
    #[ignore = "hits the live airplanes.live API"]
    fn live_airplanes_live_fetch_returns_airborne_flights() {
        use crate::domain::{LatLon, SearchArea};
        let source = ReadsbSource::new(
            "airplanes.live",
            "https://api.airplanes.live/v2".into(),
            Duration::from_secs(1),
        );
        let area = SearchArea {
            center: LatLon::new(51.47, -0.4543), // London Heathrow — reliably busy
            radius_nm: 100.0,
        };
        let snapshot = source.fetch(&area).expect("live fetch should succeed");
        assert!(
            !snapshot.flights.is_empty(),
            "expected airborne flights over London"
        );
        // Idents that survive are trimmed and non-empty; on-ground filtered out.
        assert!(snapshot
            .flights
            .iter()
            .filter_map(|f| f.ident.as_deref())
            .all(|id| id == id.trim() && !id.is_empty()));
    }
}
