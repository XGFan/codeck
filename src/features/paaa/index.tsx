import { useEffect } from "react";
import { usePaaa } from "./store";

export { PaaaBar } from "./PaaaBar";
export { usePaaa } from "./store";

/** paaa Worker status is desktop only: not on the daemon's pages (`VITE_IPC=remote`). */
export const paaaAvailable = import.meta.env.VITE_IPC !== "remote";

/** Runs the desktop paaa watch while `wanted`: the open Session is Live or connected. Mount once. */
export function PaaaRoot({ wanted }: { wanted: boolean }) {
  const start = usePaaa((s) => s.start);
  const setWanted = usePaaa((s) => s.setWanted);
  useEffect(() => start(), [start]);
  useEffect(() => setWanted(wanted), [wanted, setWanted]);
  return null;
}
