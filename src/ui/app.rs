//! UI state and the logic behind each [`super::event::Action`]. The `App` owns
//! the UI-side [`Tracker`] (fed by the poller) and the selection, which is held
//! by aircraft `hex` rather than list index so the highlight stays on the same
//! aircraft as the list re-sorts each frame.

use std::time::{Duration, Instant};

use ratatui::widgets::ListState;

use crate::domain::SearchArea;
use crate::poller::PollUpdate;
use crate::tracker::{Tracker, TrackerConfig};

pub struct App {
    pub tracker: Tracker,
    pub area: SearchArea,
    pub source_name: String,
    /// One frame's duration — the input timeout and redraw cadence.
    pub fps_interval: Duration,
    pub relevance_nm: f64,

    pub running: bool,
    /// Selected aircraft by stable `hex`, or `None` for no selection.
    pub selected_hex: Option<String>,
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
            selected_hex: None,
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
        self.selected_hex = Some(tracks[index].flight.hex.clone());
    }
}
