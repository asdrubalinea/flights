//! Building the Waybar object (ADR-0008). A one-shot `/nearest` answer plus the
//! Client-side **Display range** become the `{text, tooltip, class}` JSON Waybar
//! reads on each tick. This is pure projection of what the Server already computed
//! — distance, bearing, the Nearest flight, contact state — never a recomputation
//! (CONTEXT.md): drawing a Server answer is not deriving one.
//!
//! The `class` a user styles in CSS is one of: default/`""` (in contact and live),
//! `lost` (the Nearest flight is frozen — retained and badged, never dropped, the
//! same flicker-avoidance as the TUI/ADR-0007), `stale` (the whole Picture has
//! aged), or `error` (Server unreachable or no data yet). Precedence, when more than
//! one could apply: `error` ▸ `stale` ▸ `lost` ▸ default — the broader the doubt
//! about the data, the louder the class. An empty sky (or a Nearest beyond the
//! Display range) emits empty `text`, which Waybar collapses to nothing.

use flights_api::{ContactState, Flight, Health, NearestResponse, VerticalTrend};

use crate::client::ClientError;

/// The Waybar custom-module payload: one JSON object per tick on stdout. Waybar
/// renders `text` on the bar, `tooltip` on hover, and applies `class` as a CSS class.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Bar {
    pub text: String,
    pub tooltip: String,
    pub class: String,
}

impl Bar {
    /// An empty module: Waybar collapses it to nothing. The quiet state — an empty
    /// sky, or a Nearest flight beyond the Display range.
    fn empty() -> Self {
        Self {
            text: String::new(),
            tooltip: String::new(),
            class: String::new(),
        }
    }

    /// The dim `error` stub: a plane glyph with no data and the reason in the
    /// tooltip. Non-empty on purpose, so an unreachable Server (or a broken config)
    /// is *visibly* down — the user styles `.error` dim/red — rather than silently
    /// collapsed like a genuinely empty sky.
    pub fn error(detail: &str) -> Self {
        Self {
            text: pango_escape("✈ ?"),
            tooltip: pango_escape(detail),
            class: "error".to_string(),
        }
    }
}

/// Render the outcome of the single `/nearest` call into a [`Bar`]. The
/// `Err` arm is the unreachable/odd-status/decode-failure stub; the `Ok` arm
/// applies the Display range and the class taxonomy.
pub fn render(outcome: &Result<NearestResponse, ClientError>, range_nm: f64) -> Bar {
    match outcome {
        Ok(resp) => build(resp, range_nm),
        Err(e) => Bar::error(&format!("flights-server: {e}")),
    }
}

/// Map a `/nearest` response to a [`Bar`] given the Client's Display range.
fn build(resp: &NearestResponse, range_nm: f64) -> Bar {
    // No successful poll has ever landed — distinct from a successful poll that
    // found an empty sky. The former is an `error` stub; the latter is quiet.
    if resp.health == Health::NoData {
        return Bar::error("no data yet — flights-server has no Snapshot");
    }

    // Empty sky (Live or Stale with nothing retained): stay quiet and let Waybar
    // collapse the module.
    let Some(f) = resp.flight.as_ref() else {
        return Bar::empty();
    };

    // Display range gate: show the Nearest flight only while within range. A NaN
    // distance is never `<=`, so it collapses too — never a flight with no position.
    if f.distance_nm <= range_nm {
        Bar {
            text: pango_escape(&text_line(f)),
            tooltip: pango_escape(&tooltip(f, resp)),
            class: class_for(resp.health, f.state).to_string(),
        }
    } else {
        Bar::empty()
    }
}

/// The rich one-line bar text agreed in the design (ADR-0008):
/// `✈ {ident} {type} {dist}nm {brg}° {alt} {trend}`, in aviation units. Type, altitude
/// and the trend glyph are dropped when the Source omitted them (never fabricated);
/// the fuller `Alt/Spd/Trk/Vr` breakdown lives in the tooltip. Styling for lost/stale
/// is the class's job in CSS, so the text keeps the same shape across states.
fn text_line(f: &Flight) -> String {
    let kind = f
        .aircraft_type
        .as_deref()
        .map(|t| format!(" {t}"))
        .unwrap_or_default();
    let alt = f
        .altitude_ft
        .map(|a| format!(" {a:.0}ft"))
        .unwrap_or_default();
    let glyph = trend_glyph(f.vertical_trend);
    let trend = if glyph.is_empty() {
        String::new()
    } else {
        format!(" {glyph}")
    };
    format!(
        "✈ {}{kind} {:.1}nm {:03.0}°{alt}{trend}",
        label(f),
        f.distance_nm,
        f.bearing_deg
    )
}

/// The class a user styles in CSS. `error` is handled before this is reached (no
/// flight to show), so here health is Live or Stale: a stale Picture outranks a
/// per-flight lost badge, which outranks the default in-contact state.
fn class_for(health: Health, state: ContactState) -> &'static str {
    match health {
        Health::Stale => "stale",
        // Live: the Nearest flight may still be one we've lost contact with — it is
        // the closest thing we know of, so it is shown and badged, never dropped.
        _ => match state {
            ContactState::InContact => "",
            _ => "lost",
        },
    }
}

/// The hover tooltip: the full `Alt/Spd/Trk/Vr` detail in aviation units (the wire's
/// fixed units), led by a stale banner when the Nearest flight is lost so its frozen,
/// last-known figures don't read as live.
fn tooltip(f: &Flight, resp: &NearestResponse) -> String {
    let mut lines: Vec<String> = Vec::new();

    if let Some(banner) = lost_banner(f) {
        lines.push(banner);
    }

    // Identity: callsign (or bracketed hex), then type and model when known.
    let kind = match (&f.aircraft_type, &f.model) {
        (Some(t), Some(m)) => format!(" · {t} {m}"),
        (Some(t), None) => format!(" · {t}"),
        (None, Some(m)) => format!(" · {m}"),
        (None, None) => String::new(),
    };
    lines.push(format!("{}{kind}", label(f)));
    if let Some(op) = &f.operator {
        lines.push(op.clone());
    }
    if let Some(reg) = &f.registration {
        lines.push(format!("Reg {reg}"));
    }

    lines.push(format!("{:.1} nm @ {:03.0}°", f.distance_nm, f.bearing_deg));

    // Alt / Spd / Trk / Vr — the detail the ADR names, each `—` when the Source
    // omitted it (never fabricated). Altitude carries the server-derived trend glyph.
    let glyph = trend_glyph(f.vertical_trend);
    let alt = match f.altitude_ft {
        Some(a) if glyph.is_empty() => format!("{a:.0} ft"),
        Some(a) => format!("{a:.0} ft {glyph}"),
        None => "—".to_string(),
    };
    lines.push(format!("Alt {alt}"));
    lines.push(format!(
        "Spd {}",
        f.groundspeed_kt
            .map(|g| format!("{g:.0} kt"))
            .unwrap_or_else(|| "—".into())
    ));
    lines.push(format!(
        "Trk {}",
        f.track_deg
            .map(|t| format!("{t:03.0}°"))
            .unwrap_or_else(|| "—".into())
    ));
    lines.push(format!(
        "Vr {}",
        f.vertical_rate_fpm
            .map(|r| format!("{r:+.0} fpm"))
            .unwrap_or_else(|| "—".into())
    ));

    // CPA only while in contact and still approaching — a lost flight's CPA is frozen
    // and never paces, so an approach figure would mislead (ADR-0007).
    if f.state == ContactState::InContact {
        if let Some(c) = &f.cpa {
            if c.time_to_cpa_s >= 0.0 {
                lines.push(format!(
                    "CPA {:.1} nm in {:.0}s",
                    c.cpa_distance_nm, c.time_to_cpa_s
                ));
            }
        }
    }

    // Freshness: a lost flight's age is already in its banner; otherwise note how old
    // the underlying data is — the snapshot age when stale, this report's age when live.
    if f.state == ContactState::InContact {
        if resp.health == Health::Stale {
            match resp.snapshot_age_s {
                Some(age) => lines.push(format!("stale — snapshot {age:.0}s old")),
                None => lines.push("stale".to_string()),
            }
        } else {
            lines.push(format!("updated {:.0}s ago", f.age_s));
        }
    }

    lines.join("\n")
}

/// A one-line stale banner for a lost Nearest flight, mirroring the TUI's
/// (ADR-0007): the reason and how long ago contact was lost. `None` while in contact.
fn lost_banner(f: &Flight) -> Option<String> {
    let reason = match f.state {
        ContactState::InContact => return None,
        ContactState::Landed => "landed",
        ContactState::LeftScope => "left the Search area",
        ContactState::LostContact => "contact lost",
    };
    Some(format!("⚠ {reason} — last-known data, {:.0}s ago", f.age_s))
}

/// A display label: the callsign, or the bracketed hex for an ident-blocked flight.
/// Mirrors the TUI and webclient.
fn label(f: &Flight) -> String {
    f.ident.clone().unwrap_or_else(|| format!("[{}]", f.hex))
}

/// The climb/descend/level glyph the Server derived, mirroring the TUI and webclient.
/// Unknown renders blank rather than a fake symbol.
fn trend_glyph(trend: VerticalTrend) -> &'static str {
    match trend {
        VerticalTrend::Climb => "↑",
        VerticalTrend::Descend => "↓",
        VerticalTrend::Level => "–",
        VerticalTrend::Unknown => "",
    }
}

/// Escape the three Pango-markup metacharacters, since Waybar renders both `text`
/// and `tooltip` as Pango markup and we emit no markup of our own. A label is plain
/// alphanumerics, but an operator or model could carry an `&` (e.g. "M & N Aviation")
/// that would otherwise make Pango drop the whole string.
fn pango_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use flights_api::Cpa;

    fn flight(distance_nm: f64) -> Flight {
        Flight {
            hex: "abc123".into(),
            ident: Some("RYR4GH".into()),
            aircraft_type: Some("B738".into()),
            model: Some("BOEING 737-800".into()),
            registration: Some("EI-DWX".into()),
            operator: Some("RYANAIR".into()),
            lat: 0.05,
            lon: 0.0,
            distance_nm,
            bearing_deg: 45.0,
            altitude_ft: Some(37000.0),
            geometric_altitude_ft: None,
            groundspeed_kt: Some(421.0),
            track_deg: Some(197.0),
            vertical_rate_fpm: Some(1200.0),
            vertical_trend: VerticalTrend::Climb,
            squawk: None,
            emergency: None,
            emitter_category: None,
            state: ContactState::InContact,
            age_s: 1.8,
            cpa: Some(Cpa {
                time_to_cpa_s: 97.0,
                cpa_distance_nm: 1.8,
            }),
        }
    }

    fn resp(health: Health, flight: Option<Flight>) -> NearestResponse {
        NearestResponse {
            as_of: 1_780_000_000.0,
            health,
            snapshot_age_s: Some(1.8),
            flight,
        }
    }

    #[test]
    fn empty_sky_collapses_the_module() {
        let bar = build(&resp(Health::Live, None), 35.0);
        assert_eq!(bar, Bar::empty());
        assert!(bar.text.is_empty(), "empty text so Waybar collapses it");
    }

    #[test]
    fn a_flight_beyond_the_display_range_collapses_the_module() {
        // The Server still tracks and answers with it; the bar simply stays quiet.
        let bar = build(&resp(Health::Live, Some(flight(50.0))), 35.0);
        assert_eq!(bar, Bar::empty());
    }

    #[test]
    fn a_flight_within_range_shows_the_rich_bar_line() {
        // `✈ {ident} {type} {dist}nm {brg}° {alt} {trend}` (ADR-0008): bearing 45 → 045°,
        // a climbing 737 at 37000 ft.
        let bar = build(&resp(Health::Live, Some(flight(3.1))), 35.0);
        for needle in ["RYR4GH", "B738", "3.1nm", "045°", "37000ft", "↑"] {
            assert!(
                bar.text.contains(needle),
                "bar text missing {needle:?}: {:?}",
                bar.text
            );
        }
        // In contact and live → the default (empty) class.
        assert_eq!(bar.class, "");
    }

    #[test]
    fn the_bar_line_drops_fields_the_source_omits() {
        // No type, no altitude, unknown trend: the line degrades to ident + distance +
        // bearing rather than showing fabricated or empty slots.
        let mut f = flight(3.1);
        f.aircraft_type = None;
        f.altitude_ft = None;
        f.vertical_trend = VerticalTrend::Unknown;
        let bar = build(&resp(Health::Live, Some(f)), 35.0);
        assert_eq!(
            bar.text, "✈ RYR4GH 3.1nm 045°",
            "degraded line: {:?}",
            bar.text
        );
    }

    #[test]
    fn tooltip_carries_alt_spd_trk_vr() {
        let bar = build(&resp(Health::Live, Some(flight(3.1))), 35.0);
        for needle in ["Alt 37000 ft", "Spd 421 kt", "Trk 197°", "Vr +1200 fpm"] {
            assert!(
                bar.tooltip.contains(needle),
                "tooltip missing {needle:?}: {:?}",
                bar.tooltip
            );
        }
        assert!(bar.tooltip.contains("CPA 1.8 nm in 97s"));
    }

    #[test]
    fn a_lost_nearest_is_badged_not_dropped() {
        let mut f = flight(3.1);
        f.state = ContactState::LostContact;
        f.age_s = 45.0;
        let bar = build(&resp(Health::Live, Some(f)), 35.0);
        // Still shown (it's the closest thing we know of) and classed lost.
        assert!(bar.text.contains("RYR4GH"));
        assert_eq!(bar.class, "lost");
        assert!(
            bar.tooltip.contains("contact lost"),
            "stale banner: {:?}",
            bar.tooltip
        );
    }

    #[test]
    fn a_lost_flight_shows_no_live_cpa() {
        let mut f = flight(3.1);
        f.state = ContactState::LeftScope;
        let bar = build(&resp(Health::Live, Some(f)), 35.0);
        assert!(
            !bar.tooltip.contains("CPA"),
            "frozen CPA must not read as live: {:?}",
            bar.tooltip
        );
        assert!(bar.tooltip.contains("left the Search area"));
    }

    #[test]
    fn a_stale_picture_classes_stale_over_lost() {
        // Stale outranks a per-flight badge: the whole Picture is the bigger doubt.
        let mut f = flight(3.1);
        f.state = ContactState::LostContact;
        let bar = build(&resp(Health::Stale, Some(f)), 35.0);
        assert_eq!(bar.class, "stale");
    }

    #[test]
    fn no_data_is_an_error_stub_not_an_empty_sky() {
        let bar = build(&resp(Health::NoData, None), 35.0);
        assert_eq!(bar.class, "error");
        assert!(
            !bar.text.is_empty(),
            "the stub is visible, unlike an empty sky"
        );
    }

    #[test]
    fn an_unreachable_server_renders_the_error_stub() {
        let outcome = Err(ClientError::Unreachable("connection refused".into()));
        let bar = render(&outcome, 35.0);
        assert_eq!(bar.class, "error");
        assert!(bar.tooltip.contains("connection refused"));
    }

    #[test]
    fn an_anonymous_flight_falls_back_to_its_hex() {
        let mut f = flight(3.1);
        f.ident = None;
        let bar = build(&resp(Health::Live, Some(f)), 35.0);
        assert!(
            bar.text.contains("[abc123]"),
            "bracketed hex: {:?}",
            bar.text
        );
    }

    #[test]
    fn pango_metacharacters_are_escaped() {
        let mut f = flight(3.1);
        f.operator = Some("M & N <Air>".into());
        let bar = build(&resp(Health::Live, Some(f)), 35.0);
        assert!(
            bar.tooltip.contains("M &amp; N &lt;Air&gt;"),
            "escaped: {:?}",
            bar.tooltip
        );
        assert!(
            !bar.tooltip.contains("M & N"),
            "no bare ampersand reaches Pango"
        );
    }

    #[test]
    fn the_bar_serializes_to_the_three_waybar_fields() {
        let bar = build(&resp(Health::Live, Some(flight(3.1))), 35.0);
        let v: serde_json::Value = serde_json::to_value(&bar).unwrap();
        assert!(v["text"].is_string());
        assert!(v["tooltip"].is_string());
        assert!(v["class"].is_string());
    }
}
