//! UI state and the logic behind each [`super::event::Action`]. The `App` owns
//! the UI-side [`Tracker`] (fed by the poller) and the selection, which is held
//! by aircraft `hex` rather than list index so the highlight stays on the same
//! aircraft as the list re-sorts each frame.

use std::time::{Duration, Instant};

use ratatui::widgets::ListState;

use crate::domain::SearchArea;
use crate::poller::PollUpdate;
use crate::tracker::{Tracker, TrackerConfig};

/// Which screen the TUI is showing. The radar/list is always drawn; in `Detail`
/// the flight-detail popup is layered on top and the keys change meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Radar,
    Detail,
}

pub struct App {
    pub tracker: Tracker,
    pub area: SearchArea,
    pub source_name: String,
    /// One frame's duration — the input timeout and redraw cadence.
    pub fps_interval: Duration,
    pub relevance_nm: f64,

    pub running: bool,
    /// Which screen is showing (radar vs. the detail popup).
    pub mode: Mode,
    /// Selected aircraft by stable `hex`, or `None` for no selection.
    pub selected_hex: Option<String>,
    /// Vertical scroll offset (in lines) of the detail popup body. Reset to 0
    /// whenever the inspected flight changes.
    pub detail_scroll: u16,
    /// When the poller's next fetch is due, for the status countdown.
    pub next_poll_at: Option<Instant>,
    /// Scroll/selection state for the flight list widget.
    pub list_state: ListState,
}

impl App {
    pub fn new(
        area: SearchArea,
        tracker_cfg: TrackerConfig,
        source_name: String,
        fps_interval: Duration,
        relevance_nm: f64,
    ) -> Self {
        Self {
            tracker: Tracker::new(area, tracker_cfg),
            area,
            source_name,
            fps_interval,
            relevance_nm,
            running: true,
            mode: Mode::Radar,
            selected_hex: None,
            detail_scroll: 0,
            next_poll_at: None,
            list_state: ListState::default(),
        }
    }

    /// Fold a poller update into state: a Snapshot replaces the tracked picture
    /// and arms the next-poll countdown; an Error is noted and the countdown is
    /// set to the poller's backoff.
    pub fn on_update(&mut self, update: PollUpdate) {
        let now = Instant::now();
        match update {
            PollUpdate::Snapshot {
                snapshot,
                next_interval,
            } => {
                self.tracker.ingest(snapshot);
                self.next_poll_at = Some(now + next_interval);
            }
            PollUpdate::Error { error, retry_in } => {
                self.tracker.note_error(error.to_string());
                self.next_poll_at = Some(now + retry_in);
            }
        }
    }

    /// The poller thread ended unexpectedly (its channel closed).
    pub fn note_poller_stopped(&mut self) {
        self.tracker.note_error("poller stopped");
        self.next_poll_at = None;
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
    /// nearest flight when nothing is selected. Stays on the radar if there is
    /// nothing to show (no flights), so the popup is never empty.
    pub fn open_detail(&mut self) {
        if self.selected_hex.is_none() {
            let tracks = self.tracker.tracks_at(Instant::now());
            self.selected_hex = tracks.first().map(|t| t.flight.hex.clone());
        }
        if self.selected_hex.is_some() {
            self.mode = Mode::Detail;
            self.detail_scroll = 0;
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
    /// ordering, wrapping around. Starting a selection picks the nearest (when
    /// moving forward) or the farthest (backward).
    fn step_selection(&mut self, forward: bool) {
        let tracks = self.tracker.tracks_at(Instant::now());
        if tracks.is_empty() {
            self.selected_hex = None;
            return;
        }
        let n = tracks.len();
        let current = self
            .selected_hex
            .as_ref()
            .and_then(|hex| tracks.iter().position(|t| &t.flight.hex == hex));
        let index = match (current, forward) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, true) => 0,
            (None, false) => n - 1,
        };
        let next_hex = tracks[index].flight.hex.clone();
        // A new flight means the popup starts from the top again.
        if self.selected_hex.as_deref() != Some(next_hex.as_str()) {
            self.detail_scroll = 0;
        }
        self.selected_hex = Some(next_hex);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Flight, LatLon, Snapshot};
    use crate::tracker::TrackerConfig;

    fn app() -> App {
        let area = SearchArea {
            center: LatLon::new(0.0, 0.0),
            radius_nm: 100.0,
        };
        let cfg = TrackerConfig {
            relevance_distance_nm: 30.0,
            stale_after: Duration::from_secs(120),
            max_flight_age: Duration::from_secs(120),
        };
        App::new(area, cfg, "test".into(), Duration::from_millis(250), 30.0)
    }

    fn flight(hex: &str, lat: f64) -> Flight {
        Flight {
            hex: hex.into(),
            ident: Some(hex.to_uppercase()),
            aircraft_type: None,
            model: None,
            registration: None,
            operator: None,
            position: LatLon::new(lat, 0.0),
            altitude_ft: Some(10_000.0),
            geometric_altitude_ft: None,
            groundspeed_kt: Some(400.0),
            track_deg: Some(180.0),
            vertical_rate_fpm: None,
            squawk: None,
            emergency: None,
            emitter_category: None,
            reported_age: Duration::ZERO,
            details: Vec::new(),
        }
    }

    fn ingest(app: &mut App, flights: Vec<Flight>) {
        app.on_update(PollUpdate::Snapshot {
            snapshot: Snapshot::new(flights, Instant::now()),
            next_interval: Duration::from_secs(1),
        });
    }

    #[test]
    fn open_detail_uses_nearest_when_no_selection() {
        let mut a = app();
        // 0.1° is nearer than 0.5°; tracks are distance-sorted, so nearest is first.
        ingest(&mut a, vec![flight("far", 0.5), flight("near", 0.1)]);
        assert!(a.selected_hex.is_none());
        a.open_detail();
        assert_eq!(a.mode, Mode::Detail);
        assert_eq!(a.selected_hex.as_deref(), Some("near"));
    }

    #[test]
    fn open_detail_is_noop_with_no_flights() {
        let mut a = app();
        a.open_detail();
        assert_eq!(a.mode, Mode::Radar);
        assert!(a.selected_hex.is_none());
    }

    #[test]
    fn close_detail_retains_selection() {
        let mut a = app();
        ingest(&mut a, vec![flight("aaa", 0.1)]);
        a.open_detail();
        let sel = a.selected_hex.clone();
        a.close_detail();
        assert_eq!(a.mode, Mode::Radar);
        assert_eq!(a.selected_hex, sel);
    }

    #[test]
    fn changing_flight_resets_detail_scroll() {
        let mut a = app();
        ingest(&mut a, vec![flight("aaa", 0.1), flight("bbb", 0.2)]);
        a.open_detail();
        a.scroll_detail(5);
        assert_eq!(a.detail_scroll, 5);
        a.select_next();
        assert_eq!(a.detail_scroll, 0);
    }
}
