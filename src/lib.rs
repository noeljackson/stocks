//! stocks — thesis-driven trading intelligence (library entry).
//!
//! Crate layout:
//! - [`platform`] — shared infra (config, NATS bus, Postgres store, subjects,
//!   domain types, logging).
//! - [`ingest`]   — adapter framework + EDGAR / FRED adapters.
//! - [`llm`]      — provider abstraction (mock | anthropic | openai_compat).
//! - [`regime`], [`router`], [`risk`], [`goalpost`] — per-service business
//!   logic; each one a pure module and a service wrapper.
//! - [`gateway`]  — HTTP + SSE; serves the embedded SPA from [`web`].
//! - [`web`]      — `rust-embed` of the built Svelte SPA.

pub mod platform;
pub mod ingest;
pub mod llm;
pub mod regime;
pub mod router;
pub mod risk;
pub mod goalpost;
pub mod reflection;
pub mod thesis;
pub mod gateway;
pub mod web;
