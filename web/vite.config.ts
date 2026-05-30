import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Build the SPA into web/dist/, baked into the Rust gateway binary at compile
// time via rust-embed (see src/web/mod.rs).
//
// Dev modes:
//   - Host:       `make web-dev` → API_TARGET defaults to localhost:8080
//   - All-docker: `make dev`     → API_TARGET=http://gateway:8080 (compose service name)
const apiTarget = process.env.API_TARGET ?? "http://localhost:8080";

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    // Listen on all interfaces inside containers so the host browser can reach HMR.
    host: true,
    strictPort: true,
    port: 5173,
    proxy: {
      "/api": { target: apiTarget, changeOrigin: true, ws: true },
      "/healthz": apiTarget,
    },
  },
});
