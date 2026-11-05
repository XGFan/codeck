// The app's color theme: 自动 follows the system, 浅色 / 暗色 fix it. The colors are switched by `data-theme` on
// <html> (index.css, Tailwind's `dark:`), set before the first paint by public/theme-init.js from the same key; the
// window itself (title bar, menus, dialogs) follows through Tauri's window theme.
import { useSyncExternalStore } from "react";
import { api } from "@/ipc";

export type ThemePref = "auto" | "light" | "dark";
export type Theme = "light" | "dark";

/** localStorage key; read by public/theme-init.js too. */
export const THEME_KEY = "codeck.theme";

/** Held for as long as the page lives: its listener follows the system under 自动. */
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");
const subs = new Set<() => void>();
let pref: ThemePref = loadPref();

function loadPref(): ThemePref {
  try {
    const v = (JSON.parse(localStorage.getItem(THEME_KEY) ?? "{}") as { value?: unknown }).value;
    return v === "light" || v === "dark" ? v : "auto";
  } catch {
    return "auto";
  }
}

const systemTheme = (): Theme => (darkQuery.matches ? "dark" : "light");

function apply() {
  const next = pref === "auto" ? systemTheme() : pref;
  if (document.documentElement.dataset.theme !== next) document.documentElement.dataset.theme = next;
  for (const f of subs) f();
}

/** Applies the saved theme and follows the system while it is 自动; once, at startup. */
export function initTheme() {
  apply();
  void api.setWindowTheme(pref === "auto" ? null : pref).catch(() => {});
  // Under 自动 the window follows the system, and so does the query.
  darkQuery.addEventListener("change", () => pref === "auto" && apply());
}

export function setThemePref(next: ThemePref) {
  pref = next;
  try {
    localStorage.setItem(THEME_KEY, JSON.stringify({ value: next }));
  } catch {
    // Not remembered; applies to this window still.
  }
  // The window first: under 自动 the query then reports the system's again. Quick changes in a row: whichever
  // call lands last, the window ends on the latest choice.
  const sent = next;
  void api
    .setWindowTheme(next === "auto" ? null : next)
    .catch(() => {})
    .finally(() => {
      if (pref !== sent) void api.setWindowTheme(pref === "auto" ? null : pref).catch(() => {});
      apply();
    });
  apply();
}

const subscribe = (f: () => void) => {
  subs.add(f);
  return () => subs.delete(f);
};

export const useThemePref = () => useSyncExternalStore(subscribe, () => pref);

/** The theme in effect (自动 resolved). */
export const useTheme = () =>
  useSyncExternalStore(
    subscribe,
    () => (document.documentElement.dataset.theme === "dark" ? "dark" : "light") as Theme,
  );
