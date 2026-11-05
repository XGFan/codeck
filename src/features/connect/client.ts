// The command / event plumbing of the ccrpc connections: commands waiting for their response, events held for a
// connection id not known yet, and waiting for a connection's ccrpc to exit. Connection state is the store's.
import type { Api } from "@/ipc";
import { errorText } from "@/lib/errors";
import { createEarlyBuffer, type Early } from "./early";
import type { ResponseMsg } from "./protocol";

export interface Timers {
  setTimeout(f: () => void, ms: number): unknown;
}

const windowTimers: Timers = { setTimeout: (f, ms) => window.setTimeout(f, ms) };

interface Pending {
  conn: string;
  resolve: (r: ResponseMsg) => void;
}

export function createCcrpcClient(api: Pick<Api, "ccrpcSend" | "ccrpcGuiLog">, timers: Timers = windowTimers) {
  const pending = new Map<string, Pending>();
  /** Waiting for a connection's ccrpc to exit (reconnect: one ccrpc per Session at a time). */
  const exitWaiters = new Map<string, () => void>();
  /** Events of connections whose id has not come back from `ccrpcConnect` / `ccrpcNew` yet. */
  const early = createEarlyBuffer();
  /** Connections already removed: their late events are dropped, not held. */
  const removed = new Set<string>();
  let seq = 0;

  /** Commands still waiting on `conn` get a failure (its ccrpc exited or is no longer followed). */
  const failPending = (conn: string) => {
    for (const [id, p] of pending) {
      if (p.conn !== conn) continue;
      pending.delete(id);
      p.resolve({
        type: "response",
        id,
        command: "",
        success: false,
        error: { code: "exited", message: "ccrpc 已退出" },
      });
    }
  };

  /** `conn`'s ccrpc exited, or it is no longer followed: what waits for its exit goes on. */
  const exited = (conn: string) => {
    exitWaiters.get(conn)?.();
    exitWaiters.delete(conn);
  };

  return {
    /** Sends a command and waits for its response (never rejects: failures are responses). */
    command(conn: string, msg: Record<string, unknown>): Promise<ResponseMsg> {
      const id = `g${++seq}`;
      const full = { ...msg, id };
      void api.ccrpcGuiLog(conn, `command ${String(msg.type)} id=${id}`);
      return new Promise<ResponseMsg>((resolve) => {
        pending.set(id, {
          conn,
          resolve: (r) => {
            if (!r.success)
              void api.ccrpcGuiLog(conn, `failed ${String(msg.type)} id=${id} ${r.error?.code}: ${r.error?.message}`);
            resolve(r);
          },
        });
        api.ccrpcSend(conn, full).catch((e: unknown) => {
          pending.delete(id);
          resolve({
            type: "response",
            id,
            command: String(msg.type),
            success: false,
            error: { code: "io", message: errorText(e) },
          });
        });
      });
    },

    /** A command's response arrived. */
    answer(msg: ResponseMsg) {
      const p = msg.id != null ? pending.get(String(msg.id)) : undefined;
      if (p) {
        pending.delete(String(msg.id));
        p.resolve(msg);
      }
    },

    failPending,
    exited,

    /** Resolves once `conn`'s ccrpc exits (`exited`), or after `ms`. */
    waitExit(conn: string, ms: number): Promise<void> {
      return new Promise<void>((resolve) => {
        exitWaiters.set(conn, resolve);
        timers.setTimeout(resolve, ms);
      }).then(() => void exitWaiters.delete(conn));
    },

    /** Holds an event for a connection not known yet (one already removed: dropped). */
    hold(conn: string, e: Early) {
      if (!removed.has(conn)) early.push(conn, e);
    },

    /** What was held for `conn`, in arrival order. */
    take: (conn: string) => early.take(conn),

    /** `conn` is no longer followed: what waits on it fails or goes on, and its late events are dropped. */
    forget(conn: string) {
      failPending(conn);
      removed.add(conn);
      exited(conn);
    },
  };
}
