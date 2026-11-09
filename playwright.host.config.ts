import { defineConfig, devices } from "@playwright/test";

// Real-backend E2E: the daemon (codeck-host) serves dist-web/ and a copy of a fixture data root.
const port = Number(process.env.E2E_HOST_PORT ?? 38497);

export default defineConfig({
  testDir: "e2e-host",
  testMatch: "*.spec.ts",
  workers: 1,
  globalSetup: "./e2e-host/global-setup.ts",
  use: { baseURL: `http://127.0.0.1:${port}` },
  projects: [{ name: "webkit", use: { ...devices["Desktop Safari"] } }],
});
