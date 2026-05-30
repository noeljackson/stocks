//! Embedded SPA: `web/dist` is built by Vite (Svelte 5) and baked into the
//! gateway binary at compile time via `rust-embed`. The placeholder
//! `web/dist/index.html` lets the crate compile before the first
//! `npm run build`.

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist"]
pub struct Dist;
