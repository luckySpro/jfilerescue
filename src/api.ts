// Typed IPC bridge to the Tauri backend. When running outside Tauri (browser
// preview / screenshots) it falls back to `demo.ts` so every screen is viewable.

import * as demo from "./demo";

export type Source = "metadata" | "carved";
export type Quality = "low" | "medium" | "high";
export type FsKind = "ntfs" | "fat12" | "fat16" | "fat32" | "ex_fat" | "apfs" | "unknown";
export type ScanMode = "directory" | "full";
export type ScanDepth = "quick" | "deep";
export type Phase = "idle" | "boot" | "metadata" | "carve" | "finalize" | "done" | "cancelled" | "failed";

export interface FoundFile {
  id: number;
  name: string;
  dir: string | null;
  size: number;
  source: Source;
  quality: Quality;
  overwritten_pct: number;
  deleted_at: number | null;
  modified_at: number | null;
  created_at: number | null;
  ext: string;
  extents: { offset: number; len: number }[];
  name_damaged: boolean;
  via_recycle_bin: boolean;
  note: string;
}

export interface ScanRequest {
  mode: ScanMode;
  depth: ScanDepth;
  directory?: string | null;
  extensions: string[];
  include_damaged_names: boolean;
  carve_unallocated_only: boolean;
  threads: number;
}

export interface ResultFilter {
  query?: string;
  min_quality?: Quality | null;
  min_size?: number | null;
  deleted_after?: number | null;
  source?: Source | null;
  dir_prefix?: string | null;
  ext?: string | null;
  recycle_only?: boolean;
  hide_empty?: boolean;
  offset?: number;
  limit?: number;
}

export interface Summary {
  total: number; with_name: number; carved: number; damaged_name: number; via_recycle_bin: number; bytes: number; high: number; medium: number; low: number;
}

export interface VolumeEntry {
  path: string; display_name: string; mount: string | null; fs: FsKind; fs_label: string; size: number; used: number | null;
  partition_index: number | null; encrypted: boolean; encryption_unlocked: boolean; is_system: boolean; notes: string[];
}
export interface DiskEntry {
  path: string; display_name: string; model: string; size: number; sector_size: number; is_ssd: boolean | null; trim_enabled: boolean | null;
  removable: boolean; health: "ok" | "warn" | "bad" | "unknown"; health_text: string; table: string; volumes: VolumeEntry[]; identity: string; capabilities: string[];
}
export interface PrivilegeState {
  elevated: boolean; os: string; arch: string; os_version: string; full_disk_access: boolean | null; helper_version: string | null; details: string[];
}
export interface WriterProcess { name: string; pid: number; reason: string }

export interface ProgressSnapshot {
  phase: Phase; done: number; total: number; found: number; bytes_read: number; read_errors: number; paused: boolean; message: string;
}
export interface DeviceRef { path: string; identity: string; size: number; sector_size: number; partition: number | null; display_name: string; is_image: boolean }
export interface SessionMeta {
  id: string; created_at: number; finished_at: number | null; state: "running" | "completed" | "aborted" | "failed"; device: DeviceRef; request: ScanRequest;
  summary: Summary; fs: string; volume_label: string | null; volume_offset: number; volume_len: number; recovered: number; read_errors: number; warning: string | null; error: string | null;
  store_disk_identity?: string | null; source_written_by_app?: boolean;
}
export interface ScanStatus { session_id: string | null; running: boolean; progress: ProgressSnapshot; error: string | null; recent: FoundFile[]; memory_only: boolean }
export interface SourceRisk { store_on_source: boolean; app_data_on_source: boolean; exe_on_source: boolean; store_root: string }
export interface StoreInfo { root: string; default_root: string; disk_identity: string | null; bytes: number; memory_sessions: number }
export interface TreeNode { name: string; path: string; count: number; children: TreeNode[] }
export interface Page { total: number; files: FoundFile[] }
export interface Preview { kind: "image" | "text" | "hex"; mime: string | null; data_base64: string | null; text: string | null; hex: string }
export interface DestCheck { ok: boolean; same_disk: boolean; free: number; needed: number; message: string }
export type Conflict = "rename" | "skip" | "overwrite";
export interface RecoverOptions { dest: string; keep_structure: boolean; conflict: Conflict; verify: boolean; restore_times: boolean }
export interface ItemResult { id: number; name: string; path: string | null; status: "ok" | "ok_with_warning" | "skipped" | "failed"; message: string; bytes: number }
export interface RecoverReport { items: ItemResult[]; ok: number; warned: number; skipped: number; failed: number; bytes: number }
export interface RecoverStatus { running: boolean; progress: ProgressSnapshot; report: RecoverReport | null; error: string | null }

export interface HeaderPattern { hex: string; offset: number }
export type LengthRule = { kind: "footer"; hex: string } | { kind: "parser"; name: string } | { kind: "fixed"; size: number } | { kind: "max_size" };
export interface Signature {
  id: string; name: string; extensions: string[]; headers: HeaderPattern[]; length: LengthRule; min_size: number; max_size: number;
  aligned: boolean; enabled: boolean; builtin: boolean; category: string;
}

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri) return demo.invoke<T>(cmd, args);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export const api = {
  privilegeState: () => call<PrivilegeState>("privilege_state"),
  listDisks: () => call<DiskEntry[]>("list_disks"),
  busyWriters: (volume: string) => call<WriterProcess[]>("busy_writers", { volume }),
  relaunchElevated: () => call<void>("relaunch_elevated"),
  openPrivacySettings: () => call<void>("open_privacy_settings"),

  startScan: (source: string, partition: number | null, request: ScanRequest, displayName: string) =>
    call<string>("start_scan", { args: { source, partition, request, display_name: displayName } }),
  scanProgress: () => call<ScanStatus>("scan_progress"),
  pauseScan: (paused: boolean) => call<void>("pause_scan", { paused }),
  cancelScan: () => call<void>("cancel_scan"),

  listSessions: () => call<SessionMeta[]>("list_sessions"),
  loadSession: (id: string) => call<SessionMeta>("load_session", { id }),
  sourceRisk: (source: string) => call<SourceRisk>("source_risk", { source }),
  storeInfo: () => call<StoreInfo>("store_info"),
  setStoreRoot: (root: string) => call<StoreInfo>("set_store_root", { root }),
  saveMemorySession: (id: string) => call<SessionMeta>("save_memory_session", { id }),
  querySession: (id: string, filter: ResultFilter) => call<Page>("query_session", { id, filter }),
  sessionTree: (id: string) => call<TreeNode>("session_tree", { id }),
  sessionTypes: (id: string) => call<Record<string, number>>("session_types", { id }),
  deleteSession: (id: string) => call<void>("delete_session", { id }),
  exportCsv: (id: string, path: string) => call<number>("export_csv", { id, path }),
  sessionsBytes: () => call<number>("sessions_bytes"),
  previewFile: (id: string, fileId: number) => call<Preview>("preview_file", { id, fileId }),

  checkDestination: (id: string, dest: string, fileIds: number[]) => call<DestCheck>("check_destination", { id, dest, fileIds }),
  startRecover: (id: string, fileIds: number[], options: RecoverOptions) => call<void>("start_recover", { args: { id, file_ids: fileIds, options, source_override: null } }),
  recoverProgress: () => call<RecoverStatus>("recover_progress"),
  createImage: (source: string, dest: string) => call<void>("create_image", { source, dest }),

  listSignatures: () => call<Signature[]>("list_signatures"),
  upsertSignature: (sig: Signature) => call<void>("upsert_signature", { sig }),
  setSignatureEnabled: (id: string, enabled: boolean) => call<void>("set_signature_enabled", { id, enabled }),
  deleteSignature: (id: string) => call<void>("delete_signature", { id }),
  restoreSignatureDefaults: () => call<void>("restore_signature_defaults"),
  exportSignatures: (path: string) => call<void>("export_signatures", { path }),
  importSignatures: (path: string) => call<number>("import_signatures", { path }),
  testSignature: (sig: Signature, sample: string) => call<{ matched: boolean; resolved_len: number | null; actual_len: number; exact: boolean }>("test_signature", { sig, sample }),

  diagnostics: () => call<Record<string, unknown>>("diagnostics"),
  dataDir: () => call<string>("data_dir"),
};

// ---- dialogs / shell (Tauri plugins; browser fallbacks) ----
export async function pickDirectory(title: string): Promise<string | null> {
  if (!isTauri) return window.prompt(title, "E:\\恢复") ?? null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, multiple: false, title });
  return typeof r === "string" ? r : null;
}
export async function pickFile(title: string, filters?: { name: string; extensions: string[] }[]): Promise<string | null> {
  if (!isTauri) return window.prompt(title) ?? null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: false, multiple: false, title, filters });
  return typeof r === "string" ? r : null;
}
export async function pickSavePath(title: string, defaultName: string, filters?: { name: string; extensions: string[] }[]): Promise<string | null> {
  if (!isTauri) return window.prompt(title, defaultName) ?? null;
  const { save } = await import("@tauri-apps/plugin-dialog");
  return (await save({ title, defaultPath: defaultName, filters })) ?? null;
}
export async function openPath(path: string) {
  if (!isTauri) return;
  const { openPath: op } = await import("@tauri-apps/plugin-opener");
  await op(path);
}
export async function revealPath(path: string) {
  if (!isTauri) return;
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}
export async function confirmDialog(message: string, title: string): Promise<boolean> {
  if (!isTauri) return window.confirm(message);
  const { confirm } = await import("@tauri-apps/plugin-dialog");
  return confirm(message, { title, kind: "warning" });
}
