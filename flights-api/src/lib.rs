//! The **flights** REST wire format: the single shared contract between the
//! Server (ADR-0005) and its thin Clients (the TUI, the waybar module, a future
//! webclient). Nothing here knows about geometry, sources, or the monotonic clock
//! — it is serde DTOs and unit constants only, so the Server can `Serialize` what
//! it computes and every Client can `Deserialize` the same shapes in lockstep.
//!
//! Conventions, fixed across every endpoint:
//! - **snake_case** field names with **unit suffixes** (`distance_nm`,
//!   `altitude_ft`, `groundspeed_kt`, `track_deg`, `age_s`).
//! - **Fixed aviation units** (see [`Units`]); a Client converts for display.
//! - **Epoch-seconds** timestamps (`as_of`) and second-valued ages — no date/time
//!   crate; the Server fills them from `SystemTime` at request time.
//! - **Explicit `null`** for absent optional fields (a stable schema a Client can
//!   rely on), which is plain serde `Option` behaviour with no `skip_serializing`.
//! - Each flight carries **both** an estimated `lat`/`lon` *and* a
//!   `distance_nm`/`bearing_deg`, so a Client never recomputes geometry.
//!
//! The API is unversioned (`Meta::version` reports the Server build) until an
//! independently-deployed consumer appears — until then Clients update in lockstep
//! with this crate (ADR-0005).

use serde::{Deserialize, Serialize};

/// A geographic point. Used for **Home** in [`Meta`] and as each flight's
/// estimated position is carried as flat `lat`/`lon` on [`Flight`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

/// How fresh the Server's view is, mirroring its internal health: a fresh-enough
/// Snapshot (`Live`), a held-but-aged one (`Stale`), or none yet (`NoData`). The
/// human-readable last poll error rides the separate `last_error` field on
/// [`PictureResponse`], not this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Live,
    Stale,
    NoData,
}

/// Whether a flight is climbing, descending, or level — **derived server-side**
/// from the vertical rate, so a Client maps it straight to a glyph with no
/// threshold logic of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerticalTrend {
    Climb,
    Descend,
    Level,
    Unknown,
}

/// The flight's **closest point of approach** to Home, as the Server computed it.
/// A negative `time_to_cpa_s` means the closest pass is already behind it (the
/// flight is receding).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cpa {
    pub time_to_cpa_s: f64,
    pub cpa_distance_nm: f64,
}

/// One flight as the Server sees it *at the instant of the request* — the typed,
/// cross-provider fields promoted onto the domain `Flight` (ADR-0004),
/// dead-reckoned and with geometry already derived. This is what `/picture`'s
/// `tracks[]` and `/nearest`'s `flight` carry. The opaque long-tail `details`
/// are **not** here — they would be ~40 strings per flight at the Client's frame
/// rate; they ride [`FlightDetail`] on `/flight/{hex}` instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flight {
    /// ICAO 24-bit address — the stable identity across snapshots.
    pub hex: String,
    /// Callsign / flight number, or `null` when the operator blocks it.
    pub ident: Option<String>,
    /// ICAO aircraft type designator (e.g. `"B738"`), or `null`.
    #[serde(rename = "type")]
    pub aircraft_type: Option<String>,
    /// Human-readable model (e.g. `"BOEING 737-800"`), or `null`.
    pub model: Option<String>,
    /// Civil registration / tail number, or `null`.
    pub registration: Option<String>,
    /// Owner/operating organisation, or `null`.
    pub operator: Option<String>,
    /// Estimated (dead-reckoned) latitude at the request instant.
    pub lat: f64,
    /// Estimated (dead-reckoned) longitude at the request instant.
    pub lon: f64,
    /// Ground distance from Home to the estimated position.
    pub distance_nm: f64,
    /// Bearing from Home to the estimated position, degrees clockwise from north.
    pub bearing_deg: f64,
    /// Barometric altitude, or `null`. Never part of the nearest-flight measure.
    pub altitude_ft: Option<f64>,
    /// Geometric (GNSS) altitude, or `null`.
    pub geometric_altitude_ft: Option<f64>,
    /// Groundspeed, or `null`.
    pub groundspeed_kt: Option<f64>,
    /// Track over the ground, degrees clockwise from north, or `null`.
    pub track_deg: Option<f64>,
    /// Vertical rate (climb positive), or `null`.
    pub vertical_rate_fpm: Option<f64>,
    /// Climb/descend/level, derived server-side (see [`VerticalTrend`]).
    pub vertical_trend: VerticalTrend,
    /// Transponder squawk (octal text, e.g. `"1200"`), or `null`.
    pub squawk: Option<String>,
    /// Broadcast emergency state, or `null` when none.
    pub emergency: Option<String>,
    /// Decoded coarse aircraft classification (e.g. `"large"`), or `null`.
    pub emitter_category: Option<String>,
    /// Effective age of the underlying position report at the request instant.
    pub age_s: f64,
    /// Closest point of approach, or `null` when the flight is not usably moving.
    pub cpa: Option<Cpa>,
}

/// One `(label, value)` pair inside a [`DetailGroup`]: a display-only string the
/// Server's adapter pre-formatted, rendered verbatim by a Client (ADR-0004).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetailField {
    pub label: String,
    pub value: String,
}

/// A titled section of the flight-detail popup (e.g. `"Signal"`, `"Integrity"`),
/// holding already-formatted `(label, value)` pairs. The adapter is the only
/// layer that understands the underlying wire fields; a Client renders these
/// blindly and reasons about none of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetailGroup {
    pub title: String,
    pub fields: Vec<DetailField>,
}

/// One flight's **full** detail, served by `/flight/{hex}`: every promoted field
/// (flattened from [`Flight`], so the JSON is one flat object) plus the opaque,
/// adapter-formatted [`DetailGroup`]s shown only in the detail popup. Fetched
/// once when the popup opens; the Server returns the last-known detail while the
/// flight is still in the area and `404`s once it leaves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlightDetail {
    #[serde(flatten)]
    pub flight: Flight,
    pub details: Vec<DetailGroup>,
}

/// `GET /nearest` — the single **Nearest flight** (smallest ground distance from
/// Home) if any, else `flight: null` with a `200` (the "if any"). What the waybar
/// module reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NearestResponse {
    /// Request instant, epoch seconds.
    pub as_of: f64,
    pub health: Health,
    /// Age of the held Snapshot at the request instant, or `null` if none yet.
    pub snapshot_age_s: Option<f64>,
    /// The Nearest flight, or `null` when the airspace is empty.
    pub flight: Option<Flight>,
}

/// `GET /picture` — the complete, self-consistent airspace at one instant, from a
/// single dead-reckoning pass so the radar, list, and status can never disagree.
/// `tracks` is **nearest-first** (so the Nearest flight is `tracks[0]`); only
/// `pacing_hex` is called out, since pacing is not derivable from distance order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PictureResponse {
    /// Request instant, epoch seconds.
    pub as_of: f64,
    pub health: Health,
    /// The most recent poll failure, if any — surfaced once the picture is stale.
    pub last_error: Option<String>,
    /// Age of the held Snapshot at the request instant, or `null` if none yet.
    pub snapshot_age_s: Option<f64>,
    /// `hex` of the **Pacing flight** (soonest relevant CPA), or `null` when the
    /// airspace is quiet. Distinct from `tracks[0]`, the Nearest flight.
    pub pacing_hex: Option<String>,
    /// Every flight dead-reckoned to `as_of`, sorted by ground distance ascending.
    pub tracks: Vec<Flight>,
}

/// The fixed aviation units every numeric field is expressed in. Static for a
/// given Server build; a Client reads these from `/meta` and converts for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Units {
    pub distance: String,
    pub altitude: String,
    pub speed: String,
    pub bearing: String,
    pub vertical_rate: String,
}

impl Units {
    /// The units the Server always serves: nautical miles, feet, knots, degrees,
    /// feet per minute.
    pub fn aviation() -> Self {
        Self {
            distance: "nm".into(),
            altitude: "ft".into(),
            speed: "kt".into(),
            bearing: "deg".into(),
            vertical_rate: "fpm".into(),
        }
    }
}

/// `GET /meta` — the unchanging facts about this Server: where Home is, how big
/// the Search area is, the Relevance distance that gates pacing, which Source is
/// active, the units, and the Server build version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub home: LatLon,
    pub radius_nm: f64,
    pub relevance_nm: f64,
    pub source: String,
    pub units: Units,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_flight() -> Flight {
        Flight {
            hex: "abc123".into(),
            ident: Some("TEST1".into()),
            aircraft_type: Some("B738".into()),
            model: Some("BOEING 737-800".into()),
            registration: Some("N1".into()),
            operator: None, // exercises explicit-null
            lat: 1.0,
            lon: 2.0,
            distance_nm: 12.5,
            bearing_deg: 24.0,
            altitude_ft: Some(13850.0),
            geometric_altitude_ft: Some(14900.0),
            groundspeed_kt: Some(319.5),
            track_deg: Some(197.3),
            vertical_rate_fpm: Some(-1472.0),
            vertical_trend: VerticalTrend::Descend,
            squawk: Some("0521".into()),
            emergency: None,
            emitter_category: Some("large".into()),
            age_s: 1.85,
            cpa: Some(Cpa {
                time_to_cpa_s: 135.7,
                cpa_distance_nm: 1.43,
            }),
        }
    }

    /// The wire format itself is the contract. This pins the bits a Client relies
    /// on: the `type` rename, snake_case enum values, explicit `null` for absent
    /// optionals (no `skip_serializing_if`), and that `FlightDetail` flattens so
    /// `hex` and `details` are siblings.
    #[test]
    fn flight_detail_json_shape_is_pinned() {
        let detail = FlightDetail {
            flight: sample_flight(),
            details: vec![DetailGroup {
                title: "Signal".into(),
                fields: vec![DetailField {
                    label: "RSSI".into(),
                    value: "-7.4 dBFS".into(),
                }],
            }],
        };
        let v: serde_json::Value = serde_json::to_value(&detail).unwrap();

        // `type`, not `aircraft_type`.
        assert_eq!(v["type"], "B738");
        // Enum is snake_case.
        assert_eq!(v["vertical_trend"], "descend");
        // Absent optional is an explicit null, not omitted.
        assert!(v.get("operator").is_some());
        assert!(v["operator"].is_null());
        // CPA is a nested object.
        assert_eq!(v["cpa"]["time_to_cpa_s"], 135.7);
        // FlightDetail flattens: promoted fields and `details` are siblings.
        assert_eq!(v["hex"], "abc123");
        assert_eq!(v["details"][0]["title"], "Signal");
        assert_eq!(v["details"][0]["fields"][0]["label"], "RSSI");
    }

    /// Every response type round-trips, so a Client deserializes exactly what the
    /// Server serialized.
    #[test]
    fn responses_round_trip() {
        let picture = PictureResponse {
            as_of: 1780000000.0,
            health: Health::Live,
            last_error: None,
            snapshot_age_s: Some(1.8),
            pacing_hex: Some("abc123".into()),
            tracks: vec![sample_flight()],
        };
        let s = serde_json::to_string(&picture).unwrap();
        assert_eq!(
            serde_json::from_str::<PictureResponse>(&s).unwrap(),
            picture
        );

        let nearest = NearestResponse {
            as_of: 1780000000.0,
            health: Health::Stale,
            snapshot_age_s: Some(200.0),
            flight: None,
        };
        let s = serde_json::to_string(&nearest).unwrap();
        assert_eq!(
            serde_json::from_str::<NearestResponse>(&s).unwrap(),
            nearest
        );

        let meta = Meta {
            home: LatLon { lat: 1.0, lon: 2.0 },
            radius_nm: 100.0,
            relevance_nm: 30.0,
            source: "airplanes.live".into(),
            units: Units::aviation(),
            version: "0.1.0".into(),
        };
        let s = serde_json::to_string(&meta).unwrap();
        assert_eq!(serde_json::from_str::<Meta>(&s).unwrap(), meta);
    }
}
