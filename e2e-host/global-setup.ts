import { type ChildProcess, execSync, spawn } from "node:child_process";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const port = Number(process.env.E2E_HOST_PORT ?? 38497);

export const SESSION_ID = "86d20d64-9fdd-5e64-aae4-93895b13068d";
export const SESSION_FILE = `projects/-Users-dev-code-lumen-api/${SESSION_ID}.jsonl`;

async function waitHealth(child: ChildProcess) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`codeck-host exited early (${child.exitCode})`);
    try {
      if ((await fetch(`http://127.0.0.1:${port}/health`)).ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error("codeck-host /health did not come up");
}

export default async function globalSetup() {
  execSync("cargo build -p codeck-daemon", { cwd: root, stdio: "inherit", timeout: 20 * 60_000 });
  execSync("pnpm build:web", { cwd: root, stdio: "inherit", timeout: 5 * 60_000 });

  const tmp = mkdtempSync(path.join(tmpdir(), "codeck-e2e-host-"));
  const dataRoot = path.join(tmp, "data-root");
  cpSync(path.join(root, "crates/codeck-core/tests/fixtures/basic"), dataRoot, { recursive: true });
  process.env.E2E_HOST_DATA_ROOT = dataRoot;

  const child = spawn(
    path.join(root, "target/debug/codeck-host"),
    [
      ...["--port", String(port), "--listen", "127.0.0.1"],
      ...["--state-dir", path.join(tmp, "state")],
      ...["--data-root", dataRoot],
      ...["--web-dir", path.join(root, "dist-web")],
    ],
    { stdio: "inherit" },
  );
  try {
    await waitHealth(child);
  } catch (e) {
    child.kill();
    rmSync(tmp, { recursive: true, force: true });
    throw e;
  }

  return async () => {
    child.kill();
    rmSync(tmp, { recursive: true, force: true });
  };
}
