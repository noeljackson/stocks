//! Shared infrastructure: NATS bus, Postgres store, env-driven config,
//! NATS subject + stream constants, domain types, structured logging.

pub mod brain;
pub mod bus;
pub mod config;
pub mod domain;
pub mod logging;
pub mod market_calendar;
pub mod store;
pub mod subjects;
pub mod technical;
