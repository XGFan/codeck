import type { LsEntry } from "./protocol";

/**
 * Keeps `ccrpc ls --json --watch` running while it is wanted (`setWanted`) and `canWatch` says so: started (it sends
 * the list at once, then again on every change, `list`), stopped otherwise. A changed list goes to `onLs`; a watch
 * that cannot start or ends by itself (`ended`) to `onError`, after which `canWatch` is asked again.
 */
export function createLsWatch(o: {
  watch: (on: boolean) => Promise<void>;
  onLs: (ls: LsEntry[]) => void;
  onError: (e: unknown) => void;
  canWatch: () => boolean;
}) {
  let watching = false;
  let wanted = false;
  let last = "";

  const stop = () => {
    if (!watching) return;
    watching = false;
    void o.watch(false).catch(() => {});
  };
  /** Watches while wanted and allowed; stops otherwise. */
  const sync = () => {
    const go = wanted && o.canWatch();
    if (go && !watching) {
      watching = true;
      o.watch(true).catch((e) => {
        if (!watching) return;
        watching = false;
        o.onError(e);
        sync();
      });
    } else if (!go && watching) {
      stop();
    }
  };

  return {
    sync,
    stop,
    /** Whether the list is worth watching; not wanted any more, what it said last is stale. */
    setWanted(w: boolean) {
      if (w === wanted) return sync();
      wanted = w;
      if (!w) {
        last = "";
        o.onLs([]);
      }
      sync();
    },
    /** A list from the watch (dropped while not watching: it was sent before the stop). */
    list(ls: LsEntry[]) {
      if (!watching) return;
      const text = JSON.stringify(ls);
      if (text !== last) {
        last = text;
        o.onLs(ls);
      }
    },
    /** The watch ended by itself. */
    ended(message: string) {
      if (!watching) return;
      watching = false;
      o.onError(new Error(message));
      sync();
    },
  };
}
