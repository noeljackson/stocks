//! Thesis-level integrity: substance gates, condition evaluation (later),
//! state-machine enforcement (later). The goalpost detector lives in its own
//! crate-level module for historical reasons; expect that to move under here
//! when the state-machine work (#15) lands.

pub mod evaluator;
pub mod staleness;
pub mod substance;
