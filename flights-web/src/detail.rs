//! The flight-detail overlay: a modal that renders the lazily-fetched
//! [`FlightDetail`] for the selected flight — the promoted typed fields grouped
//! under "Position & motion" / "Transponder", then the Server-contributed opaque
//! [`DetailGroup`]s rendered verbatim (ADR-0004), exactly as the TUI popup does.
//! Shows "flight left the area" once the flight drops out of the Picture or the
//! fetch 404s, and an error notice if the fetch failed — never fabricated values.

use leptos::prelude::*;

use flights_api::{FlightDetail, VerticalTrend};

use crate::app::{DetailView, Mode};
use crate::display::{label, trend_glyph};

#[component]
pub fn DetailOverlay(
    mode: RwSignal<Mode>,
    detail: RwSignal<Option<DetailView>>,
    close: Callback<()>,
) -> impl IntoView {
    move || {
        if mode.get() != Mode::Detail {
            return None;
        }
        let body = match detail.get() {
            Some(DetailView::Loaded(d)) => detail_body(&d),
            Some(DetailView::LeftArea) => notice("— flight left the area —"),
            Some(DetailView::Error(e)) => notice(&format!("— {e} —")),
            _ => notice("— loading… —"),
        };
        Some(view! {
            // Click the dimmed backdrop to close; clicks inside the modal don't
            // bubble up to it.
            <div class="overlay" on:click=move |_| close.run(())>
                <div class="modal" on:click=move |ev| ev.stop_propagation()>
                    <div class="modal-title">
                        <span>"flight detail"</span>
                        <button class="modal-close" on:click=move |_| close.run(())>"×"</button>
                    </div>
                    <div class="modal-body">{body}</div>
                    <div class="modal-footer">"Esc close · ↑/↓ flight"</div>
                </div>
            </div>
        })
    }
}

/// Every displayable field for one flight: a header, the promoted typed fields
/// grouped under section headings, then the opaque Server-formatted groups.
fn detail_body(d: &FlightDetail) -> Vec<AnyView> {
    let f = &d.flight;
    let mut out: Vec<AnyView> = Vec::new();

    out.push(view! { <div class="d-header">{label(f)}</div> }.into_any());

    if let Some(r) = &f.registration {
        out.push(field("Registration", r));
    }
    if let Some(o) = &f.operator {
        out.push(field("Operator", o));
    }
    let kind = match (&f.aircraft_type, &f.model) {
        (Some(ty), Some(m)) => Some(format!("{ty} · {m}")),
        (Some(ty), None) => Some(ty.clone()),
        (None, Some(m)) => Some(m.clone()),
        (None, None) => None,
    };
    if let Some(k) = kind {
        out.push(field("Type", &k));
    }
    if let Some(c) = &f.emitter_category {
        out.push(field("Category", c));
    }

    out.push(section("Position & motion"));
    out.push(field("Distance", &format!("{:.1} nm", f.distance_nm)));
    out.push(field("Bearing", &format!("{:03.0}°", f.bearing_deg)));
    if let Some(a) = f.altitude_ft {
        out.push(field("Baro alt", &format!("{a:.0} ft")));
    }
    if let Some(a) = f.geometric_altitude_ft {
        out.push(field("Geo alt", &format!("{a:.0} ft")));
    }
    if let Some(r) = f.vertical_rate_fpm {
        let (glyph, _) = trend_glyph(f.vertical_trend);
        out.push(field("Vertical rate", &format!("{r:+.0} fpm {glyph}")));
    } else if f.vertical_trend != VerticalTrend::Unknown {
        let (glyph, _) = trend_glyph(f.vertical_trend);
        out.push(field("Vertical trend", glyph));
    }
    if let Some(g) = f.groundspeed_kt {
        out.push(field("Groundspeed", &format!("{g:.0} kt")));
    }
    if let Some(tr) = f.track_deg {
        out.push(field("Track", &format!("{tr:03.0}°")));
    }
    out.push(field("Position", &format!("{:.4}, {:.4}", f.lat, f.lon)));
    out.push(field("Position age", &format!("{:.0}s", f.age_s)));

    if f.squawk.is_some() || f.emergency.is_some() {
        out.push(section("Transponder"));
        if let Some(s) = &f.squawk {
            out.push(field("Squawk", s));
        }
        if let Some(e) = &f.emergency {
            out.push(field("Emergency", e));
        }
    }

    // The opaque, Server-formatted detail groups — rendered verbatim.
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
