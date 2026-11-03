import { defineConfig, devices } from "@playwright/test";

// E2E_PORT lets several local runs (e.g. parallel worktrees/agents) use separate dev servers.
const port = Number(process.env.E2E_PORT ?? 1431);

export default defineConfig({
  testDir: "e2e",
  use: { baseURL: `http://localhost:${port}` },
  projects: [{ name: "webkit", use: { ...devices["Desktop Safari"] } }],
  webServer: {
    command: `pnpm dev:mock --port ${port}`,
    url: `http://localhost:${port}`,
    reuseExistingServer: !process.env.CI,
  },
});
