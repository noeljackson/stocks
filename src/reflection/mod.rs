//! Reflection — forward-only validation (SPEC §3 FR9, §9, #23).
//!
//! The kill-criterion's measurement layer. Records predictions when a thesis
//! becomes actionable; scores outcomes when it's fulfilled or invalidated.
//! Computes Brier (forecast calibration) and lead-time-to-consensus.

pub mod scoring;
pub mod service;
pub mod technical_timing;
