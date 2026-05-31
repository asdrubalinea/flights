//! The domain → wire mapping (ADR-0005). The HTTP handlers ([`crate::http`]) call
//! these to turn the [`Tracker`]'s answers into the serde DTOs of `flights-api`.
//! This is the *only* place the internal types (monotonic [`Instant`], the domain
//! [`Track`]/[`Flight`], the [`Health`] enum) cross into the wire shapes; the
//! conversions live here so the contract has one home.
//!
//! Every response is built from a single [`Tracker::picture_at`] pass at one
//! request instant, so a Client's radar, list, and status can never disagree. The
//! request instant is stamped two ways: the monotonic [`Instant`] for
//! dead reckoning and snapshot age, and a wall-clock epoch second for the
//! Client-facing `as_of`.

use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flights_api as api;

use crate::config::Config;
use crate::domain::VerticalTrend;
use crate::geo::Cpa;
use crate::tracker::{Health, Track, Tracker};

/// `GET /nearest`: the single Nearest flight (smallest ground distance), or
/// `flight: null` when the airspace is empty.
pub fn nearest(tracker: &RwLock<Tracker>) -> api::NearestResponse {
    let now = Instant::now();
    let as_of = epoch_now();
    let t = read(tracker);
    let picture = t.picture_at(now);
    let (health, _last_error) = wire_health(&picture.health);
    api::NearestResponse {
        as_of,
        health,
        snapshot_age_s: t.snapshot_age(now).map(dur_s),
        flight: picture.nearest().map(wire_flight),
    }
}

/// `GET /picture`: every track nearest-first, the Pacing flight's hex, and how
/// fresh the data is — all from one dead-reckoning pass.
pub fn picture(tracker: &RwLock<Tracker>) -> api::PictureResponse {
    let now = Instant::now();
    let as_of = epoch_now();
    let t = read(tracker);
    let picture = t.picture_at(now);
    let (health, last_error) = wire_health(&picture.health);
    api::PictureResponse {
        as_of,
        health,
        last_error,
        snapshot_age_s: t.snapshot_age(now).map(dur_s),
        pacing_hex: picture.pacing().map(|tr| tr.flight.hex.clone()),
        tracks: picture.tracks.iter().map(wire_flight).collect(),
    }
}

/// `GET /flight/{hex}`: one flight's full detail (promoted fields plus the opaque
/// grouped `details`), or `None` once it has left the area (the handler maps that
/// to a `404`).
pub fn flight_detail(tracker: &RwLock<Tracker>, hex: &str) -> Option<api::FlightDetail> {
    let now = Instant::now();
    let t = read(tracker);
    let picture = t.picture_at(now);
    let track = picture.tracks.iter().find(|tr| tr.flight.hex == hex)?;
    Some(api::FlightDetail {
        flight: wire_flight(track),
        details: track.flight.details.iter().map(wire_detail_group).collect(),
    })
}

/// `GET /meta`: the unchanging facts about this Server, built once at startup and
/// cloned per request. `source` is the active Source's display name.
pub fn build_meta(cfg: &Config, source: &str) -> api::Meta {
    api::Meta {
        home: api::LatLon {
            lat: cfg.home.lat,
            lon: cfg.home.lon,
        },
        radius_nm: cfg.search.radius_nm,
        relevance_nm: cfg.search.relevance_distance_nm,
        source: source.to_string(),
        units: api::Units::aviation(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Map a dead-reckoned domain [`Track`] to the wire [`api::Flight`]: the promoted
/// typed fields, the estimated position carried as flat `lat`/`lon` *and* as
/// `distance_nm`/`bearing_deg`, the server-derived vertical trend, and the CPA.
/// The opaque `details` are deliberately left out here (they ride `/flight/{hex}`).
fn wire_flight(t: &Track) -> api::Flight {
    let f = &t.flight;
    api::Flight {
        hex: f.hex.clone(),
        ident: f.ident.clone(),
        aircraft_type: f.aircraft_type.clone(),
        model: f.model.clone(),
        registration: f.registration.clone(),
        operator: f.operator.clone(),
        lat: t.estimated.lat,
        lon: t.estimated.lon,
        distance_nm: t.distance_nm,
        bearing_deg: t.bearing_from_home,
        altitude_ft: f.altitude_ft,
        geometric_altitude_ft: f.geometric_altitude_ft,
        groundspeed_kt: f.groundspeed_kt,
        track_deg: f.track_deg,
        vertical_rate_fpm: f.vertical_rate_fpm,
        vertical_trend: wire_trend(f.vertical_trend()),
        squawk: f.squawk.clone(),
        emergency: f.emergency.clone(),
        emitter_category: f.emitter_category.clone(),
        age_s: dur_s(t.age),
        cpa: t.cpa.map(wire_cpa),
    }
}

fn wire_detail_group(g: &crate::domain::DetailGroup) -> api::DetailGroup {
    api::DetailGroup {
        title: g.title.clone(),
        fields: g
            .fields
            .iter()
            .map(|(label, value)| api::DetailField {
                label: label.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn wire_cpa(c: Cpa) -> api::Cpa {
    api::Cpa {
        time_to_cpa_s: c.time_to_cpa_s,
        cpa_distance_nm: c.cpa_distance_nm,
    }
}

fn wire_trend(t: VerticalTrend) -> api::VerticalTrend {
    match t {
        VerticalTrend::Climb => api::VerticalTrend::Climb,
        VerticalTrend::Descend => api::VerticalTrend::Descend,
        VerticalTrend::Level => api::VerticalTrend::Level,
        VerticalTrend::Unknown => api::VerticalTrend::Unknown,
    }
}

/// Split the domain [`Health`] into the wire enum plus the last poll error, which
/// the wire carries as a separate `last_error` field (only `/picture` surfaces it).
fn wire_health(h: &Health) -> (api::Health, Option<String>) {
    match h {
        Health::Live => (api::Health::Live, None),
        Health::Stale { last_error } => (api::Health::Stale, last_error.clone()),
        Health::NoData { last_error } => (api::Health::NoData, last_error.clone()),
    }
}

/// Wall-clock epoch seconds for the Client-facing `as_of`. Before the Unix epoch
/// (an unset clock) reads as 0 rather than panicking.
fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn dur_s(d: Duration) -> f64 {
    d.as_secs_f64()
}

/// Read the Tracker, recovering from a poisoned lock (see [`crate::poller`]).
fn read(tracker: &RwLock<Tracker>) -> std::sync::RwLockReadGuard<'_, Tracker> {
    tracker.read().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DetailGroup, Flight, LatLon, SearchArea, Snapshot};
    use crate::tracker::TrackerConfig;
    use std::sync::{Arc, RwLock};
    use std::time::Instant;

    fn area() -> SearchArea {
        SearchArea {
            center: LatLon::new(0.0, 0.0),
            radius_nm: 100.0,
        }
    }

    fn tracker_cfg() -> TrackerConfig {
        TrackerConfig {
            relevance_distance_nm: 30.0,
            stale_after: Duration::from_secs(120),
            max_flight_age: Duration::from_secs(120),
        }
    }

    fn moving(hex: &str, lat: f64, lon: f64, track: f64, gs: f64) -> Flight {
        Flight {
            hex: hex.into(),
            ident: Some(hex.to_uppercase()),
            aircraft_type: Some("B738".into()),
            model: Some("BOEING 737-800".into()),
            registration: Some("N1".into()),
            operator: Some("ACME".into()),
            position: LatLon::new(lat, lon),
            altitude_ft: Some(30_000.0),
            geometric_altitude_ft: Some(30_500.0),
            groundspeed_kt: Some(gs),
            track_deg: Some(track),
            vertical_rate_fpm: Some(1200.0),
            squawk: Some("1200".into()),
            emergency: None,
            emitter_category: Some("large".into()),
            reported_age: Duration::ZERO,
            details: vec![DetailGroup {
                title: "Signal".into(),
                fields: vec![("RSSI".into(), "-7.4 dBFS".into())],
            }],
        }
    }

    fn shared(flights: Vec<Flight>) -> Arc<RwLock<Tracker>> {
        let mut tr = Tracker::new(area(), tracker_cfg());
        tr.ingest(Snapshot::new(flights, Instant::now()));
        Arc::new(RwLock::new(tr))
    }

    #[test]
    fn nearest_maps_the_closest_flight_with_promotions() {
        // North (receding) overhead is nearer; south inbound is farther.
        let near = moving("near", 0.05, 0.0, 0.0, 400.0);
        let far = moving("far", -0.4, 0.0, 0.0, 400.0);
        let t = shared(vec![far, near]);

        let resp = nearest(&t);
        assert!(matches!(resp.health, api::Health::Live));
        let flight = resp.flight.expect("a nearest flight");
        assert_eq!(flight.hex, "near");
        assert_eq!(flight.aircraft_type.as_deref(), Some("B738"));
        assert_eq!(flight.vertical_trend, api::VerticalTrend::Climb);
        // Carries both flat lat/lon and distance/bearing.
        assert!(flight.distance_nm > 0.0);
        assert!(flight.cpa.is_some());
    }

    #[test]
    fn picture_is_nearest_first_with_pacing_hex() {
        // Receding overhead (nearest), and an inbound that paces.
        let receding = moving("recede", 0.05, 0.0, 0.0, 400.0);
        let inbound = moving("inbound", -20.0 / 60.0, 0.0, 0.0, 480.0);
        let t = shared(vec![receding, inbound]);

        let resp = picture(&t);
        assert_eq!(resp.tracks.first().unwrap().hex, "recede");
        assert_eq!(resp.pacing_hex.as_deref(), Some("inbound"));
        // Both flights are carried. That the long-tail `details` never inline into
        // /picture is enforced by the type (`api::Flight` has no such field), not
        // assertable at runtime.
        assert_eq!(resp.tracks.len(), 2);
    }

    #[test]
    fn flight_detail_present_carries_details_absent_is_none() {
        let t = shared(vec![moving("abc123", 0.05, 0.0, 90.0, 400.0)]);
        let detail = flight_detail(&t, "abc123").expect("present");
        assert_eq!(detail.flight.hex, "abc123");
        assert_eq!(detail.details.len(), 1);
        assert_eq!(detail.details[0].title, "Signal");
        assert_eq!(detail.details[0].fields[0].label, "RSSI");

        assert!(flight_detail(&t, "nope").is_none());
    }

    #[test]
    fn empty_airspace_is_a_live_picture_with_null_flight() {
        let t = shared(vec![]);
        let n = nearest(&t);
        assert!(n.flight.is_none());
        let p = picture(&t);
        assert!(p.tracks.is_empty());
        assert!(p.pacing_hex.is_none());
    }
}
