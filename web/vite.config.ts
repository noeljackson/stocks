import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Build the SPA straight into the Go embed dir (SPEC §11: single self-contained binary).
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "../internal/web/dist",
    emptyOutDir: true,
  },
  server: {
    // Dev: proxy API + SSE to the Go gateway.
    proxy: {
      "/api": { target: "http://localhost:8080", changeOrigin: true },
      "/healthz": "http://localhost:8080",
    },
  },
});
