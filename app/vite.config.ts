import { resolve } from "node:path";
import { defineConfig } from "vite";

// The office and the card are two pages of one Vite site, each loaded by its own Tauri window.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "safari16",
    minify: "esbuild",
    sourcemap: false,
    rollupOptions: {
      input: {
        office: resolve(import.meta.dirname, "index.html"),
        card: resolve(import.meta.dirname, "card.html"),
      },
    },
  },
});
