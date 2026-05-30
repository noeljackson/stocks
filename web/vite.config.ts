import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Build the SPA into web/dist/, baked into the Rust gateway binary at compile
// time via rust-embed (see src/web/mod.rs).
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    // Dev: proxy API + SSE to the gateway.
    proxy: {
      "/api": { target: "http://localhost:8080", changeOrigin: true },
      "/healthz": "http://localhost:8080",
    },
  },
});
