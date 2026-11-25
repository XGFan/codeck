import { defineConfig, devices } from "@playwright/test";

// E2E_PORT lets several local runs (e.g. parallel worktrees/agents) use separate servers.
const port = Number(process.env.E2E_PORT ?? 1421);
// The mock app is built once and served as static files. A dev server transforms modules on demand in one
// process: every worker's first page waited ~13 s for it, and it slowed all of them as workers were added.
// One directory per port, so two runs in the same checkout do not overwrite each other's build.
const dist = `dist-e2e/${port}`;

export default defineConfig({
  testDir: "e2e",
  // e2e/host is the real-backend E2E (playwright.host.config.ts).
  testIgnore: "host/**",
  // Beside this file: the default is beside package.json, at the repository root.
  outputDir: "test-results",
  // Tests of one file run in parallel too: each has its own page and the mock backend lives in the page.
  fullyParallel: true,
  use: { baseURL: `http://localhost:${port}` },
  projects: [{ name: "webkit", use: { ...devices["Desktop Safari"] } }],
  webServer: {
    command: `pnpm exec vite build --mode mock --outDir ${dist} --logLevel error && pnpm exec vite preview --mode mock --outDir ${dist} --port ${port} --strictPort`,
    url: `http://localhost:${port}`,
    // A server already on the port is used as it is (a `pnpm dev:mock --port ${port}` left running included).
    reuseExistingServer: !process.env.CI,
  },
});
