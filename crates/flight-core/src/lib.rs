//! Platform independent core of the flight computer.
//!
//! This crate must not depend on any UI, operating system or audio API. Time never comes
//! from the system clock: every sensor sample carries its own [`time::Timestamp`], which
//! keeps replay, accelerated simulation and tests deterministic.

pub mod atmosphere;
pub mod geo;
pub mod igc;
pub mod sensor;
pub mod time;
pub mod track;
