import type { Backend } from "./api";
import { mockBackend } from "./mock";
import { realBackend } from "./real";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** The Tauri backend, or the wire-replay mock (`vite --mode mock`, E2E). */
export const api: Backend = import.meta.env.MODE === "mock" || !inTauri ? mockBackend : realBackend;
export type { AppInfo, Backend, DoctorLine } from "./api";
