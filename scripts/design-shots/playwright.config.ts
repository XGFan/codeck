import { defineConfig, devices } from "@playwright/test";

// Screenshots of the current UI (mock mode) for the 「现状」 pages of the CoDeck design canvas:
// `pnpm design:shots` writes target/design-shots/*.png and manifest.json (see CLAUDE.md).
const port = Number(process.env.E2E_PORT ?? 1422);

export default defineConfig({
  testDir: ".",
  outputDir: "../../target/design-shots/.results",
  workers: 1,
  use: { baseURL: `http://localhost:${port}`, ...devices["Desktop Safari"], viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 },
  webServer: {
    command: `pnpm dev:mock --port ${port}`,
    url: `http://localhost:${port}`,
    reuseExistingServer: !process.env.CI,
  },
});
