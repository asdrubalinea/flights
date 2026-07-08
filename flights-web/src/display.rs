//! Display helpers shared by the radar canvas and the DOM panels — the webclient's
//! equivalent of the TUI's `render` helpers. Everything here is *projection and
//! formatting* of answers the Server already computed (CONTEXT.md): turning a
//! flight's polar `(distance_nm, bearing_deg)` into screen offsets, picking a
//! colour for its role, formatting a label. None of it derives a position,
//! distance, bearing, or which flight is nearest — drawing a Server answer is not
//! computing one (ADR-0007).

use flights_api::{ContactState, Cpa, Flight, VerticalTrend};

/// Groundspeed below this (knots) is treated as stationary, so no heading vector
/// is drawn. Mirrors the Server's domain floor — purely a display threshold here.
pub const MOVING_FLOOR_KT: f64 = 1.0;

// Accent colours, by role — the Airbus **EFIS colour philosophy** (ADR-0009):
// red=warning, amber=caution, green=active, cyan=for-info, magenta=managed-target,
// white=present. Kept in lockstep with the CSS custom properties in `style.css` so a
// flight reads the same on the canvas and in the list. Unlike before, these
// **deliberately diverge from the TUI's `blip_color`** — colour here signals meaning
// (ADR-0009); the TUI's palette is left unchanged.
pub const COL_SELECTED: &str = "#eef2f6"; // white — present / the selection box
pub const COL_EMERGENCY: &str = "#ff3b30"; // red — warning (the `emergency` state)
pub const COL_CAUTION: &str = "#ffb01f"; // amber — caution (lost / stale contact)
pub const COL_PACING: &str = "#ff5cf0"; // magenta — the managed target (Pacing flight)
pub const COL_NEAREST: &str = "#25e36a"; // green — active (the Nearest flight)
pub const COL_APPROACH: &str = "#25e36a"; // green — active (approaching traffic)
pub const COL_RECEDE: &str = "#28d6ff"; // cyan — for-info (receding traffic)
pub const COL_ANON: &str = "#5b7a86"; // dim cyan — inactive (ident-blocked)
pub const COL_HOME: &str = "#eef2f6"; // white — reference (Home)
pub const COL_RING: &str = "#2b4a40"; // faint scope ring
pub const COL_CROSS: &str = "#16242a"; // fainter cross-hair

/// A display label: the callsign, or the bracketed hex for an ident-blocked flight.
pub fn label(f: &Flight) -> String {
    f.ident
        .clone()
        .unwrap_or_else(|| format!("[{}]", f.hex))
}

/// A flight whose closest pass is still ahead of it (the Server signs it via
/// `time_to_cpa_s`).
pub fn approaching(c: &Cpa) -> bool {
    c.time_to_cpa_s >= 0.0
}

/// The TCAS-style symbol shape for a track, by contact + emergency state (ADR-0009).
/// Colour is separate (`symbol_color`); this is only the glyph geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// In-contact traffic — a filled diamond.
    Diamond,
    /// Lost/stale contact — a hollow diamond (it fades by age, ADR-0007).
    OpenDiamond,
    /// Broadcasting an emergency — a filled square (the TCAS "RA" shape).
    Square,
}

/// Pick a track's symbol shape. Emergency wins over contact state, mirroring the
/// colour precedence in `symbol_color`.
pub fn symbol_shape(state: ContactState, emergency: Option<&str>) -> Shape {
    if emergency.is_some() {
        return Shape::Square;
    }
    match state {
        ContactState::InContact => Shape::Diamond,
        _ => Shape::OpenDiamond, // landed / left_scope / lost_contact
    }
}

/// A flight's velocity as `(track_deg, groundspeed_kt)` when usably moving, for the
/// radar heading vector. `None` ⇒ no vector drawn.
pub fn velocity(f: &Flight) -> Option<(f64, f64)> {
    match (f.track_deg, f.groundspeed_kt) {
        (Some(track), Some(gs)) if gs >= MOVING_FLOOR_KT => Some((track, gs)),
        _ => None,
    }
}

/// A flight's radar position as `(east, north)` nm offsets from Home — the polar
/// answer the Server gave, expressed in Cartesian nm for the canvas to scale.
pub fn radar_xy(f: &Flight) -> (f64, f64) {
    let b = f.bearing_deg.to_radians();
    (f.distance_nm * b.sin(), f.distance_nm * b.cos())
}

/// Whether a flight carries one of the radar's call-out roles (nearest / pacing /
/// selected) and so gets a label drawn beside its blip.
pub fn is_flagged(
    f: &Flight,
    nearest: Option<&str>,
    pacing: Option<&str>,
    selected: Option<&str>,
) -> bool {
    let hex = f.hex.as_str();
    Some(hex) == nearest
        || Some(hex) == pacing
        || Some(hex) == selected
        || f.emergency.is_some()
}

/// A track's *fill* colour, by the Airbus EFIS philosophy (ADR-0009). Precedence,
/// highest wins: **emergency** (red) → **lost/stale** contact (amber) → **pacing**
/// (magenta) → **nearest** (green) → ident-blocked (dim cyan) → **approaching**
/// (green) / **receding** (cyan). Selection is *not* a fill — it is an orthogonal
/// white box drawn around the symbol (see `radar.rs`) — so a selected emergency
/// still reads red. Anonymous flights are dimmed unless they carry a louder role.
/// Deliberately diverges from the TUI's `blip_color` (ADR-0009).
pub fn symbol_color(f: &Flight, nearest: Option<&str>, pacing: Option<&str>) -> &'static str {
    let hex = f.hex.as_str();
    if f.emergency.is_some() {
        return COL_EMERGENCY;
    }
    if f.state != ContactState::InContact {
        return COL_CAUTION; // lost / stale — never "active" green
    }
    if pacing == Some(hex) {
        return COL_PACING;
    }
    if nearest == Some(hex) {
        return COL_NEAREST;
    }
    if f.ident.is_none() {
        return COL_ANON;
    }
    match &f.cpa {
        Some(c) if approaching(c) => COL_APPROACH,
        _ => COL_RECEDE,
    }
}

/// The glyph and colour for a vertical trend. Unknown renders blank rather than a
/// fake symbol, like the TUI.
pub fn trend_glyph(trend: VerticalTrend) -> (&'static str, &'static str) {
    match trend {
        VerticalTrend::Climb => ("↑", COL_APPROACH), // green
        VerticalTrend::Descend => ("↓", COL_RECEDE), // cyan — distinct from climb
        VerticalTrend::Level => ("–", COL_ANON),
        VerticalTrend::Unknown => ("", COL_ANON),
    }
}

/// The status block's one-line summary of the Nearest flight.
pub fn fmt_position(f: &Flight) -> String {
    let alt = f
        .altitude_ft
        .map(|a| format!("{a:.0} ft"))
        .unwrap_or_else(|| "alt ?".into());
    let kind = f
        .aircraft_type
        .as_deref()
        .map(|k| format!(" {k}"))
        .unwrap_or_default();
    format!(
        "{}{kind} — {:.1} nm @ {:03.0}° ({alt})",
        label(f),
        f.distance_nm,
        f.bearing_deg
    )
}

/// The status block's one-line summary of the Pacing flight, including its CPA.
pub fn fmt_pacing(f: &Flight) -> String {
    match &f.cpa {
        Some(c) => format!(
            "{} — CPA {:.1} nm in {:.0}s",
            label(f),
            c.cpa_distance_nm,
            c.time_to_cpa_s
        ),
        None => label(f),
    }
}

/// The first flight broadcasting an emergency, if any — what the ECAM-style master
/// warning keys off (ADR-0009). Keyed on the Server's `emergency` *state*, never the
/// client interpreting special squawk codes (CONTEXT.md, *Squawk*).
pub fn any_emergency(tracks: &[Flight]) -> Option<&Flight> {
    tracks.iter().find(|t| t.emergency.is_some())
}

/// Flight level from a barometric altitude: hundreds of feet, three digits
/// (`13850 ft → "FL139"`). Display-only formatting; the underlying altitude is the
/// Server's, never recomputed.
pub fn flight_level(alt_ft: f64) -> String {
    format!("FL{:03.0}", (alt_ft / 100.0).round())
}

/// The scratchpad's one line, by priority (ADR-0009): a live error (amber) first,
/// then a summary of the selected flight, else the key-hint. Returns the text and a
/// CSS class. Pure, so the precedence is testable without a browser.
pub fn scratchpad_line(error: Option<&str>, selected: Option<&str>) -> (String, &'static str) {
    if let Some(e) = error {
        return (format!("! {e}"), "err");
    }
    if let Some(s) = selected {
        return (s.to_string(), "sel");
    }
    (
        "◂ select · ↑/↓ step · ENTER detail · ESC clear".into(),
        "hint",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal in-contact, identified, non-emergency flight. Tests mutate the few
    /// fields each case cares about.
    fn flight(hex: &str) -> Flight {
        Flight {
            hex: hex.into(),
            ident: Some("TEST".into()),
            aircraft_type: None,
            model: None,
            registration: None,
            operator: None,
            lat: 0.0,
            lon: 0.0,
            distance_nm: 10.0,
            bearing_deg: 0.0,
            altitude_ft: None,
            geometric_altitude_ft: None,
            groundspeed_kt: None,
            track_deg: None,
            vertical_rate_fpm: None,
            vertical_trend: VerticalTrend::Unknown,
            squawk: None,
            emergency: None,
            emitter_category: None,
            state: ContactState::InContact,
            age_s: 0.0,
            cpa: None,
        }
    }

    #[test]
    fn emergency_outranks_every_other_role() {
        let mut f = flight("aaa");
        f.emergency = Some("general".into());
        // Even as the nearest *and* pacing flight, emergency wins.
        assert_eq!(symbol_color(&f, Some("aaa"), Some("aaa")), COL_EMERGENCY);
    }

    #[test]
    fn lost_contact_is_amber_even_when_nearest() {
        let mut f = flight("bbb");
        f.state = ContactState::LostContact;
        assert_eq!(symbol_color(&f, Some("bbb"), None), COL_CAUTION);
    }

    #[test]
    fn pacing_outranks_nearest() {
        let f = flight("ccc");
        assert_eq!(symbol_color(&f, Some("ccc"), Some("ccc")), COL_PACING);
    }

    #[test]
    fn nearest_is_green() {
        let f = flight("ddd");
        assert_eq!(symbol_color(&f, Some("ddd"), None), COL_NEAREST);
    }

    #[test]
    fn anon_is_dim_but_approach_and_recede_split_by_cpa() {
        let mut anon = flight("eee");
        anon.ident = None;
        assert_eq!(symbol_color(&anon, None, None), COL_ANON);

        let mut approaching = flight("fff");
        approaching.cpa = Some(Cpa {
            time_to_cpa_s: 60.0,
            cpa_distance_nm: 5.0,
        });
        assert_eq!(symbol_color(&approaching, None, None), COL_APPROACH);

        let mut receding = flight("ggg");
        receding.cpa = Some(Cpa {
            time_to_cpa_s: -60.0,
            cpa_distance_nm: 5.0,
        });
        assert_eq!(symbol_color(&receding, None, None), COL_RECEDE);
    }

    #[test]
    fn emergency_flight_is_always_flagged_for_a_label() {
        let mut f = flight("hhh");
        f.emergency = Some("general".into());
        assert!(is_flagged(&f, None, None, None));
    }

    #[test]
    fn any_emergency_finds_the_first_emergency_flight() {
        let mut a = flight("aaa");
        let mut b = flight("bbb");
        let c = flight("ccc");
        assert!(any_emergency(&[a.clone(), c.clone()]).is_none());
        b.emergency = Some("general".into());
        // Returns the emergency flight even when it isn't first in the list.
        assert_eq!(any_emergency(&[a.clone(), b.clone(), c.clone()]).unwrap().hex, "bbb");
        // First emergency wins when several are present.
        a.emergency = Some("downed".into());
        assert_eq!(any_emergency(&[a, b, c]).unwrap().hex, "aaa");
    }

    #[test]
    fn flight_level_is_hundreds_of_feet_three_digits() {
        assert_eq!(flight_level(13850.0), "FL139"); // rounds to nearest hundred
        assert_eq!(flight_level(35000.0), "FL350");
        assert_eq!(flight_level(900.0), "FL009"); // zero-padded to three digits
        assert_eq!(flight_level(0.0), "FL000");
    }

    #[test]
    fn scratchpad_prefers_error_then_selection_then_hint() {
        // Error wins even when a flight is selected.
        assert_eq!(scratchpad_line(Some("boom"), Some("SEL x")).1, "err");
        // Then the selection summary.
        assert_eq!(scratchpad_line(None, Some("SEL x")), ("SEL x".into(), "sel"));
        // Else the key hint.
        assert_eq!(scratchpad_line(None, None).1, "hint");
    }

    #[test]
    fn symbol_shape_by_state_and_emergency() {
        assert_eq!(symbol_shape(ContactState::InContact, None), Shape::Diamond);
        assert_eq!(
            symbol_shape(ContactState::LostContact, None),
            Shape::OpenDiamond
        );
        assert_eq!(symbol_shape(ContactState::Landed, None), Shape::OpenDiamond);
        // Emergency overrides any contact state.
        assert_eq!(
            symbol_shape(ContactState::InContact, Some("general")),
            Shape::Square
        );
        assert_eq!(
            symbol_shape(ContactState::LostContact, Some("general")),
            Shape::Square
        );
    }
}
