//! International Standard Atmosphere (ISA), troposphere only (below 11 km).
//!
//! "Pressure altitude" is the altitude at which the standard atmosphere has a given
//! pressure. It differs from true altitude depending on the weather, but its *changes*
//! over short times are accurate, which is what a variometer needs. IGC files store it
//! next to the GPS altitude.

/// Sea level pressure of the standard atmosphere in pascal.
pub const SEA_LEVEL_PRESSURE_PA: f64 = 101_325.0;

const SEA_LEVEL_TEMPERATURE_K: f64 = 288.15;
const LAPSE_RATE_K_PER_M: f64 = 0.0065;
const GRAVITY_MPS2: f64 = 9.806_65;
const GAS_CONSTANT_AIR: f64 = 287.052_87;

/// Exponent of the barometric formula, about 0.1903.
const EXPONENT: f64 = GAS_CONSTANT_AIR * LAPSE_RATE_K_PER_M / GRAVITY_MPS2;

/// Pressure altitude in metres for a static pressure in pascal.
pub fn pressure_to_altitude(pressure_pa: f64) -> f64 {
    SEA_LEVEL_TEMPERATURE_K / LAPSE_RATE_K_PER_M
        * (1.0 - (pressure_pa / SEA_LEVEL_PRESSURE_PA).powf(EXPONENT))
}

/// Static pressure in pascal at a pressure altitude in metres.
pub fn altitude_to_pressure(altitude_m: f64) -> f64 {
    SEA_LEVEL_PRESSURE_PA
        * (1.0 - LAPSE_RATE_K_PER_M * altitude_m / SEA_LEVEL_TEMPERATURE_K).powf(1.0 / EXPONENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sea_level_is_zero_altitude() {
        assert!(pressure_to_altitude(SEA_LEVEL_PRESSURE_PA).abs() < 1e-9);
        assert!((altitude_to_pressure(0.0) - SEA_LEVEL_PRESSURE_PA).abs() < 1e-9);
    }

    #[test]
    fn matches_published_isa_table() {
        // ISA table values: 1000 m -> 898.75 hPa, 3000 m -> 701.09 hPa.
        assert!((altitude_to_pressure(1000.0) - 89_875.0).abs() < 5.0);
        assert!((altitude_to_pressure(3000.0) - 70_109.0).abs() < 5.0);
    }

    #[test]
    fn conversions_are_inverse() {
        for altitude in [-300.0, 0.0, 587.0, 1500.0, 4000.0, 8000.0] {
            let back = pressure_to_altitude(altitude_to_pressure(altitude));
            assert!((back - altitude).abs() < 1e-6, "{altitude} -> {back}");
        }
    }
}
