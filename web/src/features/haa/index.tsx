import { useEffect } from "react";
import { useHaa } from "./store";

export { HaaBar } from "./HaaBar";
export { useHaa } from "./store";

/** haa Worker status is desktop only: not on the daemon's pages (`VITE_IPC=remote`). */
export const haaAvailable = import.meta.env.VITE_IPC !== "remote";

/** Runs the desktop haa watch while `wanted`: the open Session is Live or connected. Mount once. */
export function HaaRoot({ wanted }: { wanted: boolean }) {
  const start = useHaa((s) => s.start);
  const setWanted = useHaa((s) => s.setWanted);
  useEffect(() => start(), [start]);
  useEffect(() => setWanted(wanted), [wanted, setWanted]);
  return null;
}
