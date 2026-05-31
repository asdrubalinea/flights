//! Display helpers shared by the radar canvas and the DOM panels — the webclient's
//! equivalent of the TUI's `render` helpers. Everything here is *projection and
//! formatting* of answers the Server already computed (CONTEXT.md): turning a
//! flight's polar `(distance_nm, bearing_deg)` into screen offsets, picking a
//! colour for its role, formatting a label. None of it derives a position,
//! distance, bearing, or which flight is nearest — drawing a Server answer is not
//! computing one (ADR-0007).

use flights_api::{Cpa, Flight, VerticalTrend};

/// Groundspeed below this (knots) is treated as stationary, so no heading vector
/// is drawn. Mirrors the Server's domain floor — purely a display threshold here.
pub const MOVING_FLOOR_KT: f64 = 1.0;

// Blip / accent colours, by role. Kept in lockstep with the CSS custom properties
// in `style.css` so a flight reads the same on the canvas and in the list.
pub const COL_SELECTED: &str = "#f5f6f7";
pub const COL_PACING: &str = "#ff6b6b";
pub const COL_NEAREST: &str = "#5fd7ff";
pub const COL_ANON: &str = "#5a6270";
pub const COL_APPROACH: &str = "#a6e22e";
pub const COL_RECEDE: &str = "#5c9eff";
pub const COL_HOME: &str = "#ffd866";
pub const COL_RING: &str = "#39414d";
pub const COL_CROSS: &str = "#222831";

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
    Some(hex) == nearest || Some(hex) == pacing || Some(hex) == selected
}

/// Blip / row colour by priority: selected, then pacing, then nearest, then
/// approach state. Anonymous (ident-blocked) flights are dimmed unless flagged.
/// Mirrors the TUI's `blip_color`.
pub fn blip_color(
    f: &Flight,
    nearest: Option<&str>,
    pacing: Option<&str>,
    selected: Option<&str>,
) -> &'static str {
    let hex = f.hex.as_str();
    if selected == Some(hex) {
        return COL_SELECTED;
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
        VerticalTrend::Climb => ("↑", COL_APPROACH),
        VerticalTrend::Descend => ("↓", COL_NEAREST),
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
