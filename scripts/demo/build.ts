// Builds the demo data root (demo/root) from the stories in demo/story. Same input, same output.
//
//   node scripts/demo/build.ts

import fs from "node:fs";
import path from "node:path";
import { forkOf } from "../../demo/story/fork.ts";
import { mainSession } from "../../demo/story/main.ts";
import { teamSession } from "../../demo/story/team.ts";
import { workflowSession } from "../../demo/story/workflow.ts";

const ROOT = path.resolve(import.meta.dirname, "../../demo/root");

fs.rmSync(ROOT, { recursive: true, force: true });
const main = mainSession();
const sessions = [main, forkOf(main), teamSession(), workflowSession()];
for (const s of sessions) s.write(ROOT);
console.log(`demo/root: ${sessions.length} sessions`);
