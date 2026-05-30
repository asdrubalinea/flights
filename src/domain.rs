//! The stable internal contract every layer above `sources/` speaks.
//!
//! These types are deliberately **provider-neutral**: nothing here mentions a
//! bounding box, an API key, an epoch timestamp, or "ground" as a wire value.
//! Each [`crate::sources`] adapter maps its own wire format into these types,
//! and the poller, tracker, and UI only ever see these. See `CONTEXT.md` for the
//! domain language each name below is drawn from.

use std::time::{Duration, Instant};

/// A geographic point. Both the fixed **Home** and every flight position are `LatLon`s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub const fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
}

/// The **Search area**: the region around Home we ask a Source about, expressed
/// in the domain as Home plus a radius. The rest of the app only ever thinks in
/// radius — box-only Sources convert to a bounding box and filter back *inside
/// their adapter* (ADR-0003).
#[derive(Debug, Clone, Copy)]
pub struct SearchArea {
    /// Home — the single fixed point all distances are measured from.
    pub center: LatLon,
    pub radius_nm: f64,
}

/// One airborne flight as last reported by a Source.
///
/// Only the fields the app reasons about or renders are kept; a Source omits any
/// it cannot supply, so the optional ones are genuinely optional. On-ground
/// aircraft never become a `Flight` — adapters filter them out (airborne only).
#[derive(Debug, Clone)]
pub struct Flight {
    /// ICAO 24-bit address — the stable identity of an aircraft across Snapshots.
    pub hex: String,
    /// Callsign / flight number. `None` when the operator blocks it; such a
    /// flight is rendered as an anonymous blip but still tracked.
    pub ident: Option<String>,
    /// ICAO aircraft type designator (e.g. `"B738"`, `"C172"`), when the Source
    /// supplies it — a compact code suited to the list column. Like `ident`, it
    /// is purely descriptive: it never affects nearest/pacing.
    pub aircraft_type: Option<String>,
    /// Human-readable model description (e.g. `"BOEING 737-800"`), when known.
    /// Shown in the selected-flight detail; often absent for GA/uncatalogued craft.
    pub model: Option<String>,
    /// Last reported ground position.
    pub position: LatLon,
    /// Barometric altitude in feet, when known. Altitude is **not** part of the
    /// nearest-flight measure — it is carried only for display.
    pub altitude_ft: Option<f64>,
    /// Groundspeed in knots, when known.
    pub groundspeed_kt: Option<f64>,
    /// Track over the ground, degrees clockwise from true north, when known.
    pub track_deg: Option<f64>,
    /// How old the position report already was at the Snapshot's instant (the
    /// Source's `seen_pos`). Dead reckoning extrapolates by this *plus* the time
    /// elapsed since the Snapshot, so the estimate tracks true wall-clock age.
    pub reported_age: Duration,
}

impl Flight {
    /// The flight's velocity as `(track_deg, groundspeed_kt)` when it is usably
    /// moving — both components known and groundspeed above a small floor.
    /// `None` means we cannot dead-reckon or estimate a CPA for it; it holds
    /// position and never paces.
    pub fn velocity(&self) -> Option<(f64, f64)> {
        match (self.track_deg, self.groundspeed_kt) {
            (Some(track), Some(gs)) if gs >= MOVING_FLOOR_KT => Some((track, gs)),
            _ => None,
        }
    }
}

/// Groundspeed below this (knots) is treated as stationary: no dead reckoning,
/// no CPA. Filters out parked/taxiing noise that slips through the ground filter.
pub const MOVING_FLOOR_KT: f64 = 1.0;

/// A **Snapshot**: the complete set of airborne flights in the Search area as of
/// a single poll. A new Snapshot replaces the previous one wholesale — flights it
/// omits have left the area.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub flights: Vec<Flight>,
    /// When *this process* received the Snapshot, on the local monotonic clock —
    /// the basis for dead reckoning and staleness. Provider epoch timestamps are
    /// deliberately not used as the time base, so clock skew can't corrupt it.
    pub taken_at: Instant,
}

impl Snapshot {
    pub fn new(flights: Vec<Flight>, taken_at: Instant) -> Self {
        Self { flights, taken_at }
    }
}
