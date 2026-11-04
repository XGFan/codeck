import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
  worker: { format: "es" },
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
  server: { port: 1430, strictPort: true },
  test: { include: ["src/**/*.test.ts", "src/**/*.test.tsx"] },
});
