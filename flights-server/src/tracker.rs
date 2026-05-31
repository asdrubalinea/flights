//! Holds the latest [`Snapshot`] and answers the three questions the rest of the
//! app asks of it, all at a caller-supplied instant via **dead reckoning**:
//!
//! - **Nearest flight** — smallest ground distance from Home (what the display shows).
//! - **Pacing flight** — soonest CPA among approaching, relevant flights (what
//!   decides the next poll). Often a *different* aircraft from the nearest.
//! - **Staleness** — whether polls have stopped, and which blips have aged out.
//!
//! Between Snapshots the tracker never invents a poll: it extrapolates each
//! flight's last reported position along its last reported velocity. A flight
//! whose effective age exceeds the staleness cap is dropped so the radar never
//! shows fiction indefinitely.

use std::time::{Duration, Instant};

use crate::domain::{Flight, LatLon, SearchArea, Snapshot};
use crate::geo::{self, Cpa};

/// Tunables the tracker needs that are not part of the Search area itself.
#[derive(Debug, Clone, Copy)]
pub struct TrackerConfig {
    /// The CPA-distance cutoff beyond which an approaching flight is ignored for
    /// **pacing** (never for display). Bounded by the Search radius upstream.
    pub relevance_distance_nm: f64,
    /// Snapshot age beyond which the whole picture is flagged *stale* (polling
    /// appears to have stopped). Typically ~2× the max poll interval.
    pub stale_after: Duration,
    /// Per-flight effective-age beyond which a blip is dropped — the staleness
    /// cap that stops dead reckoning from showing fiction forever.
    pub max_flight_age: Duration,
}

/// A flight as estimated at a particular instant, with the geometry derived from
/// that estimate. This is what the UI and poller consume — never the raw wire data.
#[derive(Debug, Clone)]
pub struct Track {
    /// The underlying reported flight (identity, altitude, raw velocity).
    pub flight: Flight,
    /// Dead-reckoned position at the query instant.
    pub estimated: LatLon,
    /// Ground distance from Home to the estimated position, nautical miles.
    pub distance_nm: f64,
    /// Bearing from Home to the estimated position, degrees from true north
    /// (where to place the blip on a north-up radar).
    pub bearing_from_home: f64,
    /// Closest point of approach from the estimated position; `None` when the
    /// flight is not usably moving (it neither dead-reckons nor paces).
    pub cpa: Option<Cpa>,
    /// Effective age of the underlying report at the query instant
    /// (`reported_age` + time since the Snapshot). Drives the staleness cap and
    /// can fade a blip in the UI.
    pub age: Duration,
}

impl Track {
    /// An approaching flight whose CPA distance is within the Relevance distance —
    /// i.e. one eligible to set the poll cadence.
    fn is_relevant(&self, relevance_distance_nm: f64) -> bool {
        match self.cpa {
            Some(c) => c.is_approaching() && c.cpa_distance_nm <= relevance_distance_nm,
            None => false,
        }
    }
}

/// Why the tracker currently has no usable picture, for the status line.
#[derive(Debug, Clone)]
pub enum Health {
    /// A fresh-enough Snapshot is in hand.
    Live,
    /// We have a Snapshot but it has aged past `stale_after`; `last_error`
    /// carries the most recent poll failure, if any.
    Stale { last_error: Option<String> },
    /// No Snapshot has ever arrived.
    NoData { last_error: Option<String> },
}

/// Everything the tracker can say about the airspace at one instant, derived
/// from a **single** dead-reckoning pass: the tracks (nearest first), which of
/// them are the Nearest and Pacing flights, and the overall health. Computing
/// these together is what guarantees the radar, list, and status never disagree
/// — they are all reading the same vector, not three independent recomputations.
pub struct Picture {
    /// All flights dead-reckoned to the query instant, sorted by ground distance
    /// from Home ascending.
    pub tracks: Vec<Track>,
    pub health: Health,
    nearest_idx: Option<usize>,
    pacing_idx: Option<usize>,
}

impl Picture {
    /// The **Nearest flight**: smallest ground distance from Home (what the
    /// display shows).
    pub fn nearest(&self) -> Option<&Track> {
        self.nearest_idx.map(|i| &self.tracks[i])
    }

    /// The **Pacing flight**: soonest CPA among approaching, relevant flights
    /// (what sets the poll cadence) — often a different aircraft from the nearest.
    pub fn pacing(&self) -> Option<&Track> {
        self.pacing_idx.map(|i| &self.tracks[i])
    }
}

/// Index in `tracks` of the **Pacing flight**: among approaching flights whose
/// CPA distance is within the Relevance distance, the one whose CPA is *soonest*.
/// `tracks` ordering is irrelevant here — pacing is by time-to-CPA, not distance.
fn pacing_index(tracks: &[Track], relevance_distance_nm: f64) -> Option<usize> {
    tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_relevant(relevance_distance_nm))
        .min_by(|(_, a), (_, b)| {
            // Both are relevant ⇒ both have an approaching CPA.
            let ta = a.cpa.map_or(f64::INFINITY, |c| c.time_to_cpa_s);
            let tb = b.cpa.map_or(f64::INFINITY, |c| c.time_to_cpa_s);
            ta.total_cmp(&tb)
        })
        .map(|(i, _)| i)
}

pub struct Tracker {
    area: SearchArea,
    cfg: TrackerConfig,
    snapshot: Option<Snapshot>,
    last_error: Option<String>,
}

impl Tracker {
    pub fn new(area: SearchArea, cfg: TrackerConfig) -> Self {
        Self {
            area,
            cfg,
            snapshot: None,
            last_error: None,
        }
    }

    /// Replace the held Snapshot wholesale and clear the last error.
    pub fn ingest(&mut self, snapshot: Snapshot) {
        self.snapshot = Some(snapshot);
        self.last_error = None;
    }

    /// Record a poll failure. The held Snapshot is retained and dead-reckoned;
    /// the error surfaces in [`Tracker::health`] once the picture goes stale.
    pub fn note_error(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
    }

    /// Age of the held Snapshot at `now`, or `None` if none has arrived.
    pub fn snapshot_age(&self, now: Instant) -> Option<Duration> {
        self.snapshot
            .as_ref()
            .map(|s| now.saturating_duration_since(s.taken_at))
    }

    pub fn health(&self, now: Instant) -> Health {
        match self.snapshot_age(now) {
            None => Health::NoData {
                last_error: self.last_error.clone(),
            },
            Some(age) if age > self.cfg.stale_after => Health::Stale {
                last_error: self.last_error.clone(),
            },
            Some(_) => Health::Live,
        }
    }

    /// Every flight dead-reckoned to `now`, with derived geometry, dropping those
    /// aged past the staleness cap. Sorted by ground distance from Home ascending
    /// (so the **Nearest flight** is first).
    pub fn tracks_at(&self, now: Instant) -> Vec<Track> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let elapsed = now.saturating_duration_since(snapshot.taken_at);

        let mut tracks: Vec<Track> = snapshot
            .flights
            .iter()
            .filter_map(|f| self.dead_reckon(f, elapsed))
            .collect();
        tracks.sort_by(|a, b| a.distance_nm.total_cmp(&b.distance_nm));
        tracks
    }

    /// The **Nearest flight** at `now`: smallest ground distance from Home.
    pub fn nearest_at(&self, now: Instant) -> Option<Track> {
        self.tracks_at(now).into_iter().next()
    }

    /// The **Pacing flight** at `now`: among approaching flights whose CPA
    /// distance is within the Relevance distance, the one whose CPA is *soonest*.
    /// This is what sets the poll cadence — distinct from the Nearest flight.
    ///
    /// Callers that also need the tracks or nearest flight (the UI) should prefer
    /// [`Tracker::picture_at`], which derives all three from one pass.
    pub fn pacing_at(&self, now: Instant) -> Option<Track> {
        let tracks = self.tracks_at(now);
        pacing_index(&tracks, self.cfg.relevance_distance_nm).map(|i| tracks[i].clone())
    }

    /// The complete derived [`Picture`] at `now` — tracks, Nearest, Pacing, and
    /// health — from a single dead-reckoning pass. This is the one the UI uses
    /// each frame, so the radar, list, and status are always mutually consistent
    /// (and the expensive `tracks_at` runs once, not once per question).
    pub fn picture_at(&self, now: Instant) -> Picture {
        let tracks = self.tracks_at(now);
        let nearest_idx = (!tracks.is_empty()).then_some(0);
        let pacing_idx = pacing_index(&tracks, self.cfg.relevance_distance_nm);
        Picture {
            tracks,
            health: self.health(now),
            nearest_idx,
            pacing_idx,
        }
    }

    /// Dead-reckon one flight forward by `elapsed` (time since the Snapshot),
    /// returning `None` if its effective age exceeds the staleness cap.
    fn dead_reckon(&self, flight: &Flight, elapsed: Duration) -> Option<Track> {
        let age = flight.reported_age + elapsed;
        if age > self.cfg.max_flight_age {
            return None;
        }

        let estimated = match flight.velocity() {
            Some((track, gs)) => {
                let nm = gs * age.as_secs_f64() / 3600.0;
                geo::project(flight.position, track, nm)
            }
            // Unknown / negligible velocity: hold the last reported position.
            None => flight.position,
        };

        let distance_nm = geo::haversine_nm(self.area.center, estimated);
        let bearing_from_home = geo::bearing_deg(self.area.center, estimated);
        let cpa = flight
            .velocity()
            .map(|(track, gs)| geo::cpa(self.area.center, estimated, track, gs));

        Some(Track {
            flight: flight.clone(),
            estimated,
            distance_nm,
            bearing_from_home,
            cpa,
            age,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> SearchArea {
        SearchArea {
            center: LatLon::new(0.0, 0.0),
            radius_nm: 100.0,
        }
    }

    fn cfg() -> TrackerConfig {
        TrackerConfig {
            relevance_distance_nm: 30.0,
            stale_after: Duration::from_secs(120),
            max_flight_age: Duration::from_secs(120),
        }
    }

    fn flight(hex: &str, pos: LatLon) -> Flight {
        Flight {
            hex: hex.into(),
            ident: None,
            aircraft_type: None,
            model: None,
            registration: None,
            operator: None,
            position: pos,
            altitude_ft: Some(30_000.0),
            geometric_altitude_ft: None,
            groundspeed_kt: None,
            track_deg: None,
            vertical_rate_fpm: None,
            squawk: None,
            emergency: None,
            emitter_category: None,
            reported_age: Duration::ZERO,
            details: Vec::new(),
        }
    }

    fn moving(hex: &str, pos: LatLon, track: f64, gs: f64) -> Flight {
        Flight {
            track_deg: Some(track),
            groundspeed_kt: Some(gs),
            ..flight(hex, pos)
        }
    }

    #[test]
    fn dead_reckoning_advances_along_track_at_groundspeed() {
        let t0 = Instant::now();
        // Flying due east at 360 kt → 1 nm in 10 s.
        let f = moving("abc", LatLon::new(0.0, 0.0), 90.0, 360.0);
        let mut tr = Tracker::new(area(), cfg());
        tr.ingest(Snapshot::new(vec![f], t0));

        let track = tr.nearest_at(t0 + Duration::from_secs(10)).unwrap();
        assert!(
            (track.distance_nm - 1.0).abs() < 1e-2,
            "moved {} nm",
            track.distance_nm
        );
        assert!(track.estimated.lon > 0.0, "should drift east");
        assert!(
            track.estimated.lat.abs() < 1e-6,
            "should not drift north/south"
        );
    }

    #[test]
    fn reported_age_counts_toward_extrapolation() {
        let t0 = Instant::now();
        // Position already 10 s old at snapshot time, zero elapsed since:
        // still extrapolated by the full 10 s ⇒ 1 nm east.
        let f = Flight {
            reported_age: Duration::from_secs(10),
            ..moving("abc", LatLon::new(0.0, 0.0), 90.0, 360.0)
        };
        let mut tr = Tracker::new(area(), cfg());
        tr.ingest(Snapshot::new(vec![f], t0));

        let track = tr.nearest_at(t0).unwrap();
        assert!(
            (track.distance_nm - 1.0).abs() < 1e-2,
            "moved {} nm",
            track.distance_nm
        );
    }

    #[test]
    fn nearest_is_the_smallest_ground_distance() {
        let t0 = Instant::now();
        let near = flight("near", LatLon::new(0.1, 0.0)); // 6 nm
        let far = flight("far", LatLon::new(1.0, 0.0)); // 60 nm
        let mut tr = Tracker::new(area(), cfg());
        tr.ingest(Snapshot::new(vec![far, near], t0));

        assert_eq!(tr.nearest_at(t0).unwrap().flight.hex, "near");
    }

    #[test]
    fn pacing_picks_soonest_approaching_within_relevance() {
        let t0 = Instant::now();
        // Receding overhead jet (nearest, but not pacing): just north, flying north.
        let receding = moving("recede", LatLon::new(0.05, 0.0), 0.0, 400.0);
        // Inbound, within relevance, CPA ~ far in time (50 nm south, slow).
        let slow_inbound = moving("slow", LatLon::new(-50.0 / 60.0, 0.0), 0.0, 120.0);
        // Inbound, within relevance, CPA sooner (20 nm south, fast).
        let fast_inbound = moving("fast", LatLon::new(-20.0 / 60.0, 0.0), 0.0, 480.0);
        // Inbound but will miss by 80 nm — beyond relevance, must not pace.
        let wide = moving("wide", LatLon::new(-50.0 / 60.0, 80.0 / 60.0), 0.0, 480.0);

        let mut tr = Tracker::new(area(), cfg());
        tr.ingest(Snapshot::new(
            vec![receding, slow_inbound, fast_inbound, wide],
            t0,
        ));

        // Nearest is the receding jet; pacing is the soonest relevant inbound.
        assert_eq!(tr.nearest_at(t0).unwrap().flight.hex, "recede");
        assert_eq!(tr.pacing_at(t0).unwrap().flight.hex, "fast");
    }

    #[test]
    fn picture_agrees_with_the_granular_queries() {
        let t0 = Instant::now();
        let receding = moving("recede", LatLon::new(0.05, 0.0), 0.0, 400.0);
        let fast_inbound = moving("fast", LatLon::new(-20.0 / 60.0, 0.0), 0.0, 480.0);
        let mut tr = Tracker::new(area(), cfg());
        tr.ingest(Snapshot::new(vec![receding, fast_inbound], t0));

        let picture = tr.picture_at(t0);
        // One pass yields the same answers as three separate calls.
        assert_eq!(picture.tracks.len(), tr.tracks_at(t0).len());
        assert_eq!(
            picture.nearest().map(|t| &t.flight.hex),
            tr.nearest_at(t0).map(|t| t.flight.hex).as_ref()
        );
        assert_eq!(
            picture.pacing().map(|t| &t.flight.hex),
            tr.pacing_at(t0).map(|t| t.flight.hex).as_ref()
        );
        // And the specific aircraft are as expected: nearest receding, pacing inbound.
        assert_eq!(picture.nearest().unwrap().flight.hex, "recede");
        assert_eq!(picture.pacing().unwrap().flight.hex, "fast");
    }

    #[test]
    fn flights_aged_past_the_cap_are_dropped() {
        let t0 = Instant::now();
        let f = flight("old", LatLon::new(0.1, 0.0));
        let mut tr = Tracker::new(area(), cfg());
        tr.ingest(Snapshot::new(vec![f], t0));

        // Well past the 120 s cap.
        assert!(tr.tracks_at(t0 + Duration::from_secs(200)).is_empty());
        assert!(tr.nearest_at(t0 + Duration::from_secs(200)).is_none());
    }

    #[test]
    fn health_goes_stale_then_reports_no_data_initially() {
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg());
        assert!(matches!(tr.health(t0), Health::NoData { .. }));

        tr.ingest(Snapshot::new(vec![flight("a", LatLon::new(0.1, 0.0))], t0));
        assert!(matches!(tr.health(t0), Health::Live));
        assert!(matches!(
            tr.health(t0 + Duration::from_secs(200)),
            Health::Stale { .. }
        ));
    }
}
