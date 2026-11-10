//! Small geodesy helpers on a spherical earth. Accurate to a fraction of a percent, which
//! is plenty for ground speed, course and map display.

/// Mean earth radius in metres.
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Great-circle distance in metres between two positions given in degrees.
pub fn distance_m(lat1_deg: f64, lon1_deg: f64, lat2_deg: f64, lon2_deg: f64) -> f64 {
    let (lat1, lat2) = (lat1_deg.to_radians(), lat2_deg.to_radians());
    let d_lat = lat2 - lat1;
    let d_lon = (lon2_deg - lon1_deg).to_radians();
    // Haversine formula: numerically stable for the short distances we deal with.
    let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Initial course in degrees (0 = north, 90 = east, range 0..360) from the first position
/// to the second.
pub fn bearing_deg(lat1_deg: f64, lon1_deg: f64, lat2_deg: f64, lon2_deg: f64) -> f64 {
    let (lat1, lat2) = (lat1_deg.to_radians(), lat2_deg.to_radians());
    let d_lon = (lon2_deg - lon1_deg).to_radians();
    let y = d_lon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * d_lon.cos();
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

/// Position reached by moving `north_m` and `east_m` metres away from a position.
/// Returns `(latitude_deg, longitude_deg)`. Uses a flat-earth approximation that is fine
/// for offsets of a few tens of kilometres away from the poles.
pub fn offset_position(lat_deg: f64, lon_deg: f64, north_m: f64, east_m: f64) -> (f64, f64) {
    let lat = lat_deg + (north_m / EARTH_RADIUS_M).to_degrees();
    let lon = lon_deg + (east_m / (EARTH_RADIUS_M * lat_deg.to_radians().cos())).to_degrees();
    (lat, lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_degree_of_latitude_is_about_111_km() {
        let d = distance_m(47.0, 11.0, 48.0, 11.0);
        assert!((d - 111_195.0).abs() < 10.0, "{d}");
    }

    #[test]
    fn bearings_of_cardinal_directions() {
        assert!((bearing_deg(47.0, 11.0, 48.0, 11.0) - 0.0).abs() < 1e-6);
        assert!((bearing_deg(47.0, 11.0, 46.0, 11.0) - 180.0).abs() < 1e-6);
        assert!((bearing_deg(0.0, 11.0, 0.0, 12.0) - 90.0).abs() < 1e-6);
        assert!((bearing_deg(0.0, 11.0, 0.0, 10.0) - 270.0).abs() < 1e-6);
    }

    #[test]
    fn offset_matches_distance_and_bearing() {
        let (lat, lon) = offset_position(47.0, 11.0, 300.0, 400.0);
        let d = distance_m(47.0, 11.0, lat, lon);
        let b = bearing_deg(47.0, 11.0, lat, lon);
        assert!((d - 500.0).abs() < 0.5, "{d}");
        assert!((b - 53.13).abs() < 0.05, "{b}");
    }
}
