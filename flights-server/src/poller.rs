//! The poller: a single background thread that owns the active Source, decides
//! its own poll cadence, and writes each fresh Snapshot into the shared
//! [`Tracker`] (ADR-0002). It never refreshes for any screen — Clients
//! dead-reckon the held Snapshot on read — so request volume tracks how
//! *interesting* the airspace is, not how many Clients are watching.
//!
//! After ingesting a Snapshot the poller reads the Tracker back to find the
//! current Pacing flight, which sets the next interval. Cadence is bounded
//! **below** by the Source's `min_interval()` (the rate-limit floor) and **above**
//! by the configured max (kept under Search-area transit time). Between those, the
//! poll interval shrinks as the Pacing flight's CPA approaches. On error the
//! poller backs off exponentially, honoring an explicit `Retry-After` when the
//! Source supplies one, and notes the error on the Tracker so it can surface as
//! `stale` once the held Snapshot ages out.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::domain::SearchArea;
use crate::geo::Cpa;
use crate::sources::{FlightSource, SourceError};
use crate::tracker::Tracker;

/// The poll-interval window: `[min, max]`. `min` is the Source's floor; `max` is
/// the quiet-airspace cadence (configured, below transit time).
#[derive(Debug, Clone, Copy)]
pub struct PollBounds {
    pub min: Duration,
    pub max: Duration,
}

/// Aim to poll this fraction of the way into the Pacing flight's time-to-CPA, so
/// we sample an imminent approach several times before it arrives.
const PACE_FRACTION: f64 = 0.25;

/// Absolute ceiling on how long we'll honor a `Retry-After`, so a hostile or
/// buggy header can't park the poller for an unreasonable time.
const RETRY_AFTER_CEILING: Duration = Duration::from_secs(300);

/// The adaptive cadence: how long to wait before the next poll given the current
/// Pacing flight's CPA (if any). Quiet airspace → the max (slowest) interval; an
/// imminent approach → as fast as the Source floor allows.
pub fn schedule(pacing: Option<Cpa>, bounds: PollBounds) -> Duration {
    match pacing {
        Some(cpa) => {
            let target = (cpa.time_to_cpa_s.max(0.0) * PACE_FRACTION).max(0.0);
            clamp(Duration::from_secs_f64(target), bounds)
        }
        None => bounds.max,
    }
}

fn clamp(d: Duration, bounds: PollBounds) -> Duration {
    d.clamp(bounds.min, bounds.max)
}

/// Exponential backoff state for consecutive poll failures.
#[derive(Debug, Default)]
struct Backoff {
    failures: u32,
}

impl Backoff {
    fn reset(&mut self) {
        self.failures = 0;
    }

    /// The delay to wait after a failure. An explicit `Retry-After` wins (clamped
    /// to a sane ceiling); otherwise `min · 2^(failures-1)`, capped at `max`.
    fn next_delay(&mut self, error: &SourceError, bounds: PollBounds) -> Duration {
        self.failures = self.failures.saturating_add(1);

        if let SourceError::RateLimited {
            retry_after: Some(d),
        } = error
        {
            // Clamp into `[min, ceiling]`, but never let the floor exceed the
            // ceiling: a misconfigured `source.min_interval_ms` above the ceiling
            // would otherwise make `Duration::clamp` panic (min > max).
            let lo = bounds.min.min(RETRY_AFTER_CEILING);
            return (*d).clamp(lo, RETRY_AFTER_CEILING);
        }

        let factor = 2u32.saturating_pow(self.failures - 1);
        clamp(bounds.min.saturating_mul(factor), bounds)
    }
}

/// Spawn the poller thread, writing into the shared [`Tracker`]. Returns its join
/// handle and a shutdown handle; drop the shutdown handle (or signal it) to stop
/// the thread between polls.
pub fn spawn(
    source: Box<dyn FlightSource>,
    area: SearchArea,
    bounds: PollBounds,
    tracker: Arc<RwLock<Tracker>>,
) -> (JoinHandle<()>, Shutdown) {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        run(source, area, bounds, tracker, &stop_rx);
    });
    (handle, Shutdown { _tx: stop_tx })
}

/// Dropping this (or signalling it) wakes the poller and ends its loop.
pub struct Shutdown {
    _tx: Sender<()>,
}

fn run(
    source: Box<dyn FlightSource>,
    area: SearchArea,
    bounds: PollBounds,
    tracker: Arc<RwLock<Tracker>>,
    stop_rx: &Receiver<()>,
) {
    let mut backoff = Backoff::default();

    loop {
        let delay = match source.fetch(&area) {
            Ok(snapshot) => {
                backoff.reset();
                let now = snapshot.taken_at;
                // Ingest under the write lock, then drop it before the (cloning)
                // pacing computation so handler threads aren't blocked on it.
                write(&tracker).ingest(snapshot);
                let pacing = read(&tracker).pacing_at(now).and_then(|t| t.cpa);
                schedule(pacing, bounds)
            }
            Err(error) => {
                let retry_in = backoff.next_delay(&error, bounds);
                write(&tracker).note_error(error.to_string());
                retry_in
            }
        };

        match stop_rx.recv_timeout(delay) {
            Err(RecvTimeoutError::Timeout) => continue, // time to poll again
            _ => break,                                 // shutdown requested or channel closed
        }
    }
}

/// Take the Tracker's write lock, recovering from a poisoned lock so one panicked
/// handler thread can't wedge the poller. The data stays coherent: a single
/// `ingest`/`note_error` either fully applied or did not.
fn write(tracker: &RwLock<Tracker>) -> RwLockWriteGuard<'_, Tracker> {
    tracker.write().unwrap_or_else(|e| e.into_inner())
}

fn read(tracker: &RwLock<Tracker>) -> RwLockReadGuard<'_, Tracker> {
    tracker.read().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> PollBounds {
        PollBounds {
            min: Duration::from_secs(1),
            max: Duration::from_secs(60),
        }
    }

    #[test]
    fn quiet_airspace_polls_at_the_max_interval() {
        assert_eq!(schedule(None, bounds()), Duration::from_secs(60));
    }

    #[test]
    fn imminent_cpa_clamps_to_the_source_floor() {
        // CPA 2 s away ⇒ target 0.5 s ⇒ clamped up to the 1 s floor.
        let cpa = Cpa {
            time_to_cpa_s: 2.0,
            cpa_distance_nm: 1.0,
        };
        assert_eq!(schedule(Some(cpa), bounds()), Duration::from_secs(1));
    }

    #[test]
    fn distant_cpa_clamps_to_the_max_interval() {
        // CPA 1 hour away ⇒ target 900 s ⇒ clamped down to the 60 s max.
        let cpa = Cpa {
            time_to_cpa_s: 3600.0,
            cpa_distance_nm: 5.0,
        };
        assert_eq!(schedule(Some(cpa), bounds()), Duration::from_secs(60));
    }

    #[test]
    fn mid_range_cpa_scales_with_time_to_cpa() {
        // CPA 80 s away ⇒ target 20 s, within [1, 60].
        let cpa = Cpa {
            time_to_cpa_s: 80.0,
            cpa_distance_nm: 5.0,
        };
        assert_eq!(schedule(Some(cpa), bounds()), Duration::from_secs(20));
    }

    #[test]
    fn backoff_doubles_then_caps_and_resets() {
        let mut b = Backoff::default();
        let seq: Vec<u64> = (0..8)
            .map(|_| b.next_delay(&SourceError::Transient, bounds()).as_secs())
            .collect();
        assert_eq!(seq, vec![1, 2, 4, 8, 16, 32, 60, 60]);

        b.reset();
        assert_eq!(b.next_delay(&SourceError::Transient, bounds()).as_secs(), 1);
    }

    #[test]
    fn backoff_honors_retry_after_within_ceiling() {
        let mut b = Backoff::default();
        let d = b.next_delay(
            &SourceError::RateLimited {
                retry_after: Some(Duration::from_secs(12)),
            },
            bounds(),
        );
        assert_eq!(d, Duration::from_secs(12));

        // Absurd guidance is clamped to the ceiling.
        let d = b.next_delay(
            &SourceError::RateLimited {
                retry_after: Some(Duration::from_secs(9999)),
            },
            bounds(),
        );
        assert_eq!(d, RETRY_AFTER_CEILING);
    }

    #[test]
    fn retry_after_with_a_floor_above_the_ceiling_does_not_panic() {
        // A (mis)configured source min_interval above RETRY_AFTER_CEILING once made
        // the retry-after clamp panic (min > max). It must instead pin to the
        // ceiling rather than abort the poller.
        let bounds = PollBounds {
            min: Duration::from_secs(600), // above the 300 s ceiling
            max: Duration::from_secs(600),
        };
        let mut b = Backoff::default();
        let d = b.next_delay(
            &SourceError::RateLimited {
                retry_after: Some(Duration::from_secs(30)),
            },
            bounds,
        );
        assert_eq!(d, RETRY_AFTER_CEILING);
    }
}
