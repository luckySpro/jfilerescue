// Global reactive UI state shared across screens.
import { reactive, ref, watch } from "vue";
import type { DiskEntry, FoundFile, PrivilegeState, ScanDepth, ScanMode, SessionMeta, VolumeEntry } from "./api";

export type Screen = "home" | "scan" | "results" | "recover" | "sessions" | "settings" | "perm" | "about";
export const screen = ref<Screen>("home");
export function go(s: Screen) {
  screen.value = s;
  document.querySelector(".main")?.scrollTo(0, 0);
}

export type Theme = "system" | "light" | "dark";
export const theme = ref<Theme>((localStorage.getItem("jfr.theme") as Theme) || "system");
function applyTheme() {
  const dark = theme.value === "dark" || (theme.value === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}
watch(theme, (t) => { localStorage.setItem("jfr.theme", t); applyTheme(); }, { immediate: true });
window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", applyTheme);
export function toggleTheme() {
  theme.value = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
}

export const privilege = ref<PrivilegeState | null>(null);
export const disks = ref<DiskEntry[]>([]);
export const disksLoading = ref(false);

/** Scan configuration being built on the Home screen. */
export const setup = reactive({
  mode: "directory" as ScanMode,
  depth: "quick" as ScanDepth,
  directory: "",
  extensions: [] as string[],
  includeDamaged: true,
  carveUnallocatedOnly: true,
  makeImage: false,
  threads: 0,
  /** selected source: either a volume (preferred) or a whole disk */
  source: null as null | { path: string; partition: number | null; displayName: string; disk: DiskEntry; volume: VolumeEntry | null },
  imagePath: "" as string,
});

/** The session the Results / Recover screens show. */
export const currentSession = ref<SessionMeta | null>(null);
export const selection = reactive(new Set<number>());
export const selectedFiles = ref<FoundFile[]>([]);

export const toast = ref<{ text: string; kind: "info" | "error" | "ok" } | null>(null);
let toastTimer: number | undefined;
export function notify(text: string, kind: "info" | "error" | "ok" = "info") {
  toast.value = { text, kind };
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.value = null), kind === "error" ? 6000 : 3000);
}

export function humanBytes(n: number | null | undefined): string {
  if (n == null) return "—";
  const u = ["B", "KB", "MB", "GB", "TB", "PB"];
  let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(1)} ${u[i]}`;
}

export function fmtTime(t: number | null | undefined): string {
  if (!t) return "—";
  const d = new Date(t * 1000);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function fmtDuration(sec: number): string {
  sec = Math.max(0, Math.floor(sec));
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), s = sec % 60;
  return h ? `${h}h ${m}m` : m ? `${m}m ${s}s` : `${s}s`;
}
