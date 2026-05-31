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

/// Source-contributed, display-only telemetry for one flight, already formatted
/// and grouped by the adapter (the only layer that understands the wire fields).
/// Rendered verbatim in the flight-detail popup with no per-Source code; never
/// parsed back; never affects the Nearest or Pacing flight. Names no wire field
/// by design — every value is an opaque, pre-formatted string (ADR-0004).
#[derive(Debug, Clone, Default)]
pub struct DetailGroup {
    /// Section heading, e.g. `"Signal"`, `"Integrity"`.
    pub title: String,
    /// `(label, value)` pairs, pre-formatted with units by the adapter.
    pub fields: Vec<(String, String)>,
}

/// Whether a flight is climbing, descending, or holding level — the displayable
/// fact derived from [`Flight::vertical_rate_fpm`]. The UI maps it to a glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalTrend {
    Climb,
    Descend,
    Level,
    Unknown,
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
    /// Civil registration / tail number (e.g. `"N292WN"`), when the Source supplies
    /// it. Tied to the airframe, not the flight; purely descriptive — the **hex**
    /// remains the stable identity. Often absent for blocked or uncatalogued craft.
    pub registration: Option<String>,
    /// Owner/operating organisation (e.g. `"SOUTHWEST AIRLINES CO"`), when known.
    /// Descriptive only; commonly absent for GA aircraft.
    pub operator: Option<String>,
    /// Last reported ground position.
    pub position: LatLon,
    /// Barometric (pressure-derived) altitude in feet, when known. Altitude is
    /// **not** part of the nearest-flight measure — it is carried only for display.
    pub altitude_ft: Option<f64>,
    /// Geometric (GNSS-derived) altitude in feet, when known. Distinct from the
    /// barometric `altitude_ft` and may differ by hundreds of feet; display-only.
    pub geometric_altitude_ft: Option<f64>,
    /// Groundspeed in knots, when known.
    pub groundspeed_kt: Option<f64>,
    /// Track over the ground, degrees clockwise from true north, when known.
    pub track_deg: Option<f64>,
    /// Vertical rate in feet per minute, signed (climb positive), when known.
    /// Display-only — like altitude, it never affects the Nearest or Pacing flight.
    /// The Source's barometric rate when present, else its geometric rate.
    pub vertical_rate_fpm: Option<f64>,
    /// Transponder squawk code (an octal code, e.g. `"1200"`), carried as text and
    /// never as a number. Descriptive only.
    pub squawk: Option<String>,
    /// Emergency state, when one is being broadcast. `None` covers both "no field"
    /// and the routine `"none"` value, so a quiet flight surfaces nothing.
    pub emergency: Option<String>,
    /// Coarse aircraft classification decoded by the adapter (e.g. "large",
    /// "rotorcraft"), when the Source classifies it. The raw wire code lives only
    /// inside the adapter; the domain carries the human label. Descriptive only.
    pub emitter_category: Option<String>,
    /// How old the position report already was at the Snapshot's instant (the
    /// Source's `seen_pos`). Dead reckoning extrapolates by this *plus* the time
    /// elapsed since the Snapshot, so the estimate tracks true wall-clock age.
    pub reported_age: Duration,
    /// Source-contributed, display-only telemetry shown in the flight-detail popup.
    /// Empty when the Source supplies none. See [`DetailGroup`].
    pub details: Vec<DetailGroup>,
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

    /// Whether the flight is climbing, descending, or level, from its vertical
    /// rate. A rate within [`VERTICAL_LEVEL_FLOOR_FPM`] of zero reads as level so
    /// the display shows a level glyph rather than a misleadingly precise trend.
    pub fn vertical_trend(&self) -> VerticalTrend {
        match self.vertical_rate_fpm {
            None => VerticalTrend::Unknown,
            Some(r) if r.abs() < VERTICAL_LEVEL_FLOOR_FPM => VerticalTrend::Level,
            Some(r) if r > 0.0 => VerticalTrend::Climb,
            Some(_) => VerticalTrend::Descend,
        }
    }
}

/// Groundspeed below this (knots) is treated as stationary: no dead reckoning,
/// no CPA. Filters out parked/taxiing noise that slips through the ground filter.
pub const MOVING_FLOOR_KT: f64 = 1.0;

/// Vertical rate within this many feet per minute of zero is treated as level
/// flight — below the noise floor of a meaningful climb or descent.
pub const VERTICAL_LEVEL_FLOOR_FPM: f64 = 100.0;

/// A **Snapshot**: one poll's contribution to the held state — the airborne flights
/// it returned in the Search area, authoritative as of its timestamp. The
/// [`crate::tracker::Tracker`] **merges** a Snapshot into its retained set of tracks
/// rather than swapping it in wholesale (ADR-0007): a flight a poll reports is *in
/// contact*; one it omits is kept and becomes *lost*, not assumed gone. This
/// reverses the older "omitted ⇒ left the area" rule, which turned feed jitter into
/// flicker.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub flights: Vec<Flight>,
    /// Hexes the poll reported **on the ground** (`alt_baro == "ground"`). The
    /// airborne-only invariant (ADR-0003) is relaxed only at this seam: the adapter
    /// still maps just airborne aircraft to [`Flight`]s, but carries ground hexes so
    /// the Tracker can turn an *already-tracked* hex to **landed**. Untracked ground
    /// hexes are ignored — no new ground blips appear.
    pub on_ground: Vec<String>,
    /// When *this process* received the Snapshot, on the local monotonic clock —
    /// the basis for dead reckoning and staleness. Provider epoch timestamps are
    /// deliberately not used as the time base, so clock skew can't corrupt it.
    pub taken_at: Instant,
}

impl Snapshot {
    /// A Snapshot with no on-ground reports — a test convenience. Production builds
    /// every Snapshot through the adapter, which always uses [`Snapshot::with_ground`]
    /// (with a possibly-empty ground list), so this exists only under `cfg(test)`.
    #[cfg(test)]
    pub fn new(flights: Vec<Flight>, taken_at: Instant) -> Self {
        Self::with_ground(flights, Vec::new(), taken_at)
    }

    /// A Snapshot that also carries the hexes a poll reported on the ground, so the
    /// Tracker can mark an already-tracked flight **landed** (see [`Snapshot::on_ground`]).
    pub fn with_ground(flights: Vec<Flight>, on_ground: Vec<String>, taken_at: Instant) -> Self {
        Self {
            flights,
            on_ground,
            taken_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_rate(rate: Option<f64>) -> Flight {
        Flight {
            hex: "abc".into(),
            ident: None,
            aircraft_type: None,
            model: None,
            registration: None,
            operator: None,
            position: LatLon::new(0.0, 0.0),
            altitude_ft: None,
            geometric_altitude_ft: None,
            groundspeed_kt: None,
            track_deg: None,
            vertical_rate_fpm: rate,
            squawk: None,
            emergency: None,
            emitter_category: None,
            reported_age: Duration::ZERO,
            details: Vec::new(),
        }
    }

    #[test]
    fn vertical_trend_classifies_around_the_level_floor() {
        assert_eq!(with_rate(None).vertical_trend(), VerticalTrend::Unknown);
        assert_eq!(with_rate(Some(0.0)).vertical_trend(), VerticalTrend::Level);
        assert_eq!(with_rate(Some(99.0)).vertical_trend(), VerticalTrend::Level);
        assert_eq!(
            with_rate(Some(-99.0)).vertical_trend(),
            VerticalTrend::Level
        );
        assert_eq!(
            with_rate(Some(100.0)).vertical_trend(),
            VerticalTrend::Climb
        );
        assert_eq!(
            with_rate(Some(-100.0)).vertical_trend(),
            VerticalTrend::Descend
        );
    }
}
