//! Pure spherical geometry: great-circle ground distance, bearing, destination
//! projection (for dead reckoning), and closest point of approach (CPA).
//!
//! All distances are nautical miles, all angles degrees clockwise from true
//! north, all speeds knots. Nothing here knows about flights, sources, or time
//! bases — it is hand-rolled (no `geo` crate, per the implementation plan) and
//! exhaustively unit-tested below.

use crate::domain::LatLon;

/// Earth radius in nautical miles, fixed so that one degree of latitude is
/// exactly 60 nm. This keeps [`haversine_nm`] consistent with the local-plane
/// approximation used in [`cpa`], so the two never disagree at short range.
pub const EARTH_RADIUS_NM: f64 = 60.0 * 180.0 / std::f64::consts::PI; // ≈ 3437.7468

/// One degree of latitude is 60 nm everywhere; one degree of longitude shrinks
/// by the cosine of the latitude.
const NM_PER_DEG_LAT: f64 = 60.0;

/// Great-circle **ground** distance between two points, in nautical miles.
/// Altitude plays no part — this is the horizontal measure the "nearest flight"
/// is ranked by.
pub fn haversine_nm(a: LatLon, b: LatLon) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = (b.lat - a.lat).to_radians();
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_NM * h.sqrt().atan2((1.0 - h).sqrt())
}

/// Initial great-circle bearing from `from` to `to`, in degrees clockwise from
/// true north, normalized to `[0, 360)`.
pub fn bearing_deg(from: LatLon, to: LatLon) -> f64 {
    let (lat1, lat2) = (from.lat.to_radians(), to.lat.to_radians());
    let dlon = (to.lon - from.lon).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// The destination reached by travelling `distance_nm` from `from` along a
/// constant `bearing_deg`. This is the dead-reckoning step: advance a flight's
/// last reported position along its track at its groundspeed.
pub fn project(from: LatLon, bearing_deg: f64, distance_nm: f64) -> LatLon {
    let ang = distance_nm / EARTH_RADIUS_NM; // angular distance (radians)
    let brg = bearing_deg.to_radians();
    let lat1 = from.lat.to_radians();
    let lon1 = from.lon.to_radians();

    let sin_lat2 = lat1.sin() * ang.cos() + lat1.cos() * ang.sin() * brg.cos();
    let lat2 = sin_lat2.asin();
    let y = brg.sin() * ang.sin() * lat1.cos();
    let x = ang.cos() - lat1.sin() * sin_lat2;
    let lon2 = lon1 + y.atan2(x);

    LatLon {
        lat: lat2.to_degrees(),
        lon: normalize_lon(lon2.to_degrees()),
    }
}

/// Wrap a longitude into `[-180, 180)`.
fn normalize_lon(lon: f64) -> f64 {
    ((lon + 540.0) % 360.0) - 180.0
}

/// The **closest point of approach** of a flight to Home, assuming it holds its
/// current course and speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cpa {
    /// Seconds until the closest pass. **Negative** means the closest pass has
    /// already happened — the flight is *receding*. Zero or positive means it is
    /// *approaching*.
    pub time_to_cpa_s: f64,
    /// How close the flight will get (or, when receding, how close it got at the
    /// pass), in nautical miles.
    pub cpa_distance_nm: f64,
}

impl Cpa {
    /// An approaching flight's closest pass is still in the future.
    pub fn is_approaching(&self) -> bool {
        self.time_to_cpa_s >= 0.0
    }
}

/// Compute the CPA of a moving flight relative to Home, working in a local
/// east/north plane centred on Home (good to a fraction of a percent across a
/// few-hundred-nm Search area, and exact enough for an estimate refreshed every
/// poll — ADR-0002).
///
/// `track_deg`/`gs_kt` are the flight's current velocity; the caller must only
/// pass a usably-moving flight (see [`crate::domain::Flight::velocity`]). A
/// zero-velocity input degenerates to "already at its closest", i.e. time 0 and
/// the current distance.
pub fn cpa(home: LatLon, pos: LatLon, track_deg: f64, gs_kt: f64) -> Cpa {
    // Flight position relative to Home, in nm (x = east, y = north).
    let nm_per_deg_lon = NM_PER_DEG_LAT * home.lat.to_radians().cos();
    let rx = (pos.lon - home.lon) * nm_per_deg_lon;
    let ry = (pos.lat - home.lat) * NM_PER_DEG_LAT;

    // Velocity in nm/hour, decomposed from the compass track.
    let trk = track_deg.to_radians();
    let vx = gs_kt * trk.sin();
    let vy = gs_kt * trk.cos();

    let vv = vx * vx + vy * vy;
    if vv == 0.0 {
        return Cpa {
            time_to_cpa_s: 0.0,
            cpa_distance_nm: (rx * rx + ry * ry).sqrt(),
        };
    }

    // Minimise |r + v·t| over t:  t* = -(r·v) / (v·v).
    let t_hours = -(rx * vx + ry * vy) / vv;
    let cx = rx + vx * t_hours;
    let cy = ry + vy * t_hours;

    Cpa {
        time_to_cpa_s: t_hours * 3600.0,
        cpa_distance_nm: (cx * cx + cy * cy).sqrt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: LatLon = LatLon::new(0.0, 0.0);

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn haversine_one_degree_is_sixty_nm() {
        // One degree of latitude is 60 nm by construction of EARTH_RADIUS_NM.
        assert!(approx(
            haversine_nm(HOME, LatLon::new(1.0, 0.0)),
            60.0,
            1e-6
        ));
        // One degree of longitude at the equator is also 60 nm.
        assert!(approx(
            haversine_nm(HOME, LatLon::new(0.0, 1.0)),
            60.0,
            1e-6
        ));
    }

    #[test]
    fn haversine_is_symmetric_and_zero_on_identity() {
        let a = LatLon::new(51.47, -0.45);
        let b = LatLon::new(48.85, 2.35);
        assert!(approx(haversine_nm(a, b), haversine_nm(b, a), 1e-9));
        assert!(approx(haversine_nm(a, a), 0.0, 1e-9));
    }

    #[test]
    fn bearing_cardinal_directions() {
        assert!(approx(bearing_deg(HOME, LatLon::new(1.0, 0.0)), 0.0, 1e-6)); // north
        assert!(approx(bearing_deg(HOME, LatLon::new(0.0, 1.0)), 90.0, 1e-6)); // east
        assert!(approx(
            bearing_deg(HOME, LatLon::new(-1.0, 0.0)),
            180.0,
            1e-6
        )); // south
        assert!(approx(
            bearing_deg(HOME, LatLon::new(0.0, -1.0)),
            270.0,
            1e-6
        )); // west
    }

    #[test]
    fn project_round_trips_through_distance_and_bearing() {
        let start = LatLon::new(34.05, -118.25);
        for &(brg, dist) in &[(0.0, 60.0), (90.0, 30.0), (217.0, 120.0), (315.0, 5.0)] {
            let dest = project(start, brg, dist);
            assert!(
                approx(haversine_nm(start, dest), dist, 1e-3),
                "distance mismatch for bearing {brg}"
            );
            assert!(
                approx(bearing_deg(start, dest), brg, 1e-2),
                "bearing mismatch for bearing {brg}"
            );
        }
    }

    #[test]
    fn cpa_overhead_pass_hits_zero_at_the_right_time() {
        // 10 nm due north, flying due south at 360 kt: passes overhead in
        // 10/360 h = 100 s, closest distance ~0.
        let pos = LatLon::new(10.0 / 60.0, 0.0);
        let c = cpa(HOME, pos, 180.0, 360.0);
        assert!(c.is_approaching());
        assert!(
            approx(c.time_to_cpa_s, 100.0, 0.5),
            "t = {}",
            c.time_to_cpa_s
        );
        assert!(
            approx(c.cpa_distance_nm, 0.0, 1e-3),
            "d = {}",
            c.cpa_distance_nm
        );
    }

    #[test]
    fn cpa_receding_flight_has_negative_time() {
        // 10 nm due north, flying due *north* (away): closest pass is behind it.
        let pos = LatLon::new(10.0 / 60.0, 0.0);
        let c = cpa(HOME, pos, 0.0, 360.0);
        assert!(!c.is_approaching());
        assert!(c.time_to_cpa_s < 0.0);
    }

    #[test]
    fn cpa_tangential_flight_is_at_its_closest_now() {
        // 10 nm due north, flying due east: moving tangentially, so the closest
        // point is right now (t ≈ 0) at the current 10 nm.
        let pos = LatLon::new(10.0 / 60.0, 0.0);
        let c = cpa(HOME, pos, 90.0, 420.0);
        assert!(approx(c.time_to_cpa_s, 0.0, 1e-6));
        assert!(approx(c.cpa_distance_nm, 10.0, 1e-3));
    }

    #[test]
    fn cpa_offset_inbound_misses_by_the_offset() {
        // Flight 50 nm south and 8 nm east, flying due north: it will pass 8 nm
        // to the east of Home (its eastward offset never changes).
        let pos = LatLon::new(-50.0 / 60.0, 8.0 / 60.0);
        let c = cpa(HOME, pos, 0.0, 480.0);
        assert!(c.is_approaching());
        assert!(
            approx(c.cpa_distance_nm, 8.0, 1e-2),
            "d = {}",
            c.cpa_distance_nm
        );
    }
}
