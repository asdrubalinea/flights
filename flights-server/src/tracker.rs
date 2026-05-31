//! Holds a **retained set of tracks keyed by hex** and answers the questions the
//! rest of the app asks of it, all at a caller-supplied instant:
//!
//! - **Nearest flight** — smallest ground distance from Home (what the display shows).
//! - **Pacing flight** — soonest CPA among approaching, relevant, *in-contact*
//!   flights (what decides the next poll). Often a *different* aircraft from the nearest.
//! - **Staleness** — whether polls have stopped, and which blips have aged out.
//!
//! A [`Snapshot`] is **merged** into the retained set rather than swapped in
//! wholesale (ADR-0007). A flight a poll reports is **in contact** and gets
//! dead-reckoned to the query instant; a flight a *successful* poll omits is
//! **lost** — kept, **frozen** at its last-known position, and held until it ages
//! past the staleness cap. Lost contact carries a reason (see [`LostReason`]).
//! Dead reckoning is therefore **contact-gated**: only in-contact tracks are
//! extrapolated, and each from its *own* last-seen instant — for a lost flight no
//! correcting poll is coming, so gliding it on would be fiction.

use std::collections::{HashMap, HashSet};
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

/// Why a tracked flight is **lost** — resolved when a successful poll first omits
/// it, then held for the life of the track (see CONTEXT.md "Contact"). The reasons
/// share one staleness cap; they differ only in how much the data substantiates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostReason {
    /// The Source reported this aircraft on the ground — the one disappearance we
    /// confirm from real data rather than infer.
    Landed,
    /// Its last course would now carry it outside the Search radius (pure geometry,
    /// high confidence). The blip still stays *frozen* at the last-known position;
    /// the extrapolation decides only the reason.
    LeftScope,
    /// Omitted, not on the ground, not outbound — cause genuinely unknown. We don't
    /// guess (a coverage gap, an MLAT dropout, a landing we never got a report for).
    LostContact,
}

/// A track's **contact state** (ADR-0007). Decides whether the Track is
/// dead-reckoned (in contact) or *frozen* (lost), and whether it may **pace**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactState {
    /// The latest poll reported this flight.
    InContact,
    /// A successful poll omitted it; kept, frozen, and badged with the reason.
    Lost(LostReason),
}

impl ContactState {
    /// Whether the latest poll reported this flight (eligible to be dead-reckoned
    /// and to pace). A lost flight is neither.
    pub fn is_in_contact(self) -> bool {
        matches!(self, ContactState::InContact)
    }
}

/// A flight as estimated at a particular instant, with the geometry derived from
/// that estimate. This is what the UI and poller consume — never the raw wire data.
#[derive(Debug, Clone)]
pub struct Track {
    /// The underlying reported flight (identity, altitude, raw velocity).
    pub flight: Flight,
    /// Position at the query instant: **dead-reckoned** while in contact, **frozen**
    /// at the last-known reported position while lost.
    pub estimated: LatLon,
    /// Ground distance from Home to the estimated position, nautical miles.
    pub distance_nm: f64,
    /// Bearing from Home to the estimated position, degrees from true north
    /// (where to place the blip on a north-up radar).
    pub bearing_from_home: f64,
    /// Closest point of approach from the estimated position; `None` when the
    /// flight is not usably moving. For a lost flight this is a *stale* CPA from the
    /// frozen position — carried for display, but a lost flight never paces.
    pub cpa: Option<Cpa>,
    /// Effective age of the underlying report at the query instant
    /// (`reported_age` + time since this track was last seen). For a lost flight
    /// this is time-since-contact; it drives the staleness cap and the UI fade.
    pub age: Duration,
    /// Contact state at the query instant (see [`ContactState`]).
    pub contact: ContactState,
}

impl Track {
    /// An *in-contact*, approaching flight whose CPA distance is within the Relevance
    /// distance — i.e. one eligible to set the poll cadence. A lost flight never
    /// paces: we won't spend API calls chasing a frozen, stale CPA.
    fn is_relevant(&self, relevance_distance_nm: f64) -> bool {
        if !self.contact.is_in_contact() {
            return false;
        }
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

/// One flight retained across polls: the last raw [`Flight`] a poll carried, when
/// that poll was taken, and the current contact state. The Tracker holds a map of
/// these keyed by hex and **merges** each Snapshot into it (ADR-0007).
#[derive(Debug, Clone)]
struct RetainedTrack {
    /// The last reported flight (identity, raw velocity, `reported_age`, details).
    /// Its `position` is the **last-known** position a lost track freezes at.
    flight: Flight,
    /// The instant of the poll that last *reported* this flight. Stays fixed once
    /// lost, so the track ages out measured from its last real contact.
    last_seen: Instant,
    contact: ContactState,
}

pub struct Tracker {
    area: SearchArea,
    cfg: TrackerConfig,
    /// Retained tracks keyed by hex — the held state a Snapshot is merged into.
    tracks: HashMap<String, RetainedTrack>,
    /// Instant of the most recent *successful* poll — drives snapshot age and
    /// whole-Picture health, independent of any per-flight lost contact.
    last_poll_at: Option<Instant>,
    last_error: Option<String>,
}

impl Tracker {
    pub fn new(area: SearchArea, cfg: TrackerConfig) -> Self {
        Self {
            area,
            cfg,
            tracks: HashMap::new(),
            last_poll_at: None,
            last_error: None,
        }
    }

    /// **Merge** a Snapshot into the retained tracks (ADR-0007), not swap it in:
    ///
    /// 1. Each airborne flight refreshes its track and is marked **in contact**.
    /// 2. Each on-ground hex we *already* track turns **landed** (untracked ones are
    ///    ignored — no new ground blips).
    /// 3. Any remaining track this *successful* poll omitted has **lost contact**;
    ///    a newly-lost track gets a reason now, while one already lost keeps its
    ///    reason and last-seen instant so it ages out from its last real contact.
    /// 4. Tracks already past the staleness cap are dropped to bound memory.
    ///
    /// Only a successful poll calls this, so omission means "heard others, not this
    /// one" — never a total outage (which leaves every track in contact and
    /// dead-reckoned under a stale Picture; see [`Tracker::note_error`]).
    pub fn ingest(&mut self, snapshot: Snapshot) {
        let now = snapshot.taken_at;
        self.last_poll_at = Some(now);
        self.last_error = None;

        let mut reported: HashSet<String> =
            HashSet::with_capacity(snapshot.flights.len() + snapshot.on_ground.len());

        // 1. Airborne reports → in contact (refresh or insert).
        for flight in snapshot.flights {
            reported.insert(flight.hex.clone());
            let hex = flight.hex.clone();
            self.tracks.insert(
                hex,
                RetainedTrack {
                    flight,
                    last_seen: now,
                    contact: ContactState::InContact,
                },
            );
        }

        // 2. On-ground reports → landed, but only for a hex we already track and
        //    did not just hear airborne. last_seen is left untouched (a ground
        //    report is not a fresh airborne contact), so a landed flight still ages
        //    out instead of lingering forever at a gate.
        for hex in &snapshot.on_ground {
            if reported.contains(hex) {
                continue;
            }
            if let Some(rt) = self.tracks.get_mut(hex) {
                reported.insert(hex.clone());
                rt.contact = ContactState::Lost(LostReason::Landed);
            }
        }

        // 3. Tracks this poll omitted → lost. Resolve a reason only on the
        //    in-contact → lost transition; an already-lost track is left as is.
        let area = self.area;
        for rt in self.tracks.values_mut() {
            if reported.contains(&rt.flight.hex) {
                continue;
            }
            if rt.contact.is_in_contact() {
                rt.contact = ContactState::Lost(loss_reason(rt, now, area));
            }
        }

        // 4. Bound memory: forget tracks already past the cap (also enforced per
        //    query in `resolve`, since more time passes between polls).
        let cap = self.cfg.max_flight_age;
        self.tracks.retain(|_, rt| effective_age(rt, now) <= cap);
    }

    /// Record a poll failure. The retained tracks stay **in contact** and keep being
    /// dead-reckoned (no successful poll omitted them); the error surfaces in
    /// [`Tracker::health`] once the whole Picture goes stale.
    pub fn note_error(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
    }

    /// Age of the most recent successful poll at `now`, or `None` if none has
    /// arrived. This is whole-Picture freshness, distinct from any flight's age.
    pub fn snapshot_age(&self, now: Instant) -> Option<Duration> {
        self.last_poll_at
            .map(|t| now.saturating_duration_since(t))
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

    /// Every retained track resolved to `now` — in-contact ones dead-reckoned, lost
    /// ones frozen — dropping those aged past the staleness cap. Sorted by ground
    /// distance from Home ascending (so the **Nearest flight** is first; it may be
    /// a lost track).
    pub fn tracks_at(&self, now: Instant) -> Vec<Track> {
        let mut tracks: Vec<Track> = self
            .tracks
            .values()
            .filter_map(|rt| self.resolve(rt, now))
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

    /// Resolve one retained track to `now`, or `None` if it has aged past the cap.
    /// **In contact** → dead-reckoned along its last velocity from its own last-seen
    /// instant; **lost** → *frozen* at its last-known reported position, never
    /// extrapolated (CONTEXT.md "Dead reckoning"). Either way the CPA is computed
    /// from the resolved position; pacing later ignores the lost ones.
    fn resolve(&self, rt: &RetainedTrack, now: Instant) -> Option<Track> {
        let age = effective_age(rt, now);
        if age > self.cfg.max_flight_age {
            return None;
        }

        let estimated = match rt.contact {
            ContactState::InContact => match rt.flight.velocity() {
                Some((track, gs)) => {
                    let nm = gs * age.as_secs_f64() / 3600.0;
                    geo::project(rt.flight.position, track, nm)
                }
                // Unknown / negligible velocity: hold the last reported position.
                None => rt.flight.position,
            },
            // Lost: frozen at the last-known position — no correcting poll is
            // coming, so gliding it on would be fiction.
            ContactState::Lost(_) => rt.flight.position,
        };

        let distance_nm = geo::haversine_nm(self.area.center, estimated);
        let bearing_from_home = geo::bearing_deg(self.area.center, estimated);
        let cpa = rt
            .flight
            .velocity()
            .map(|(track, gs)| geo::cpa(self.area.center, estimated, track, gs));

        Some(Track {
            flight: rt.flight.clone(),
            estimated,
            distance_nm,
            bearing_from_home,
            cpa,
            age,
            contact: rt.contact,
        })
    }
}

/// Effective age of a track's underlying position report at `now`: how old it was
/// when last reported, plus time since that poll. It grows for a lost (frozen)
/// track and for any track under a total poll outage — driving the staleness cap
/// and the Client's fade. (For a lost track this is "time since contact" plus the
/// initial report age.)
fn effective_age(rt: &RetainedTrack, now: Instant) -> Duration {
    rt.flight.reported_age + now.saturating_duration_since(rt.last_seen)
}

/// The reason a just-omitted track lost contact (landing is resolved separately,
/// from a real on-ground report). **Left scope** when dead-reckoning its last course
/// to this poll would place it outside the Search radius — a near-edge last position
/// with an outbound heading, pure geometry; otherwise plain **lost contact**, the
/// honest residual we don't guess a cause for. This hypothetical extrapolation
/// decides only the reason; the displayed blip still freezes at the last-known
/// position (see [`Tracker::resolve`]).
fn loss_reason(rt: &RetainedTrack, now: Instant, area: SearchArea) -> LostReason {
    match rt.flight.velocity() {
        Some((track, gs)) => {
            let nm = gs * effective_age(rt, now).as_secs_f64() / 3600.0;
            let hypothetical = geo::project(rt.flight.position, track, nm);
            if geo::haversine_nm(area.center, hypothetical) > area.radius_nm {
                LostReason::LeftScope
            } else {
                LostReason::LostContact
            }
        }
        // Not usably moving: it cannot be heading out of scope.
        None => LostReason::LostContact,
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

    // --- Retained tracks & contact state (ADR-0007) -----------------------------

    #[test]
    fn an_omitted_flight_is_retained_as_lost_not_dropped() {
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg());
        let a = flight("a", LatLon::new(0.1, 0.0)); // 6 nm north
        let b = flight("b", LatLon::new(0.2, 0.0)); // 12 nm north
        tr.ingest(Snapshot::new(vec![a, b], t0));

        // The next (successful) poll omits b — it is kept, not assumed gone.
        let t1 = t0 + Duration::from_secs(30);
        tr.ingest(Snapshot::new(vec![flight("a", LatLon::new(0.1, 0.0))], t1));

        let tracks = tr.tracks_at(t1);
        assert_eq!(tracks.len(), 2, "the omitted flight must be retained");
        let by = |hex| tracks.iter().find(|t| t.flight.hex == hex).unwrap();
        assert_eq!(by("a").contact, ContactState::InContact);
        // b is stationary and well inside the radius: not landed, not outbound.
        assert_eq!(by("b").contact, ContactState::Lost(LostReason::LostContact));
    }

    #[test]
    fn a_lost_flight_is_frozen_never_dead_reckoned() {
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg());
        // At Home, flying east at 360 kt — would clearly drift if dead-reckoned.
        tr.ingest(Snapshot::new(
            vec![moving("a", LatLon::new(0.0, 0.0), 90.0, 360.0)],
            t0,
        ));
        // A later poll omits it (and reports someone else, so the poll succeeded).
        let t1 = t0 + Duration::from_secs(10);
        tr.ingest(Snapshot::new(vec![flight("other", LatLon::new(0.5, 0.0))], t1));

        // A full minute on, an in-contact flight would have moved 6 nm; this one,
        // frozen at its last-known position (Home), has not budged.
        let a = tr
            .tracks_at(t1 + Duration::from_secs(60))
            .into_iter()
            .find(|t| t.flight.hex == "a")
            .unwrap();
        assert!(matches!(a.contact, ContactState::Lost(_)));
        assert!(
            a.distance_nm < 0.1,
            "a frozen flight drifted to {} nm",
            a.distance_nm
        );
    }

    #[test]
    fn an_on_ground_report_lands_a_tracked_flight_only() {
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg());
        tr.ingest(Snapshot::new(vec![flight("a", LatLon::new(0.1, 0.0))], t0));

        // Next poll: "a" is reported on the ground; "ghost" we never tracked is too.
        let t1 = t0 + Duration::from_secs(20);
        tr.ingest(Snapshot::with_ground(
            vec![flight("other", LatLon::new(0.3, 0.0))],
            vec!["a".to_string(), "ghost".to_string()],
            t1,
        ));

        let tracks = tr.tracks_at(t1);
        let a = tracks.iter().find(|t| t.flight.hex == "a").unwrap();
        assert_eq!(a.contact, ContactState::Lost(LostReason::Landed));
        // The untracked ground hex never becomes a blip (airborne-only at the seam).
        assert!(tracks.iter().all(|t| t.flight.hex != "ghost"));
    }

    #[test]
    fn an_outbound_edge_flight_is_lost_to_left_scope_but_still_frozen_inside() {
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg());
        // 95 nm north (radius is 100), flying due north at 600 kt.
        let edge = moving("edge", LatLon::new(95.0 / 60.0, 0.0), 0.0, 600.0);
        tr.ingest(Snapshot::new(vec![edge], t0));

        // A poll 60 s later omits it. Held course → 95 + 600·(60/3600) = 105 nm,
        // past the radius ⇒ left scope.
        let t1 = t0 + Duration::from_secs(60);
        tr.ingest(Snapshot::new(vec![flight("anchor", LatLon::new(0.0, 0.0))], t1));

        let e = tr
            .tracks_at(t1)
            .into_iter()
            .find(|t| t.flight.hex == "edge")
            .unwrap();
        assert_eq!(e.contact, ContactState::Lost(LostReason::LeftScope));
        // The reason used the hypothetical extrapolation, but the *displayed* blip
        // stays frozen at the last-known 95 nm — never glided out past the edge.
        assert!(
            (e.distance_nm - 95.0).abs() < 0.5,
            "frozen blip should stay at the last-known 95 nm, was {} nm",
            e.distance_nm
        );
    }

    #[test]
    fn a_one_poll_dropout_returns_to_in_contact() {
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg());
        let moving_a = || moving("a", LatLon::new(0.1, 0.0), 90.0, 360.0);
        tr.ingest(Snapshot::new(vec![moving_a()], t0));

        // Dropped for one poll → lost.
        let t1 = t0 + Duration::from_secs(10);
        tr.ingest(Snapshot::new(vec![flight("b", LatLon::new(0.5, 0.0))], t1));
        assert!(matches!(
            tr.tracks_at(t1)
                .into_iter()
                .find(|t| t.flight.hex == "a")
                .unwrap()
                .contact,
            ContactState::Lost(_)
        ));

        // Reappears next poll → back in contact (this is the jitter we refuse to
        // turn into flicker).
        let t2 = t1 + Duration::from_secs(10);
        tr.ingest(Snapshot::new(vec![moving_a()], t2));
        let a = tr
            .tracks_at(t2)
            .into_iter()
            .find(|t| t.flight.hex == "a")
            .unwrap();
        assert_eq!(a.contact, ContactState::InContact);
    }

    #[test]
    fn a_lost_flight_can_be_nearest_but_never_paces() {
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg());
        // Two inbounds on the Home meridian (both pass ~overhead → relevant).
        // The closer/faster one paces; the farther/slower one is the fallback.
        let inbound = moving("inbound", LatLon::new(-20.0 / 60.0, 0.0), 0.0, 480.0);
        let inbound2 = moving("inbound2", LatLon::new(-40.0 / 60.0, 0.0), 0.0, 300.0);
        tr.ingest(Snapshot::new(vec![inbound, inbound2], t0));
        assert_eq!(tr.nearest_at(t0).unwrap().flight.hex, "inbound");
        assert_eq!(tr.pacing_at(t0).unwrap().flight.hex, "inbound");

        // Next poll omits the pacing flight; the other stays in contact.
        let t1 = t0 + Duration::from_secs(5);
        tr.ingest(Snapshot::new(
            vec![moving("inbound2", LatLon::new(-38.0 / 60.0, 0.0), 0.0, 300.0)],
            t1,
        ));

        // It is still the Nearest (closest thing we know of), now lost...
        let near = tr.nearest_at(t1).unwrap();
        assert_eq!(near.flight.hex, "inbound");
        assert!(matches!(near.contact, ContactState::Lost(_)));
        // ...but pacing has moved to the still-in-contact flight: we won't chase a
        // frozen, stale CPA.
        assert_eq!(tr.pacing_at(t1).unwrap().flight.hex, "inbound2");
    }

    #[test]
    fn a_total_outage_keeps_dead_reckoning_under_a_stale_picture() {
        // Stale flag trips at 30 s; tracks survive to 120 s — so there is a window
        // where the Picture is stale yet flights still glide (the case this guards).
        // Production keeps the same shape: the stale flag at 2 × max_poll, the drop
        // cap one poll-interval higher at 3 × max_poll (see `tracker_cfg`). The exact
        // numbers here just widen the window so both states are observable at once.
        let cfg = TrackerConfig {
            relevance_distance_nm: 30.0,
            stale_after: Duration::from_secs(30),
            max_flight_age: Duration::from_secs(120),
        };
        let t0 = Instant::now();
        let mut tr = Tracker::new(area(), cfg);
        tr.ingest(Snapshot::new(
            vec![moving("a", LatLon::new(0.0, 0.0), 90.0, 360.0)],
            t0,
        ));

        // Every poll now fails: no ingest, only errors. No *successful* poll omits
        // the flight, so it stays in contact and keeps gliding — unlike a per-flight
        // lost contact.
        tr.note_error("network down");
        let later = t0 + Duration::from_secs(60); // 360 kt · 60 s = 6 nm east
        let a = tr
            .tracks_at(later)
            .into_iter()
            .find(|t| t.flight.hex == "a")
            .unwrap();
        assert_eq!(a.contact, ContactState::InContact);
        assert!(
            (a.distance_nm - 6.0).abs() < 0.1,
            "a flight under a total outage should keep dead-reckoning, was {} nm",
            a.distance_nm
        );
        // The whole Picture is stale, but the flight is not marked lost and survives.
        assert!(matches!(tr.health(later), Health::Stale { .. }));

        // Past the cap it is finally dropped, exactly as before.
        assert!(tr.tracks_at(t0 + Duration::from_secs(200)).is_empty());
    }
}
