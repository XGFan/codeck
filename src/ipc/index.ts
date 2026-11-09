import type { Api } from "./api";
import { createMockApi } from "./mock";
import { createRealApi } from "./real";
import { createRemoteApi } from "./remote";

export type { Api, Unlisten } from "./api";

/** `VITE_IPC=mock` (see `.env.mock`) swaps the Tauri backend for the in-memory mock; `remote` (`.env.host`) talks to the daemon. */
export const api: Api =
  import.meta.env.VITE_IPC === "mock"
    ? createMockApi()
    : import.meta.env.VITE_IPC === "remote"
      ? createRemoteApi(window.location.origin)
      : createRealApi();
