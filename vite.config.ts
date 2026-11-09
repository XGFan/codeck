import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
  worker: { format: "es" },
  // Imported only by the Shiki worker, which the startup dependency scan does not follow: found at
  // run time instead, they make Vite re-optimize and reload every open page.
  optimizeDeps: { include: ["shiki/core", "shiki/engine/javascript", "shiki/langs", "shiki/themes"] },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
  server: { port: 1420, strictPort: true },
  // `--mode host`: the iPhone app'"'"'s transcript page, served by the daemon under /web/.
  ...(mode === "host" && {
    base: "/web/",
    build: { outDir: "dist-web", rollupOptions: { input: path.resolve(import.meta.dirname, "transcript.html") } },
  }),
  test: { include: ["src/**/*.test.ts", "src/**/*.test.tsx"] },
}));
