import type { ProjectSummary } from "@/ipc/bindings";

/** `sessionCount` / `liveCount` / `lastActiveMs` are derived from the session list by the mock. */
export const projectBase = [
  {
    id: "-Users-dev-Developer-lumen-api",
    path: "/Users/dev/Developer/lumen-api",
    displayName: "lumen-api",
    missing: false,
  },
  {
    id: "-Users-dev-Developer-orbit-web",
    path: "/Users/dev/Developer/orbit-web",
    displayName: "orbit-web",
    missing: false,
  },
  {
    id: "-Users-dev-Developer-homelab-infra",
    path: "/Users/dev/Developer/homelab-infra",
    displayName: "homelab-infra",
    missing: true,
  },
] satisfies Pick<ProjectSummary, "id" | "path" | "displayName" | "missing">[];

export const [LUMEN, ORBIT, HOMELAB] = projectBase.map((p) => p.id) as [string, string, string];
