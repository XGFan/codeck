import type { LsEntry } from "./protocol";

const POLL_MS = 2000;

/**
 * Polls `ccrpc ls` while it is wanted (`setWanted`) and `canPoll` says so: one call at a time, the next
 * `POLL_MS` after the answer; when it starts again it asks at once. A changed answer goes to `onLs`; a failure
 * to `onError`, after which `canPoll` is asked again.
 */
export function createLsPoller(o: {
  list: () => Promise<LsEntry[]>;
  onLs: (ls: LsEntry[]) => void;
  onError: (e: unknown) => void;
  canPoll: () => boolean;
}) {
  let timer: number | undefined;
  let polling = false;
  /** Bumped whenever polling stops: a `ccrpc ls` answer from an older generation is dropped. */
  let gen = 0;
  let last = "";
  let wanted = false;

  const stop = () => {
    polling = false;
    gen++;
    window.clearTimeout(timer);
    timer = undefined;
  };
  const poll = async (g: number) => {
    try {
      const ls = await o.list();
      if (g !== gen) return;
      const text = JSON.stringify(ls);
      if (text !== last) {
        last = text;
        o.onLs(ls);
      }
    } catch (e) {
      if (g !== gen) return;
      o.onError(e);
      return sync();
    }
    timer = window.setTimeout(() => void poll(g), POLL_MS);
  };
  /** Polls while wanted and allowed; stops otherwise. */
  const sync = () => {
    const go = wanted && o.canPoll();
    if (go && !polling) {
      polling = true;
      void poll(++gen);
    } else if (!go && polling) {
      stop();
    }
  };

  return {
    sync,
    stop,
    /** Whether `ccrpc ls` is worth polling; not wanted any more, what it said last is stale. */
    setWanted(w: boolean) {
      if (w === wanted) return sync();
      wanted = w;
      if (!w) {
        last = "";
        o.onLs([]);
      }
      sync();
    },
    /** Drops an answer under way and asks again at once (when polling). */
    restart() {
      if (polling) {
        stop();
        sync();
      }
    },
  };
}
