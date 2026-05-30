//! UI state and the logic behind each [`super::event::Action`]. As a thin Client
//! (ADR-0005) the `App` holds **no engine** — no tracker, no source, no geometry.
//! It keeps the latest [`PictureResponse`] fetched from the Server, the static
//! [`Meta`], the connection state, and the selection (held by aircraft `hex`, so
//! the highlight stays on the same aircraft as the list re-sorts each frame).
//!
//! The side-effecting calls — `/picture` every frame ([`App::refresh`]) and
//! `/flight/{hex}` when the popup needs it ([`App::sync_detail`]) — live here but
//! are driven by the event loop in [`super`]. Pure selection math is factored into
//! [`next_index`] so it can be unit-tested without a Server.

use ratatui::widgets::ListState;

use flights_api::{Flight, FlightDetail, Meta, PictureResponse};

use crate::client::Client;

/// Which screen the TUI is showing. The radar/list is always drawn; in `Detail`
/// the flight-detail popup is layered on top and the keys change meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Radar,
    Detail,
}

/// Whether the last `/picture` poll reached the Server. `Down` is the TUI's
/// "server unreachable" state, shown beside the Server's own `stale`/`no_data`.
#[derive(Debug, Clone)]
pub enum Conn {
    Ok,
    Down(String),
}

/// What the detail popup currently has for the selected flight: its full detail,
/// a "left the area" notice once it drops out of the picture or 404s, or an error
/// if the fetch failed.
#[derive(Debug, Clone)]
pub enum DetailView {
    Loaded(Box<FlightDetail>),
    LeftArea,
    Error(String),
}

pub struct App {
    client: Client,
    pub meta: Meta,
    /// One frame's duration — the input timeout and redraw cadence.
    pub fps_interval: std::time::Duration,

    pub running: bool,
    pub mode: Mode,
    /// Latest airspace picture from the Server, or `None` before the first poll.
    pub picture: Option<PictureResponse>,
    /// Whether the Server is currently reachable.
    pub conn: Conn,
    /// Selected aircraft by stable `hex`, or `None` for no selection.
    pub selected_hex: Option<String>,
    /// The popup body for the current selection (fetched lazily; see
    /// [`App::sync_detail`]).
    pub detail: Option<DetailView>,
    /// Set when the selection changed in Detail mode and the popup needs a fresh
    /// `/flight/{hex}` fetch.
    detail_pending: bool,
    /// Vertical scroll offset (lines) of the detail popup body.
    pub detail_scroll: u16,
    /// Scroll/selection state for the flight list widget.
    pub list_state: ListState,
}

impl App {
    pub fn new(client: Client, meta: Meta, fps_interval: std::time::Duration) -> Self {
        Self {
            client,
            meta,
            fps_interval,
            running: true,
            mode: Mode::Radar,
            picture: None,
            conn: Conn::Ok,
            selected_hex: None,
            detail: None,
            detail_pending: false,
            detail_scroll: 0,
            list_state: ListState::default(),
        }
    }

    /// The current tracks (nearest-first), or an empty slice before the first poll.
    pub fn tracks(&self) -> &[Flight] {
        self.picture
            .as_ref()
            .map(|p| p.tracks.as_slice())
            .unwrap_or(&[])
    }

    /// Poll `/picture` and fold the result into state. On success the connection
    /// is marked up and the picture replaced; on failure the connection goes down
    /// and the last picture is kept (frozen) so the screen still shows something.
    /// While the popup is open, a selected flight that has dropped out of the new
    /// picture flips the popup to "left the area" — using data already in hand,
    /// no extra fetch.
    pub fn refresh(&mut self) {
        match self.client.picture() {
            Ok(picture) => {
                self.conn = Conn::Ok;
                self.picture = Some(picture);
                if self.mode == Mode::Detail {
                    if let Some(hex) = self.selected_hex.clone() {
                        if !self.tracks().iter().any(|f| f.hex == hex) {
                            self.detail = Some(DetailView::LeftArea);
                        }
                    }
                }
            }
            Err(e) => self.conn = Conn::Down(e.to_string()),
        }
    }

    /// Fetch `/flight/{hex}` if the popup needs it (the selection changed in Detail
    /// mode). Idempotent and cheap to call every loop iteration.
    pub fn sync_detail(&mut self) {
        if !self.detail_pending {
            return;
        }
        self.detail_pending = false;
        match self.selected_hex.clone() {
            Some(hex) => {
                self.detail = Some(match self.client.flight(&hex) {
                    Ok(Some(detail)) => DetailView::Loaded(Box::new(detail)),
                    Ok(None) => DetailView::LeftArea,
                    Err(e) => DetailView::Error(e.to_string()),
                });
            }
            None => self.detail = None,
        }
    }

    pub fn select_next(&mut self) {
        self.step_selection(true);
    }

    pub fn select_prev(&mut self) {
        self.step_selection(false);
    }

    pub fn clear_selection(&mut self) {
        self.selected_hex = None;
    }

    /// Open the flight-detail popup on the selected flight, defaulting to the
    /// nearest when nothing is selected. Stays on the radar if there is nothing to
    /// show, so the popup is never empty. Arms a `/flight/{hex}` fetch.
    pub fn open_detail(&mut self) {
        if self.selected_hex.is_none() {
            self.selected_hex = self
                .picture
                .as_ref()
                .and_then(|p| p.tracks.first())
                .map(|f| f.hex.clone());
        }
        if self.selected_hex.is_some() {
            self.mode = Mode::Detail;
            self.detail_scroll = 0;
            self.detail_pending = true;
        }
    }

    /// Close the popup, returning to the radar. The selection is retained.
    pub fn close_detail(&mut self) {
        self.mode = Mode::Radar;
    }

    /// Scroll the popup body by `delta` lines, saturating at the top.
    pub fn scroll_detail(&mut self, delta: i16) {
        self.detail_scroll = self.detail_scroll.saturating_add_signed(delta);
    }

    /// Move the selection to the next/previous flight in the current distance
    /// ordering, wrapping around. Starting a selection picks the nearest (forward)
    /// or the farthest (backward). In Detail mode, arms a re-fetch for the new flight.
    fn step_selection(&mut self, forward: bool) {
        let hexes: Vec<&str> = self.tracks().iter().map(|f| f.hex.as_str()).collect();
        if hexes.is_empty() {
            self.selected_hex = None;
            return;
        }
        let current = self
            .selected_hex
            .as_deref()
            .and_then(|hex| hexes.iter().position(|h| *h == hex));
        let next_hex = hexes[next_index(current, hexes.len(), forward)].to_string();

        if self.selected_hex.as_deref() != Some(next_hex.as_str()) {
            self.detail_scroll = 0;
            if self.mode == Mode::Detail {
                self.detail_pending = true;
            }
        }
        self.selected_hex = Some(next_hex);
    }
}

/// The wrapping next index for a selection step. Pure, so the selection behaviour
/// is testable without a Server.
fn next_index(current: Option<usize>, n: usize, forward: bool) -> usize {
    match (current, forward) {
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flights_api::{Health, Units, VerticalTrend};

    fn meta() -> Meta {
        Meta {
            home: flights_api::LatLon { lat: 0.0, lon: 0.0 },
            radius_nm: 100.0,
            relevance_nm: 30.0,
            source: "test".into(),
            units: Units::aviation(),
            version: "test".into(),
        }
    }

    fn flight(hex: &str, distance_nm: f64) -> Flight {
        Flight {
            hex: hex.into(),
            ident: Some(hex.to_uppercase()),
            aircraft_type: None,
            model: None,
            registration: None,
            operator: None,
            lat: 0.0,
            lon: 0.0,
            distance_nm,
            bearing_deg: 0.0,
            altitude_ft: Some(10_000.0),
            geometric_altitude_ft: None,
            groundspeed_kt: Some(400.0),
            track_deg: Some(180.0),
            vertical_rate_fpm: None,
            vertical_trend: VerticalTrend::Unknown,
            squawk: None,
            emergency: None,
            emitter_category: None,
            age_s: 0.0,
            cpa: None,
        }
    }

    /// An App with no real Server behind it; we only drive the (network-free)
    /// selection logic, never `refresh`/`sync_detail`.
    fn app(tracks: Vec<Flight>) -> App {
        let mut a = App::new(
            Client::new("http://127.0.0.1:0"),
            meta(),
            std::time::Duration::from_millis(250),
        );
        a.picture = Some(PictureResponse {
            as_of: 0.0,
            health: Health::Live,
            last_error: None,
            snapshot_age_s: Some(0.0),
            pacing_hex: None,
            tracks,
        });
        a
    }

    #[test]
    fn next_index_wraps_both_directions() {
        assert_eq!(next_index(None, 3, true), 0);
        assert_eq!(next_index(None, 3, false), 2);
        assert_eq!(next_index(Some(2), 3, true), 0);
        assert_eq!(next_index(Some(0), 3, false), 2);
        assert_eq!(next_index(Some(1), 3, true), 2);
    }

    #[test]
    fn open_detail_uses_nearest_when_no_selection() {
        // tracks are nearest-first as the Server sends them.
        let mut a = app(vec![flight("near", 6.0), flight("far", 60.0)]);
        assert!(a.selected_hex.is_none());
        a.open_detail();
        assert_eq!(a.mode, Mode::Detail);
        assert_eq!(a.selected_hex.as_deref(), Some("near"));
    }

    #[test]
    fn open_detail_is_noop_with_no_flights() {
        let mut a = app(vec![]);
        a.open_detail();
        assert_eq!(a.mode, Mode::Radar);
        assert!(a.selected_hex.is_none());
    }

    #[test]
    fn stepping_selection_wraps_over_the_track_order() {
        let mut a = app(vec![flight("a", 1.0), flight("b", 2.0), flight("c", 3.0)]);
        a.select_next(); // None -> first
        assert_eq!(a.selected_hex.as_deref(), Some("a"));
        a.select_prev(); // wrap to last
        assert_eq!(a.selected_hex.as_deref(), Some("c"));
        a.select_next(); // wrap to first
        assert_eq!(a.selected_hex.as_deref(), Some("a"));
    }

    #[test]
    fn close_detail_retains_selection() {
        let mut a = app(vec![flight("aaa", 1.0)]);
        a.open_detail();
        let sel = a.selected_hex.clone();
        a.close_detail();
        assert_eq!(a.mode, Mode::Radar);
        assert_eq!(a.selected_hex, sel);
    }
}
