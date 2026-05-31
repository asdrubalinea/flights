//! The webclient's root component and the glue that drives it. As a thin Client
//! (ADR-0005) it holds **no engine** — no tracker, no source, no geometry. It keeps
//! the latest [`PictureResponse`] polled from the Server, the static [`Meta`], the
//! connection state, the selection (held by aircraft `hex`, so the highlight stays
//! on the same aircraft as the list re-sorts each poll), and the detail popup.
//!
//! The side effects live here: `/picture` every frame (and `/meta` until it loads)
//! in [`App`]'s poll, and `/flight/{hex}` when the popup opens. The radar, list,
//! and status are pure functions of the signals these set, so they can never
//! disagree — they read one Picture (ADR-0005).

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;

use flights_api::{FlightDetail, Meta, PictureResponse};

use crate::client::ApiClient;
use crate::config::WebConfig;
use crate::detail::DetailOverlay;
use crate::panel::{FlightList, Status};
use crate::radar::Radar;

/// Whether the last `/picture` poll reached the Server. `Down` is the "server
/// unreachable" state — which over CORS also covers a reachable Server that never
/// allowed this origin (ADR-0007). `Connecting` is the pre-first-poll state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    Connecting,
    Ok,
    Down(String),
}

/// Which view is foremost. The radar/list is always drawn; in `Detail` the popup
/// is layered on top and a few keys change meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Radar,
    Detail,
}

/// What the detail popup currently has for the selected flight: still fetching,
/// its full detail, a "left the area" notice once it drops out of the Picture or
/// 404s, or an error if the fetch failed.
#[derive(Clone)]
pub enum DetailView {
    Loading,
    Loaded(Box<FlightDetail>),
    LeftArea,
    Error(String),
}

#[component]
pub fn App() -> impl IntoView {
    let cfg = WebConfig::from_location();
    let frame = Duration::from_millis(u64::from(cfg.frame_ms()));
    let client = ApiClient::new(cfg.server_url.clone());

    let meta = RwSignal::new(None::<Meta>);
    let picture = RwSignal::new(None::<PictureResponse>);
    let conn = RwSignal::new(Conn::Connecting);
    let selected = RwSignal::new(None::<String>);
    let mode = RwSignal::new(Mode::Radar);
    let detail = RwSignal::new(None::<DetailView>);

    // Open a flight's detail popup: select it, switch to Detail mode, and fetch
    // `/flight/{hex}`. Shared by row clicks and the keyboard (Enter / arrow-nav).
    let open = {
        let client = client.clone();
        Callback::new(move |hex: String| {
            selected.set(Some(hex.clone()));
            mode.set(Mode::Detail);
            detail.set(Some(DetailView::Loading));
            let client = client.clone();
            spawn_local(async move {
                let view = match client.flight(&hex).await {
                    Ok(Some(d)) => DetailView::Loaded(Box::new(d)),
                    Ok(None) => DetailView::LeftArea,
                    Err(e) => DetailView::Error(e.to_string()),
                };
                // Apply this response only if the selection hasn't moved on while
                // the fetch was in flight — otherwise a slower earlier `/flight`
                // could overwrite the detail of a flight we've since switched to.
                if selected.get_untracked().as_deref() == Some(hex.as_str()) {
                    detail.set(Some(view));
                }
            });
        })
    };

    let close = Callback::new(move |()| mode.set(Mode::Radar));

    // Poll `/picture` every frame; lazily fetch `/meta` until it loads (so the page
    // opened before the Server still connects, retrying each tick with no sleep).
    {
        let client = client.clone();
        let poll = move || {
            let client = client.clone();
            spawn_local(async move {
                if meta.with_untracked(|m| m.is_none()) {
                    if let Ok(m) = client.meta().await {
                        meta.set(Some(m));
                    }
                }
                match client.picture().await {
                    Ok(p) => {
                        conn.set(Conn::Ok);
                        // If the open popup's flight has left the Picture, flag it
                        // from data already in hand — no extra fetch (mirrors the TUI).
                        if mode.get_untracked() == Mode::Detail {
                            if let Some(hex) = selected.get_untracked() {
                                if !p.tracks.iter().any(|t| t.hex == hex) {
                                    detail.set(Some(DetailView::LeftArea));
                                }
                            }
                        }
                        picture.set(Some(p));
                    }
                    Err(e) => conn.set(Conn::Down(e.to_string())),
                }
            });
        };
        poll(); // fetch immediately rather than waiting a whole frame
        set_interval(poll, frame);
    }

    // Keyboard parity with the TUI radar: arrows/jk step the selection, Enter opens
    // the popup, Esc closes it (or clears the selection on the radar).
    {
        let step = move |forward: bool| {
            let hexes: Vec<String> = picture
                .with_untracked(|p| p.as_ref().map(|p| p.tracks.iter().map(|t| t.hex.clone()).collect()))
                .unwrap_or_default();
            if hexes.is_empty() {
                selected.set(None);
                return;
            }
            let current = selected
                .get_untracked()
                .and_then(|h| hexes.iter().position(|x| *x == h));
            let next = hexes[next_index(current, hexes.len(), forward)].clone();
            let changed = selected.get_untracked().as_deref() != Some(next.as_str());
            selected.set(Some(next.clone()));
            // In Detail mode, stepping re-opens the popup on the new flight (re-fetch).
            if changed && mode.get_untracked() == Mode::Detail {
                open.run(next);
            }
        };
        let open_selected = move || {
            let hex = selected.get_untracked().or_else(|| {
                picture.with_untracked(|p| {
                    p.as_ref().and_then(|p| p.tracks.first().map(|t| t.hex.clone()))
                })
            });
            if let Some(hex) = hex {
                open.run(hex);
            }
        };
        let handle = window_event_listener(leptos::ev::keydown, move |ev| {
            match ev.key().as_str() {
                "ArrowDown" | "j" => step(true),
                "ArrowUp" | "k" => step(false),
                "Enter" if mode.get_untracked() == Mode::Radar => open_selected(),
                "Escape" => {
                    if mode.get_untracked() == Mode::Detail {
                        mode.set(Mode::Radar);
                    } else {
                        selected.set(None);
                    }
                }
                _ => {}
            }
        });
        // The listener lives for the whole app; keep it from being removed. (The
        // handle removes the listener only via `.remove()`, but leaking is the
        // clearest statement of "this is permanent".)
        std::mem::forget(handle);
    }

    view! {
        <div class="app">
            <Radar meta=meta picture=picture selected=selected />
            <div class="side">
                <FlightList picture=picture selected=selected open=open />
                <Status meta=meta picture=picture conn=conn selected=selected />
            </div>
            <DetailOverlay mode=mode detail=detail close=close />
        </div>
    }
}

/// The wrapping next index for a selection step, mirroring the TUI. Pure, so the
/// behaviour is obvious without a browser.
fn next_index(current: Option<usize>, n: usize, forward: bool) -> usize {
    match (current, forward) {
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    }
}
