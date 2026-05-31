//! The right column: the flight **list** (nearest-first, the order the Server
//! sends) and the **status** block. Both read the same Picture the radar does, so
//! the three never disagree (ADR-0005). Clicking a row opens that flight's detail;
//! the status block mirrors the TUI's — source, health, counts, the Nearest and
//! Pacing flights, and the Relevance distance.

use leptos::prelude::*;

use flights_api::{Flight, Health, Meta, PictureResponse};

use crate::app::Conn;
use crate::display::*;

#[component]
pub fn FlightList(
    picture: RwSignal<Option<PictureResponse>>,
    selected: RwSignal<Option<String>>,
    open: Callback<String>,
) -> impl IntoView {
    let count = move || {
        picture
            .get()
            .map(|p| p.tracks.len())
            .unwrap_or(0)
    };
    let rows = move || {
        let pic = picture.get();
        let tracks = pic
            .as_ref()
            .map(|p| p.tracks.clone())
            .unwrap_or_default();
        let pacing = pic.as_ref().and_then(|p| p.pacing_hex.clone());
        let sel = selected.get();
        tracks
            .into_iter()
            .map(|t| {
                let is_sel = sel.as_deref() == Some(t.hex.as_str());
                let is_pacing = pacing.as_deref() == Some(t.hex.as_str());
                flight_row(t, is_sel, is_pacing, open)
            })
            .collect_view()
    };

    view! {
        <div class="panel list-pane">
            <div class="panel-title">{move || format!("flights · {}", count())}</div>
            <div class="list-body">{rows}</div>
        </div>
    }
}

/// One list row. A click opens the flight's detail (and selects it). Mirrors the
/// TUI row's columns: callsign, type, distance/bearing, altitude + trend glyph,
/// groundspeed, then the CPA arrow and figures.
fn flight_row(t: Flight, is_sel: bool, is_pacing: bool, open: Callback<String>) -> impl IntoView {
    let hex = t.hex.clone();
    let name = label(&t);
    let kind = t.aircraft_type.clone().unwrap_or_default();
    let pos = format!("{:.1}nm {:03.0}°", t.distance_nm, t.bearing_deg);
    let alt = t
        .altitude_ft
        .map(|a| format!("{a:.0}ft"))
        .unwrap_or_else(|| "?".into());
    let (glyph, glyph_color) = trend_glyph(t.vertical_trend);
    let gs = t
        .groundspeed_kt
        .map(|g| format!("{g:.0}kt"))
        .unwrap_or_default();
    let (arrow, arrow_color, cpa) = match &t.cpa {
        Some(c) if approaching(c) => (
            "▲",
            COL_APPROACH,
            format!("{:.0}nm/{:.0}s", c.cpa_distance_nm, c.time_to_cpa_s),
        ),
        Some(_) => ("▼", COL_RECEDE, String::new()),
        None => ("·", COL_ANON, String::new()),
    };

    let ident_class = if is_pacing {
        "cell ident pacing"
    } else if t.ident.is_none() {
        "cell ident anon"
    } else {
        "cell ident"
    };
    let row_class = if is_sel {
        "flight-row selected"
    } else {
        "flight-row"
    };

    view! {
        <div class=row_class on:click=move |_| open.run(hex.clone())>
            <span class=ident_class>{name}</span>
            <span class="cell kind">{kind}</span>
            <span class="cell pos">{pos}</span>
            <span class="cell alt">
                {alt}
                <span class="glyph" style=format!("color:{glyph_color}")>{glyph}</span>
            </span>
            <span class="cell gs">{gs}</span>
            <span class="cell cpa">
                <span class="arrow" style=format!("color:{arrow_color}")>{arrow}</span>
                " "
                {cpa}
            </span>
        </div>
    }
}

#[component]
pub fn Status(
    meta: RwSignal<Option<Meta>>,
    picture: RwSignal<Option<PictureResponse>>,
    conn: RwSignal<Conn>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let body = move || {
        // The "server unreachable" state takes precedence over the Server's own
        // health (which we can't trust once we can't reach it).
        let (state_label, state_color, error) = match conn.get() {
            Conn::Down(e) => ("UNREACHABLE", COL_PACING, Some(e)),
            Conn::Connecting => ("CONNECTING", COL_HOME, None),
            Conn::Ok => match picture.get() {
                Some(p) => match p.health {
                    Health::Live => ("LIVE", COL_APPROACH, None),
                    Health::Stale => ("STALE", COL_HOME, p.last_error.clone()),
                    Health::NoData => ("NO DATA", COL_PACING, p.last_error.clone()),
                },
                None => ("NO DATA", COL_PACING, None),
            },
        };

        let source = meta
            .get()
            .map(|m| m.source)
            .unwrap_or_else(|| "—".into());
        let relevance = meta
            .get()
            .map(|m| format!("{:.0}nm", m.relevance_nm))
            .unwrap_or_else(|| "—".into());

        let pic = picture.get();
        let tracks = pic
            .as_ref()
            .map(|p| p.tracks.clone())
            .unwrap_or_default();
        let count = tracks.len();
        let age = pic
            .as_ref()
            .and_then(|p| p.snapshot_age_s)
            .map(|s| format!("{s:.0}s ago"))
            .unwrap_or_else(|| "—".into());
        let nearest = tracks
            .first()
            .map(fmt_position)
            .unwrap_or_else(|| "(none)".into());
        let pacing_hex = pic.as_ref().and_then(|p| p.pacing_hex.clone());
        let pacing = pacing_hex
            .as_deref()
            .and_then(|h| tracks.iter().find(|t| t.hex == h))
            .map(fmt_pacing)
            .unwrap_or_else(|| "quiet — backing off".into());

        let selected_line = selected
            .get()
            .as_deref()
            .and_then(|h| tracks.iter().find(|t| t.hex == h))
            .map(|t| {
                let model = t
                    .model
                    .as_deref()
                    .or(t.aircraft_type.as_deref())
                    .unwrap_or("unknown type");
                format!(
                    "sel {} · {model}  {:.3}, {:.3}  ({:.0}s old)",
                    label(t),
                    t.lat,
                    t.lon,
                    t.age_s
                )
            });

        let last_line = match selected_line {
            Some(s) => view! { <div class="status-line sel">{s}</div> }.into_any(),
            None => view! {
                <div class="status-line hint">
                    "click a flight · ↑/↓ select · Enter detail · Esc clear"
                </div>
            }
            .into_any(),
        };

        view! {
            <div class="status-line">
                "source " <b>{source}</b> "  "
                <span class="state" style=format!("color:{state_color}")>
                    {format!("[{state_label}]")}
                </span>
                {format!("  {count} flights · {age}")}
            </div>
            <div class="status-line">{format!("nearest  {nearest}")}</div>
            <div class="status-line">{format!("pacing   {pacing}")}</div>
            <div class="status-line">{format!("relevance {relevance}")}</div>
            {last_line}
            {error.map(|e| view! { <div class="status-line err">{format!("! {e}")}</div> })}
        }
    };

    view! {
        <div class="panel status-pane">
            <div class="panel-title">"status"</div>
            <div class="status-body">{body}</div>
        </div>
    }
}
