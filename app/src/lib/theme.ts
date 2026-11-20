import type { Backdrop, ThemeMode } from "./bindings";

export type ResolvedTheme = "light" | "dark";

const DARK_QUERY = "(prefers-color-scheme: dark)";

export function resolveTheme(mode: ThemeMode, systemPrefersDark: boolean): ResolvedTheme {
  if (mode === "light" || mode === "dark") return mode;
  return systemPrefersDark ? "dark" : "light";
}

export function systemPrefersDark(): boolean {
  return window.matchMedia(DARK_QUERY).matches;
}

/** 系统深浅色变化时回调；返回取消监听的函数。 */
export function watchSystemTheme(onChange: (prefersDark: boolean) => void): () => void {
  const query = window.matchMedia(DARK_QUERY);
  const listener = (event: MediaQueryListEvent) => onChange(event.matches);
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}

export function applyTheme(theme: ResolvedTheme): void {
  document.documentElement.dataset.theme = theme;
}

export function applyBackdrop(backdrop: Backdrop): void {
  document.documentElement.dataset.backdrop = backdrop;
}

export function applyReduceMotion(reduce: boolean): void {
  document.documentElement.dataset.reduceMotion = String(reduce);
}
