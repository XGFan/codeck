import type { Api } from "./api";
import { createMockApi } from "@/mocks";
import { createRealApi } from "./real";
import { createNativeRemoteApi, createRemoteApi } from "./remote";

export type { Api, Unlisten } from "./api";

/** `VITE_IPC=mock` (see `.env.mock`) swaps the Tauri backend for the in-memory mock; `remote` (`.env.host`) talks to the daemon (or via the iOS app bridge when present). */
export const api: Api =
  import.meta.env.VITE_IPC === "mock"
    ? createMockApi()
    : import.meta.env.VITE_IPC === "remote"
      ? window.webkit?.messageHandlers?.codeckRpc
        ? createNativeRemoteApi()
        : createRemoteApi(window.location.origin)
      : createRealApi();
