//! The in-console DETAIL page (ADR-0009 — no floating modal). A purpose-built Airbus
//! glass-cockpit **instrument cluster** for the selected flight, built only from data
//! ADS-B carries (ALT/VS/GS/TRK/FL, position, CPA, squawk) — no artificial horizon,
//! airspeed, or engine pages, because we have no attitude, airspeed, or systems data
//! (CONTEXT.md, *Observed kinematics*). The gauges read the *live* Picture track so
//! they update each poll; the opaque, Server-formatted [`DetailGroup`]s (ADR-0004)
//! come from the one-shot `/flight` fetch. A lost flight's gauges are flagged STALE
//! (amber), never fabricated; once it leaves the Picture the page says so.

use leptos::prelude::*;

use flights_api::{ContactState, Flight, FlightDetail, PictureResponse};

use crate::app::DetailView;
use crate::display::{flight_level, label, trend_glyph};

/// The DETAIL page: a live instrument cluster (from the Picture track, so it updates
/// each poll) above the opaque detail groups (from the one-shot `/flight` fetch). The
/// gauges' closure tracks `picture`/`selected` and the groups' closure tracks
/// `detail`, so the console body only re-runs on a `Mode` change.
pub fn detail_page(
    picture: RwSignal<Option<PictureResponse>>,
    selected: RwSignal<Option<String>>,
    detail: RwSignal<Option<DetailView>>,
    close: Callback<()>,
) -> AnyView {
    // The selected flight's *live* track from the current Picture (re-derived each
    // poll, so the gauges update). `None` once it has left the Picture.
    let live = move || {
        selected.get().and_then(|h| {
            picture.with(|p| {
                p.as_ref()
                    .and_then(|p| p.tracks.iter().find(|t| t.hex == h).cloned())
            })
        })
    };

    let gauges = move || match live() {
        Some(f) => instrument_cluster(&f),
        None => view! { <div class="d-notice">"— flight left the area —"</div> }.into_any(),
    };
    let groups = move || match detail.get() {
        Some(DetailView::Loaded(d)) => identity_groups(&d),
        Some(DetailView::Error(e)) => notice(&format!("— detail unavailable: {e} —")),
        Some(DetailView::Loading) | None => notice("— loading detail… —"),
        Some(DetailView::LeftArea) => Vec::new(), // the cluster already says so
    };

    view! {
        <div class="detail-page">
            <div class="detail-body">
                {gauges}
                {groups}
            </div>
            <button class="lsk-back" on:click=move |_| close.run(())>"◂ RADAR"</button>
        </div>
    }
    .into_any()
}

/// The instrument cluster — GS / ALT+FL / V/S / TRK gauges, then position, CPA, geo
/// altitude, and transponder lines. Only fields ADS-B carries; a lost track is
/// flagged STALE (amber) rather than faked (CONTEXT.md, *Observed kinematics*).
fn instrument_cluster(f: &Flight) -> AnyView {
    let stale = f.state != ContactState::InContact;
    let gauge_class = if stale { "gauge stale" } else { "gauge" };

    let gs = f
        .groundspeed_kt
        .map(|g| format!("{g:.0}"))
        .unwrap_or_else(|| "---".into());
    let alt = f
        .altitude_ft
        .map(|a| format!("{a:.0}"))
        .unwrap_or_else(|| "-----".into());
    let fl = f.altitude_ft.map(flight_level).unwrap_or_default();
    let vs = f
        .vertical_rate_fpm
        .map(|v| format!("{v:+.0}"))
        .unwrap_or_else(|| "----".into());
    let (vglyph, _) = trend_glyph(f.vertical_trend);
    let trk = f
        .track_deg
        .map(|t| format!("{t:03.0}"))
        .unwrap_or_else(|| "---".into());

    let pos = format!("{:.1} nm  {:03.0}°", f.distance_nm, f.bearing_deg);
    let geo = f
        .geometric_altitude_ft
        .map(|a| format!("{a:.0} ft"))
        .unwrap_or_else(|| "—".into());
    let cpa = match &f.cpa {
        Some(c) if c.time_to_cpa_s >= 0.0 => {
            format!("{:.1} nm in {:.0}s", c.cpa_distance_nm, c.time_to_cpa_s)
        }
        Some(_) => "receding".into(),
        None => "—".into(),
    };
    let squawk = f.squawk.clone().unwrap_or_else(|| "----".into());
    let kind = f
        .aircraft_type
        .clone()
        .or_else(|| f.model.clone())
        .unwrap_or_default();

    view! {
        <div class="cluster">
            <div class="cluster-head">
                <span class="ci">{label(f)}</span>
                <span class="ck">{kind}</span>
                {stale.then(|| view! { <span class="cstale">"STALE"</span> })}
            </div>
            <div class="gauges">
                <div class=gauge_class>
                    <div class="glabel">"GS"</div>
                    <div class="gval">{gs}</div>
                    <div class="gunit">"kt"</div>
                </div>
                <div class=gauge_class>
                    <div class="glabel">"ALT"</div>
                    <div class="gval">{alt}</div>
                    <div class="gunit">{fl}</div>
                </div>
                <div class=gauge_class>
                    <div class="glabel">"V/S"</div>
                    <div class="gval">{vs}</div>
                    <div class="gunit">{format!("fpm {vglyph}")}</div>
                </div>
                <div class=gauge_class>
                    <div class="glabel">"TRK"</div>
                    <div class="gval">{trk}</div>
                    <div class="gunit">"°T"</div>
                </div>
            </div>
            <div class="gauge-lines">
                <div class="gline"><span class="k">"POS  "</span>{pos}</div>
                <div class="gline"><span class="k">"CPA  "</span>{cpa}</div>
                <div class="gline"><span class="k">"GEO  "</span>{geo}</div>
                <div class="gline">
                    <span class="k">"XPDR "</span>
                    {squawk}
                    {f
                        .emergency
                        .as_ref()
                        .map(|e| view! { <span class="cemer">{format!("  EMER {e}")}</span> })}
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// Identity (registration / operator / category) followed by the opaque,
/// Server-formatted detail groups, rendered verbatim (ADR-0004).
fn identity_groups(d: &FlightDetail) -> Vec<AnyView> {
    let f = &d.flight;
    let mut out: Vec<AnyView> = Vec::new();

    if f.registration.is_some() || f.operator.is_some() || f.emitter_category.is_some() {
        out.push(section("Identity"));
        if let Some(r) = &f.registration {
            out.push(field("Registration", r));
        }
        if let Some(o) = &f.operator {
            out.push(field("Operator", o));
        }
        if let Some(c) = &f.emitter_category {
            out.push(field("Category", c));
        }
    }

    for group in &d.details {
        out.push(section(&group.title));
        for fld in &group.fields {
            out.push(field(&fld.label, &fld.value));
        }
    }

    out
}

fn field(label: &str, value: &str) -> AnyView {
    let (label, value) = (label.to_string(), value.to_string());
    view! {
        <div class="d-field">
            <span class="d-label">{label}</span>
            <span class="d-value">{value}</span>
        </div>
    }
    .into_any()
}

fn section(title: &str) -> AnyView {
    let title = title.to_string();
    view! { <div class="d-section">{title}</div> }.into_any()
}

fn notice(text: &str) -> Vec<AnyView> {
    let text = text.to_string();
    vec![view! { <div class="d-notice">{text}</div> }.into_any()]
}
