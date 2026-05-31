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

use crate::domain::{DetailGroup, Flight, LatLon, Snapshot};

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

/// Parse a readsb point-query body into a domain [`Snapshot`]. Airborne aircraft
/// with a position become [`Flight`]s; aircraft the feed reports **on the ground**
/// contribute only their hex to `on_ground` (ADR-0007), so the Tracker can mark an
/// already-tracked flight *landed* without ever creating a new ground blip. Pure and
/// `&str`-based so it can be tested against the golden fixture without a network call.
fn parse_snapshot(body: &str, taken_at: Instant) -> Result<Snapshot, SourceError> {
    let wire: WireResponse =
        serde_json::from_str(body).map_err(|e| SourceError::Decode(e.to_string()))?;

    let mut flights = Vec::new();
    let mut on_ground = Vec::new();
    for w in wire.ac {
        if is_on_ground(&w) {
            on_ground.push(w.hex);
        } else if let Some(f) = to_flight(w) {
            flights.push(f);
        }
    }
    Ok(Snapshot::with_ground(flights, on_ground, taken_at))
}

/// Whether a wire aircraft is on the ground (`alt_baro == "ground"`). Such an
/// aircraft is not an airborne [`Flight`]; ADR-0007 keeps its hex so an
/// *already-tracked* flight that has touched down can be marked **landed**.
fn is_on_ground(w: &WireAircraft) -> bool {
    matches!(&w.alt_baro, Some(Altitude::Label(s)) if s.eq_ignore_ascii_case("ground"))
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
    /// Civil registration / tail number, e.g. `"N292WN"`.
    #[serde(default)]
    r: Option<String>,
    /// Owner/operator, e.g. `"SOUTHWEST AIRLINES CO"`.
    #[serde(default, rename = "ownOp")]
    own_op: Option<String>,
    /// Year of manufacture, e.g. `"2007"` (the wire sends it as a string).
    #[serde(default)]
    year: Option<String>,
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lon: Option<f64>,
    #[serde(default)]
    alt_baro: Option<Altitude>,
    /// Geometric (GNSS) altitude — same number-or-`"ground"` shape as `alt_baro`.
    #[serde(default)]
    alt_geom: Option<Altitude>,
    #[serde(default)]
    gs: Option<f64>,
    #[serde(default)]
    track: Option<f64>,
    /// Barometric vertical rate, feet per minute (signed; climb positive).
    #[serde(default)]
    baro_rate: Option<f64>,
    /// Geometric vertical rate — fallback when `baro_rate` is absent.
    #[serde(default)]
    geom_rate: Option<f64>,
    #[serde(default)]
    squawk: Option<String>,
    #[serde(default)]
    emergency: Option<String>,
    /// ICAO emitter category code, e.g. `"A3"`.
    #[serde(default)]
    category: Option<String>,
    // --- Navigation / autopilot selections ---
    #[serde(default)]
    nav_qnh: Option<f64>,
    #[serde(default)]
    nav_altitude_mcp: Option<f64>,
    #[serde(default)]
    nav_heading: Option<f64>,
    #[serde(default)]
    nav_modes: Option<Vec<String>>,
    // --- Position-quality / integrity figures ---
    #[serde(default)]
    nic: Option<i64>,
    #[serde(default)]
    rc: Option<i64>,
    #[serde(default)]
    nic_baro: Option<i64>,
    #[serde(default)]
    nac_p: Option<i64>,
    #[serde(default)]
    nac_v: Option<i64>,
    #[serde(default)]
    sil: Option<i64>,
    #[serde(default)]
    sil_type: Option<String>,
    #[serde(default)]
    gva: Option<i64>,
    #[serde(default)]
    sda: Option<i64>,
    // --- Provenance ---
    /// How the position was derived, e.g. `"adsb_icao"`, `"mlat"`, `"tisb_icao"`.
    #[serde(default, rename = "type")]
    data_type: Option<String>,
    #[serde(default)]
    version: Option<i64>,
    #[serde(default)]
    mlat: Option<Vec<String>>,
    #[serde(default)]
    tisb: Option<Vec<String>>,
    #[serde(default)]
    alert: Option<i64>,
    #[serde(default)]
    spi: Option<i64>,
    // --- Signal ---
    #[serde(default)]
    messages: Option<i64>,
    /// Seconds since the last message of any kind from this aircraft.
    #[serde(default)]
    seen: Option<f64>,
    #[serde(default)]
    rssi: Option<f64>,
    /// Seconds since the last *position* update — the basis for `reported_age`.
    #[serde(default)]
    seen_pos: Option<f64>,
}

/// An altitude field is a number of feet *or* the string `"ground"`. An untagged
/// enum maps both without a custom deserializer: a JSON number becomes `Feet`, the
/// string falls through to `Label`. Shared by `alt_baro` and `alt_geom`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Altitude {
    Feet(f64),
    Label(String),
}

/// Map one airborne wire aircraft into a domain [`Flight`], or `None` if it has no
/// position to place it. On-ground aircraft are routed to `on_ground` upstream by
/// [`is_on_ground`] and never reach here.
fn to_flight(w: WireAircraft) -> Option<Flight> {
    let (lat, lon) = (w.lat?, w.lon?);

    let altitude_ft = match &w.alt_baro {
        Some(Altitude::Feet(ft)) => Some(*ft),
        // A non-numeric label (or no altitude): airborne, altitude unknown. The
        // `"ground"` label was already filtered out in `parse_snapshot`.
        Some(Altitude::Label(_)) | None => None,
    };

    // Geometric altitude shares the number-or-`"ground"` shape; a label just means
    // no usable geometric altitude (the barometric check above already dropped
    // on-ground craft, so we don't re-filter on it).
    let geometric_altitude_ft = match &w.alt_geom {
        Some(Altitude::Feet(ft)) => Some(*ft),
        _ => None,
    };

    // Group the display-only telemetry before we move fields out of `w`.
    let details = build_details(&w);

    Some(Flight {
        hex: w.hex,
        ident: trimmed_nonempty(w.flight),
        aircraft_type: trimmed_nonempty(w.t),
        model: trimmed_nonempty(w.desc),
        registration: trimmed_nonempty(w.r),
        operator: trimmed_nonempty(w.own_op),
        position: LatLon::new(lat, lon),
        altitude_ft,
        geometric_altitude_ft,
        groundspeed_kt: w.gs,
        track_deg: w.track,
        // Prefer the barometric rate; fall back to the geometric one.
        vertical_rate_fpm: w.baro_rate.or(w.geom_rate),
        squawk: trimmed_nonempty(w.squawk),
        // `"none"` is the routine no-emergency value — collapse it to absent.
        emergency: trimmed_nonempty(w.emergency).filter(|s| !s.eq_ignore_ascii_case("none")),
        emitter_category: trimmed_nonempty(w.category).map(|c| decode_category(&c)),
        reported_age: Duration::from_secs_f64(w.seen_pos.unwrap_or(0.0).max(0.0)),
        details,
    })
}

/// Trim a wire string and treat the empty result as absent — the readsb feeds
/// pad fields with spaces (`"SWA157  "`) and omit-vs-blank are equivalent here.
fn trimmed_nonempty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Decode an ICAO emitter-category code into a short human label. Unknown codes
/// pass through unchanged so an unrecognized aircraft still surfaces its code
/// rather than vanishing.
fn decode_category(code: &str) -> String {
    let label = match code {
        "A1" => "light",
        "A2" => "small",
        "A3" => "large",
        "A4" => "high-vortex large",
        "A5" => "heavy",
        "A6" => "high performance",
        "A7" => "rotorcraft",
        "B1" => "glider / sailplane",
        "B2" => "lighter-than-air",
        "B3" => "parachutist",
        "B4" => "ultralight / hang-glider",
        "B6" => "unmanned",
        "B7" => "space vehicle",
        "C1" => "surface — emergency vehicle",
        "C2" => "surface — service vehicle",
        "C3" => "point obstacle",
        "A0" | "B0" | "C0" => "no info",
        _ => return code.to_string(),
    };
    label.to_string()
}

/// Decode the readsb `type` (how the position was derived) into a readable label.
fn decode_data_type(t: &str) -> &str {
    match t {
        "adsb_icao" => "ADS-B (ICAO)",
        "adsb_icao_nt" => "ADS-B (ICAO, non-transponder)",
        "adsr_icao" => "ADS-R (ICAO)",
        "tisb_icao" => "TIS-B (ICAO)",
        "adsc" => "ADS-C",
        "mlat" => "MLAT",
        "mode_s" => "Mode S",
        "adsb_other" => "ADS-B (other)",
        "adsr_other" => "ADS-R (other)",
        "tisb_other" => "TIS-B (other)",
        "tisb_trackfile" => "TIS-B trackfile",
        other => other,
    }
}

/// Append a group only when it carries at least one field — empty sections never
/// reach the popup.
fn push_group(groups: &mut Vec<DetailGroup>, title: &str, fields: Vec<(String, String)>) {
    if !fields.is_empty() {
        groups.push(DetailGroup {
            title: title.into(),
            fields,
        });
    }
}

/// Build the display-only [`DetailGroup`]s from one wire aircraft. The adapter is
/// the only layer that understands these wire fields (ADR-0003/0004); it formats
/// and groups them here, and the popup renders the result verbatim. Every field is
/// pushed only when present — nothing is fabricated.
fn build_details(w: &WireAircraft) -> Vec<DetailGroup> {
    let mut groups = Vec::new();

    let mut signal = Vec::new();
    if let Some(v) = w.rssi {
        signal.push(("RSSI".into(), format!("{v:.1} dBFS")));
    }
    if let Some(v) = w.messages {
        signal.push(("Messages".into(), v.to_string()));
    }
    if let Some(v) = w.seen {
        signal.push(("Last seen".into(), format!("{v:.1}s ago")));
    }
    push_group(&mut groups, "Signal", signal);

    let mut integrity = Vec::new();
    if let Some(v) = w.nic {
        integrity.push(("NIC".into(), v.to_string()));
    }
    if let Some(v) = w.rc {
        integrity.push(("Radius of containment".into(), format!("{v} m")));
    }
    if let Some(v) = w.nic_baro {
        integrity.push(("NIC baro".into(), v.to_string()));
    }
    if let Some(v) = w.nac_p {
        integrity.push(("NACp".into(), v.to_string()));
    }
    if let Some(v) = w.nac_v {
        integrity.push(("NACv".into(), v.to_string()));
    }
    if let Some(v) = w.sil {
        integrity.push(("SIL".into(), v.to_string()));
    }
    if let Some(s) = w.sil_type.as_deref() {
        integrity.push(("SIL type".into(), s.to_string()));
    }
    if let Some(v) = w.gva {
        integrity.push(("GVA".into(), v.to_string()));
    }
    if let Some(v) = w.sda {
        integrity.push(("SDA".into(), v.to_string()));
    }
    push_group(&mut groups, "Integrity", integrity);

    let mut nav = Vec::new();
    if let Some(v) = w.nav_qnh {
        nav.push(("QNH".into(), format!("{v:.1} hPa")));
    }
    if let Some(v) = w.nav_altitude_mcp {
        nav.push(("Selected altitude".into(), format!("{v:.0} ft")));
    }
    if let Some(v) = w.nav_heading {
        nav.push(("Selected heading".into(), format!("{v:.0}°")));
    }
    if let Some(modes) = w.nav_modes.as_ref().filter(|m| !m.is_empty()) {
        nav.push(("Modes".into(), modes.join(", ")));
    }
    push_group(&mut groups, "Navigation", nav);

    let mut prov = Vec::new();
    if let Some(s) = w.data_type.as_deref() {
        prov.push(("Data source".into(), decode_data_type(s).into()));
    }
    if let Some(v) = w.version {
        prov.push(("ADS-B version".into(), v.to_string()));
    }
    if let Some(m) = w.mlat.as_ref().filter(|m| !m.is_empty()) {
        prov.push(("MLAT fields".into(), m.join(", ")));
    }
    if let Some(m) = w.tisb.as_ref().filter(|m| !m.is_empty()) {
        prov.push(("TIS-B fields".into(), m.join(", ")));
    }
    if w.alert == Some(1) {
        prov.push(("Alert".into(), "yes".into()));
    }
    if w.spi == Some(1) {
        prov.push(("SPI / ident".into(), "yes".into()));
    }
    push_group(&mut groups, "Provenance", prov);

    let mut airframe = Vec::new();
    if let Some(s) = w.year.as_deref() {
        airframe.push(("Year".into(), s.to_string()));
    }
    push_group(&mut groups, "Airframe", airframe);

    groups
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

        // The 61 on-ground aircraft are kept as bare hexes (ADR-0007), not as
        // flights — so the Tracker can land an already-tracked hex, but no new
        // ground blip is ever created.
        assert_eq!(snap.on_ground.len(), 61);
        // No on-ground aircraft survived into the airborne flights.
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
        // Promoted telemetry fields are carried through and decoded.
        assert_eq!(swa.registration.as_deref(), Some("N292WN"));
        assert_eq!(swa.operator.as_deref(), Some("SOUTHWEST AIRLINES CO"));
        assert_eq!(swa.vertical_rate_fpm, Some(2304.0));
        assert_eq!(swa.geometric_altitude_ft, Some(32700.0));
        assert_eq!(swa.squawk.as_deref(), Some("1324"));
        assert_eq!(swa.emergency, None); // "none" collapses to absent
        assert_eq!(swa.emitter_category.as_deref(), Some("large")); // A3

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
    fn alt_baro_ground_string_routes_hex_to_on_ground() {
        let body = r#"{"ac":[
            {"hex":"aaa","lat":1.0,"lon":2.0,"alt_baro":"ground","gs":5.0},
            {"hex":"bbb","lat":3.0,"lon":4.0,"alt_baro":12000,"alt_geom":12500,"gs":300.0,"track":90.0,"baro_rate":-640,"seen_pos":2.5,"t":"B738","desc":"BOEING 737-800","r":"N1","ownOp":"ACME","squawk":"1200","emergency":"none","category":"A3"}
        ]}"#;
        let snap = parse_snapshot(body, Instant::now()).unwrap();
        // The ground aircraft is not a flight, but its hex is retained so the
        // Tracker can mark an already-tracked "aaa" as landed (ADR-0007).
        assert_eq!(snap.flights.len(), 1);
        assert_eq!(snap.on_ground, vec!["aaa".to_string()]);
        let f = &snap.flights[0];
        assert_eq!(f.hex, "bbb");
        assert_eq!(f.altitude_ft, Some(12000.0));
        assert_eq!(f.geometric_altitude_ft, Some(12500.0));
        assert_eq!(f.track_deg, Some(90.0));
        assert_eq!(f.reported_age, Duration::from_secs_f64(2.5));
        assert_eq!(f.aircraft_type.as_deref(), Some("B738"));
        assert_eq!(f.model.as_deref(), Some("BOEING 737-800"));
        assert_eq!(f.registration.as_deref(), Some("N1"));
        assert_eq!(f.operator.as_deref(), Some("ACME"));
        assert_eq!(f.squawk.as_deref(), Some("1200"));
        assert_eq!(f.emergency, None);
        assert_eq!(f.emitter_category.as_deref(), Some("large"));
        assert_eq!(f.vertical_rate_fpm, Some(-640.0));
        assert_eq!(f.vertical_trend(), crate::domain::VerticalTrend::Descend);
    }

    #[test]
    fn builds_detail_groups_from_golden() {
        let snap = parse_snapshot(GOLDEN, Instant::now()).unwrap();
        let swa = snap
            .flights
            .iter()
            .find(|f| f.hex == "a2fee9")
            .expect("SWA157 is airborne");

        let group = |title: &str| swa.details.iter().find(|g| g.title == title);
        let field = |g: &DetailGroup, label: &str| {
            g.fields
                .iter()
                .find(|(l, _)| l == label)
                .map(|(_, v)| v.clone())
        };

        let signal = group("Signal").expect("Signal group");
        assert_eq!(field(signal, "Messages").as_deref(), Some("32842"));
        assert_eq!(field(signal, "RSSI").as_deref(), Some("-15.2 dBFS"));

        let nav = group("Navigation").expect("Navigation group");
        assert_eq!(field(nav, "QNH").as_deref(), Some("1013.6 hPa"));
        assert_eq!(field(nav, "Selected altitude").as_deref(), Some("36000 ft"));

        let prov = group("Provenance").expect("Provenance group");
        assert_eq!(field(prov, "Data source").as_deref(), Some("ADS-B (ICAO)"));
        assert_eq!(field(prov, "ADS-B version").as_deref(), Some("2"));
        // a2fee9 has empty mlat/tisb arrays → those fields are omitted entirely.
        assert!(field(prov, "MLAT fields").is_none());
        assert!(field(prov, "TIS-B fields").is_none());

        // ASA668 (ad5269) carries tisb: ["geom_rate"] → a TIS-B field appears.
        let asa = snap
            .flights
            .iter()
            .find(|f| f.hex == "ad5269")
            .expect("ASA668 is airborne");
        let asa_prov = asa
            .details
            .iter()
            .find(|g| g.title == "Provenance")
            .expect("Provenance group");
        assert_eq!(
            asa_prov
                .fields
                .iter()
                .find(|(l, _)| l == "TIS-B fields")
                .map(|(_, v)| v.as_str()),
            Some("geom_rate")
        );
    }

    #[test]
    fn decode_category_known_and_unknown() {
        assert_eq!(decode_category("A3"), "large");
        assert_eq!(decode_category("A7"), "rotorcraft");
        // Unknown codes pass through unchanged — no data loss.
        assert_eq!(decode_category("ZZ"), "ZZ");
    }

    #[test]
    fn vertical_rate_falls_back_to_geometric() {
        // No `baro_rate`; only `geom_rate` → the domain carries the geometric rate.
        let body = r#"{"ac":[
            {"hex":"ccc","lat":1.0,"lon":2.0,"alt_baro":5000,"geom_rate":-512}
        ]}"#;
        let snap = parse_snapshot(body, Instant::now()).unwrap();
        let f = &snap.flights[0];
        assert_eq!(f.vertical_rate_fpm, Some(-512.0));
        assert_eq!(f.vertical_trend(), crate::domain::VerticalTrend::Descend);
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
