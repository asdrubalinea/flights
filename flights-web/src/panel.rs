//! The right column — the **MCDU console** (ADR-0009). One panel that owns the
//! AIRSPACE list, the per-flight DETAIL page, a persistent status strip, and the
//! scratchpad. The body switches on [`Mode`]; the strip and scratchpad sit *outside*
//! that switch, so live traffic/health are never hidden behind a page flip. It reads
//! the same Picture the scope does, so the two never disagree (ADR-0005).

use leptos::prelude::*;

use flights_api::{Flight, Health, Meta, PictureResponse};

use crate::app::{Conn, DetailView, Mode};
use crate::detail::detail_page;
use crate::display::*;

#[component]
pub fn Console(
    meta: RwSignal<Option<Meta>>,
    picture: RwSignal<Option<PictureResponse>>,
    conn: RwSignal<Conn>,
    selected: RwSignal<Option<String>>,
    mode: RwSignal<Mode>,
    detail: RwSignal<Option<DetailView>>,
    open: Callback<String>,
    close: Callback<()>,
) -> impl IntoView {
    // Title: page name + counter, with the connection/health badge on the right.
    let title = move || {
        let (name, page) = match mode.get() {
            Mode::Radar => ("AIRSPACE", 1),
            Mode::Detail => ("DETAIL", 2),
        };
        let (label, class) = conn_state(conn.get(), picture.get());
        view! {
            <span>{format!("{name}  {page}/2")}</span>
            <span class=format!("state {class}")>{format!("[{label}]")}</span>
        }
    };

    // The body switches on `Mode` *only* — each page carries its own reactive closure
    // (the list re-rows on each poll, the detail re-renders on each `/flight`), so a
    // poll repaints the inner content without rebuilding the console chrome.
    let body = move || match mode.get() {
        Mode::Radar => airspace_page(picture, selected, open),
        Mode::Detail => detail_page(picture, selected, detail, close),
    };

    // Persistent status strip: Nearest / Pacing / Relevance + Snapshot age.
    let strip = move || {
        let pic = picture.get();
        let tracks = pic.as_ref().map(|p| p.tracks.clone()).unwrap_or_default();
        let pacing_hex = pic.as_ref().and_then(|p| p.pacing_hex.clone());
        let nearest = tracks
            .first()
            .map(fmt_position)
            .unwrap_or_else(|| "(none)".into());
        let pacing = pacing_hex
            .as_deref()
            .and_then(|h| tracks.iter().find(|t| t.hex == h))
            .map(fmt_pacing)
            .unwrap_or_else(|| "quiet — backing off".into());
        let relevance = meta
            .get()
            .map(|m| format!("{:.0}nm", m.relevance_nm))
            .unwrap_or_else(|| "—".into());
        let age = pic
            .as_ref()
            .and_then(|p| p.snapshot_age_s)
            .map(|s| format!("{s:.0}s"))
            .unwrap_or_else(|| "—".into());
        view! {
            <div class="strip-line"><span class="k">"N  "</span>{nearest}</div>
            <div class="strip-line"><span class="k">"P  "</span>{pacing}</div>
            <div class="strip-line">
                <span class="k">"REL"</span>{format!(" {relevance}  ·  {age} old")}
            </div>
        }
    };

    // Scratchpad: a live error (amber), then the selected flight, else the key hint.
    let scratch = move || {
        let error = match conn.get() {
            Conn::Down(e) => Some(e),
            _ => picture.with(|p| {
                p.as_ref().and_then(|p| match p.health {
                    Health::Live => None,
                    _ => p.last_error.clone(),
                })
            }),
        };
        let sel = selected.get().and_then(|h| {
            picture.with(|p| {
                p.as_ref().and_then(|p| {
                    p.tracks.iter().find(|t| t.hex == h).map(|t| {
                        let kind = t.aircraft_type.as_deref().unwrap_or("?");
                        format!("SEL {} · {kind} · {:.0}s", label(t), t.age_s)
                    })
                })
            })
        });
        let (text, class) = scratchpad_line(error.as_deref(), sel.as_deref());
        view! { <div class=format!("scratchpad {class}")>{text}</div> }
    };

    view! {
        <div class="panel console">
            <div class="panel-title console-title">{title}</div>
            <div class="console-body">{body}</div>
            <div class="status-strip">{strip}</div>
            {scratch}
        </div>
    }
}

/// The AIRSPACE page: the nearest-first flight list, each row a line-select key
/// (LSK) that opens that flight's DETAIL. Built as a free function with its own
/// reactive `rows` closure, so the console's body switch only re-runs on `Mode`.
fn airspace_page(
    picture: RwSignal<Option<PictureResponse>>,
    selected: RwSignal<Option<String>>,
    open: Callback<String>,
) -> AnyView {
    let rows = move || {
        let pic = picture.get();
        let tracks = pic.as_ref().map(|p| p.tracks.clone()).unwrap_or_default();
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
    view! { <div class="list-body">{rows}</div> }.into_any()
}

/// One LSK row. A click opens the flight's DETAIL (and selects it). Columns mirror
/// the TUI: the `<` line-select marker, callsign, type, distance/bearing, altitude +
/// trend glyph, groundspeed, then the CPA arrow and figures. Lost/emergency rows pick
/// up the same caution/warning colours the scope uses (ADR-0009).
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
    // Row class echoes the scope's role colours: warning (emergency) ▸ caution (lost).
    let row_class = {
        let mut c = String::from("flight-row");
        if is_sel {
            c.push_str(" selected");
        }
        if t.emergency.is_some() {
            c.push_str(" emer");
        } else if t.state != flights_api::ContactState::InContact {
            c.push_str(" lost");
        }
        c
    };

    view! {
        <div class=row_class on:click=move |_| open.run(hex.clone())>
            <span class="cell lsk">"<"</span>
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

/// The ECAM-style master warning: a red, blinking banner shown whenever any track is
/// broadcasting an emergency (ADR-0009). Keyed off the Server's `emergency` field via
/// [`any_emergency`], never the client interpreting squawk codes (CONTEXT.md,
/// *Squawk*). Renders nothing when the airspace is calm.
#[component]
pub fn MasterWarning(picture: RwSignal<Option<PictureResponse>>) -> impl IntoView {
    move || {
        picture.with(|p| {
            p.as_ref().and_then(|p| {
                any_emergency(&p.tracks).map(|f| {
                    let sq = f
                        .squawk
                        .clone()
                        .map(|s| format!(" · {s}"))
                        .unwrap_or_default();
                    let kind = f.emergency.clone().unwrap_or_default();
                    view! {
                        <div class="master-warning">
                            {format!("⚠ EMERGENCY · {} · {kind}{sq}", label(f))}
                        </div>
                    }
                })
            })
        })
    }
}

/// The connection/health badge text + CSS class for the title bar. "Server
/// unreachable" takes precedence over the Server's own health (which we can't trust
/// once we can't reach it).
fn conn_state(conn: Conn, pic: Option<PictureResponse>) -> (&'static str, &'static str) {
    match conn {
        Conn::Down(_) => ("UNREACHABLE", "warn"),
        Conn::Connecting => ("CONNECTING", "info"),
        Conn::Ok => match pic.map(|p| p.health) {
            Some(Health::Live) => ("LIVE", "ok"),
            Some(Health::Stale) => ("STALE", "caution"),
            _ => ("NO DATA", "warn"),
        },
    }
}
