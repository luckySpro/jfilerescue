// Demo backend for browser preview / screenshots. Mirrors the Tauri commands
// with static data; nothing here touches a disk.

import type { DiskEntry, FoundFile, PrivilegeState, ScanStatus, SessionMeta, Signature, TreeNode, RecoverStatus } from "./api";

const now = Math.floor(Date.now() / 1000);

const disks: DiskEntry[] = [
  {
    path: "\\\\.\\PhysicalDrive0", display_name: "磁盘 0 · Samsung SSD 980 1TB", model: "Samsung SSD 980 1TB", size: 1_000_204_886_016, sector_size: 512, is_ssd: true, trim_enabled: true, removable: false,
    health: "ok", health_text: "健康", table: "Gpt", identity: "S5GXNF0R", capabilities: ["carve", "metadata"],
    volumes: [{ path: "\\\\.\\C:", display_name: "C: 系统", mount: "C:\\", fs: "ntfs", fs_label: "NTFS", size: 998_000_000_000, used: 476_000_000_000, partition_index: null, encrypted: false, encryption_unlocked: true, is_system: true, notes: [] }],
  },
  {
    path: "\\\\.\\PhysicalDrive1", display_name: "磁盘 1 · WDC WD20EZAZ 2TB", model: "WDC WD20EZAZ", size: 2_000_398_934_016, sector_size: 4096, is_ssd: false, trim_enabled: false, removable: false,
    health: "ok", health_text: "健康", table: "Gpt", identity: "WD-WX32D", capabilities: ["carve", "metadata"],
    volumes: [{ path: "\\\\.\\D:", display_name: "D: 数据盘", mount: "D:\\", fs: "ntfs", fs_label: "NTFS", size: 2_000_000_000_000, used: 812_000_000_000, partition_index: null, encrypted: false, encryption_unlocked: true, is_system: false, notes: [] }],
  },
  {
    path: "\\\\.\\PhysicalDrive2", display_name: "磁盘 2 · SanDisk Ultra 64GB", model: "SanDisk Ultra", size: 62_000_000_000, sector_size: 512, is_ssd: null, trim_enabled: null, removable: true,
    health: "ok", health_text: "健康", table: "Mbr", identity: "4C53", capabilities: ["carve", "metadata"],
    volumes: [{ path: "\\\\.\\F:", display_name: "F: TRANSFER", mount: "F:\\", fs: "ex_fat", fs_label: "exFAT", size: 61_900_000_000, used: 12_000_000_000, partition_index: null, encrypted: false, encryption_unlocked: true, is_system: false, notes: [] }],
  },
  {
    path: "\\\\.\\PhysicalDrive3", display_name: "磁盘 3 · 未知", model: "", size: 500_107_862_016, sector_size: 512, is_ssd: false, trim_enabled: null, removable: false,
    health: "warn", health_text: "分区表损坏或未初始化", table: "Unknown", identity: "disk3", capabilities: ["carve"], volumes: [],
  },
];

const names = ["合同-华东区-2025Q2.docx", "报价单-v3.xlsx", "IMG_20250412_1023.jpg", "签字页-扫描.pdf", "会议纪要-0418.docx", "附件清单.pdf", "DSC_0391.NEF", "方案汇报.pptx", "预算表.xlsx", "现场-01.jpg", "现场-02.jpg", "现场-03.jpg", "补充协议.docx", "付款凭证.pdf", "IMG_20250501_1802.jpg"];
const dirs = ["项目/2025-合同", "项目/2025-合同/附件", "项目/2025-合同/现场照片/2025-04", "项目/2025-合同/现场照片/2025-05", "项目/2025-合同/旧版本"];
const files: FoundFile[] = [];
let id = 1;
for (let i = 0; i < 140; i++) {
  const n = names[i % names.length];
  const ext = n.split(".").pop()!.toLowerCase();
  const ov = i % 9 === 0 ? 45 : i % 5 === 0 ? 12 : 0;
  files.push({
    id: id++, name: i >= names.length ? n.replace(/(\.[^.]+)$/, `-${i}$1`) : n, dir: dirs[i % dirs.length], size: 20_000 + ((i * 7919) % 30_000_000),
    source: "metadata", quality: ov === 0 ? "high" : ov < 30 ? "medium" : "low", overwritten_pct: ov, deleted_at: now - 86400 - i * 3600, modified_at: now - 86400 * 30 - i * 7200, created_at: now - 86400 * 60,
    ext, extents: [{ offset: 1_000_000 * i, len: 20_000 }], name_damaged: i % 23 === 0, via_recycle_bin: i % 7 === 3, note: ov === 0 ? "数据簇未被重新分配" : `${ov}% 数据簇已被重新分配`,
  });
}
for (let i = 0; i < 60; i++) {
  const ext = ["jpg", "pdf", "zip", "png", "docx"][i % 5];
  files.push({ id: id++, name: `carved_${String(id).padStart(6, "0")}.${ext}`, dir: null, size: 100_000 + ((i * 104729) % 8_000_000), source: "carved", quality: i % 4 === 0 ? "medium" : "high", overwritten_pct: 0, deleted_at: null, modified_at: null, created_at: null, ext, extents: [{ offset: 5_000_000_000 + i * 9_000_000, len: 100_000 }], name_damaged: false, via_recycle_bin: false, note: "结构校验通过" });
}
for (const f of files) if (f.name_damaged) f.name = `FILE_#${48000 + f.id}.${f.ext}`;
for (const f of files) if (f.via_recycle_bin) f.note = "经回收站删除；" + f.note;

const summary = () => ({ total: files.length, with_name: files.filter(f => f.source === "metadata").length, carved: files.filter(f => f.source === "carved").length, damaged_name: files.filter(f => f.name_damaged).length, via_recycle_bin: files.filter(f => f.via_recycle_bin).length, bytes: files.reduce((a, f) => a + f.size, 0), high: files.filter(f => f.quality === "high").length, medium: files.filter(f => f.quality === "medium").length, low: files.filter(f => f.quality === "low").length });

const sessions: SessionMeta[] = [
  { id: "demo-1", created_at: now - 7200, finished_at: now - 5400, state: "completed", device: { path: "\\\\.\\D:", identity: "WD", size: 2e12, sector_size: 4096, partition: null, display_name: "D: 数据盘 · NTFS", is_image: false }, request: { mode: "directory", depth: "deep", directory: "D:\\项目\\2025-合同", extensions: [], include_damaged_names: true, carve_unallocated_only: true, threads: 0 }, summary: summary(), fs: "NTFS", volume_label: "数据盘", volume_offset: 1048576, volume_len: 2e12, recovered: 6, read_errors: 0, warning: null, error: null },
  { id: "demo-2", created_at: now - 86400 * 2, finished_at: now - 86400 * 2 + 3000, state: "completed", device: { path: "\\\\.\\F:", identity: "SD", size: 6.2e10, sector_size: 512, partition: null, display_name: "F: SanDisk 64G · exFAT", is_image: false }, request: { mode: "full", depth: "deep", extensions: [], include_damaged_names: true, carve_unallocated_only: true, threads: 0 }, summary: { total: 3902, with_name: 3100, carved: 802, damaged_name: 4, via_recycle_bin: 0, bytes: 2.1e10, high: 3400, medium: 400, low: 102 }, fs: "exFAT", volume_label: "TRANSFER", volume_offset: 0, volume_len: 6.2e10, recovered: 3811, read_errors: 0, warning: null, error: null },
  { id: "demo-3", created_at: now - 86400 * 5, finished_at: now - 86400 * 5 + 120, state: "completed", device: { path: "\\\\.\\C:", identity: "S5", size: 1e12, sector_size: 512, partition: null, display_name: "C: 系统盘 · NTFS", is_image: false }, request: { mode: "directory", depth: "quick", directory: "C:\\Users\\me\\Desktop\\testjfile", extensions: [], include_damaged_names: true, carve_unallocated_only: true, threads: 0 }, summary: { total: 0, with_name: 0, carved: 0, damaged_name: 0, via_recycle_bin: 0, bytes: 0, high: 0, medium: 0, low: 0 }, fs: "NTFS", volume_label: null, volume_offset: 0, volume_len: 1e12, recovered: 0, read_errors: 0, warning: null, error: null, source_written_by_app: true },
];

const sigs: Signature[] = [
  { id: "jpg", name: "JPEG 图片", extensions: ["jpg", "jpeg"], headers: [{ hex: "FF D8 FF", offset: 0 }], length: { kind: "parser", name: "jpeg" }, min_size: 1024, max_size: 268435456, aligned: true, enabled: true, builtin: true, category: "image" },
  { id: "png", name: "PNG 图片", extensions: ["png"], headers: [{ hex: "89 50 4E 47 0D 0A 1A 0A", offset: 0 }], length: { kind: "parser", name: "png" }, min_size: 64, max_size: 268435456, aligned: true, enabled: true, builtin: true, category: "image" },
  { id: "pdf", name: "PDF 文档", extensions: ["pdf"], headers: [{ hex: "25 50 44 46 2D", offset: 0 }], length: { kind: "parser", name: "pdf" }, min_size: 64, max_size: 1073741824, aligned: true, enabled: true, builtin: true, category: "document" },
  { id: "ooxml", name: "Office Open XML (docx/xlsx/pptx)", extensions: ["docx", "xlsx", "pptx"], headers: [{ hex: "50 4B 03 04 14 00 06 00", offset: 0 }], length: { kind: "parser", name: "zip" }, min_size: 2048, max_size: 1073741824, aligned: true, enabled: true, builtin: true, category: "document" },
  { id: "mp4", name: "MP4/MOV 视频", extensions: ["mp4", "mov"], headers: [{ hex: "?? ?? ?? ?? 66 74 79 70 69 73 6F 6D", offset: 0 }], length: { kind: "parser", name: "bmff" }, min_size: 64, max_size: 17179869184, aligned: true, enabled: false, builtin: true, category: "video" },
  { id: "zip", name: "ZIP 压缩包", extensions: ["zip"], headers: [{ hex: "50 4B 03 04", offset: 0 }], length: { kind: "parser", name: "zip" }, min_size: 64, max_size: 4294967296, aligned: true, enabled: true, builtin: true, category: "archive" },
  { id: "dwg", name: "AutoCAD 图纸", extensions: ["dwg"], headers: [{ hex: "41 43 31 30", offset: 0 }], length: { kind: "max_size" }, min_size: 64, max_size: 536870912, aligned: true, enabled: true, builtin: false, category: "document" },
  { id: "sqlite", name: "SQLite 数据库", extensions: ["sqlite", "db"], headers: [{ hex: "53 51 4C 69 74 65 20 66 6F 72 6D 61 74 20 33 00", offset: 0 }], length: { kind: "parser", name: "sqlite" }, min_size: 64, max_size: 4294967296, aligned: true, enabled: true, builtin: false, category: "database" },
];

let scanStart = 0;
let scanning = false;
let demoStoreRoot = "C:\\Users\\me\\AppData\\Roaming\\com.mjgroup.jfilerescue\\sessions";
let recoverStart = 0;
let recovering = false;
let recoverIds: number[] = [];

function tree(fs: FoundFile[]): TreeNode {
  const root: TreeNode = { name: "", path: "", count: 0, children: [] };
  for (const f of fs) {
    if (f.dir == null) continue;
    let cur = root; cur.count++;
    let p = "";
    for (const seg of f.dir.split("/").filter(Boolean)) {
      p = p ? `${p}/${seg}` : seg;
      let c = cur.children.find(x => x.name === seg);
      if (!c) { c = { name: seg, path: p, count: 0, children: [] }; cur.children.push(c); }
      c.count++; cur = c;
    }
  }
  return root;
}

export async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  await new Promise(r => setTimeout(r, 30));
  const a = args as any;
  switch (cmd) {
    case "privilege_state": return { elevated: true, os: "windows", arch: "x86_64", os_version: "Microsoft Windows [10.0.26200]", full_disk_access: null, helper_version: null, details: ["已以管理员身份运行（演示数据）"] } as PrivilegeState as T;
    case "list_disks": return disks as T;
    case "busy_writers": return [{ name: "SearchIndexer.exe", pid: 4120, reason: "Windows 搜索索引" }, { name: "OneDrive.exe", pid: 8812, reason: "OneDrive 同步" }] as T;
    case "start_scan": scanStart = Date.now(); scanning = true; return "demo-1" as T;
    case "scan_progress": {
      const t = (Date.now() - scanStart) / 1000;
      const phases: Array<[number, any]> = [[1, "boot"], [6, "metadata"], [22, "carve"], [24, "finalize"]];
      let phase: any = "done"; let span = [24, 25];
      for (let i = 0; i < phases.length; i++) if (t < phases[i][0]) { phase = phases[i][1]; span = [i ? phases[i - 1][0] : 0, phases[i][0]]; break; }
      if (phase === "done") scanning = false;
      const frac = Math.min(1, (t - span[0]) / (span[1] - span[0]));
      return { session_id: "demo-1", running: scanning, error: null, memory_only: false, recent: files.slice(0, Math.min(files.length, Math.floor(t * 8))).slice(-6).reverse(),
        progress: { phase, done: Math.floor(frac * 487e9), total: 487e9, found: Math.min(files.length, Math.floor(t * 8)), bytes_read: Math.floor(t * 286e6), read_errors: 0, paused: false, message: phase === "metadata" ? "遍历 MFT" : phase === "carve" ? "签名雕刻（52 种签名）" : phase === "boot" ? "读取分区表" : "整理结果" } } as ScanStatus as T;
    }
    case "pause_scan": case "cancel_scan": if (cmd === "cancel_scan") scanning = false; return undefined as T;
    case "list_sessions": return sessions as T;
    case "load_session": return (sessions.find(s => s.id === a.id) ?? sessions[0]) as T;
    case "source_risk": { const c = String(a.source).toUpperCase().includes("C:") || String(a.source).includes("PhysicalDrive0"); return { store_on_source: c, app_data_on_source: c, exe_on_source: false, store_root: "C:\\Users\\me\\AppData\\Roaming\\com.mjgroup.jfilerescue\\sessions" } as T; }
    case "store_info": return { root: demoStoreRoot, default_root: "C:\\Users\\me\\AppData\\Roaming\\com.mjgroup.jfilerescue\\sessions", disk_identity: "physdisk:0", bytes: 184e6, memory_sessions: 0 } as T;
    case "set_store_root": demoStoreRoot = a.root; return { root: demoStoreRoot, default_root: "C:\\Users\\me\\AppData\\Roaming\\com.mjgroup.jfilerescue\\sessions", disk_identity: "physdisk:1", bytes: 0, memory_sessions: 0 } as T;
    case "save_memory_session": return sessions[0] as T;
    case "query_session": {
      const f = a.filter ?? {};
      let r = files.filter(x => (!f.hide_empty || x.size > 0) && (!f.recycle_only || x.via_recycle_bin) && (!f.query || (x.dir + "/" + x.name).toLowerCase().includes(f.query.toLowerCase())) && (!f.source || x.source === f.source) && (!f.ext || x.ext === f.ext) && (!f.dir_prefix || (x.dir ?? "").toLowerCase() === f.dir_prefix.toLowerCase() || (x.dir ?? "").toLowerCase().startsWith(f.dir_prefix.toLowerCase() + "/")) && (!f.min_size || x.size >= f.min_size) && (!f.deleted_after || (x.deleted_at ?? 0) >= f.deleted_after));
      if (f.min_quality) { const rank = { low: 0, medium: 1, high: 2 }; r = r.filter(x => rank[x.quality] >= rank[f.min_quality as "low"]); }
      const off = f.offset ?? 0; const lim = f.limit || 500;
      return { total: r.length, files: r.slice(off, off + lim) } as T;
    }
    case "session_tree": return tree(files) as T;
    case "session_types": { const m: Record<string, number> = {}; for (const f of files) m[f.ext] = (m[f.ext] ?? 0) + 1; return m as T; }
    case "delete_session": return undefined as T;
    case "export_csv": return files.length as T;
    case "sessions_bytes": return 184_000_000 as T;
    case "preview_file": {
      const f = files.find(x => x.id === a.fileId) ?? files[0];
      const hex = "0000  50 4B 03 04 14 00 06 00 08 00 00 00 21 00 D1 7A  PK..........!..z\n0010  9B 1C 68 01 00 00 F8 05 00 00 13 00 08 02 5B 43  ..h...........[C\n0020  6F 6E 74 65 6E 74 5F 54 79 70 65 73 5D 2E 78 6D  ontent_Types].xm\n0030  6C 20 A2 04 02 28 A0 00 02 00 00 00 00 00 00 00  l ...(..........";
      if (["jpg", "png"].includes(f.ext)) {
        const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='320' height='200'><defs><linearGradient id='g' x1='0' y1='0' x2='1' y2='1'><stop offset='0' stop-color='%236aa5ff'/><stop offset='1' stop-color='%23c084fc'/></linearGradient></defs><rect width='320' height='200' fill='url(%23g)'/><circle cx='240' cy='60' r='28' fill='%23fff8'/><path d='M0 160 L80 100 L140 140 L210 80 L320 150 L320 200 L0 200Z' fill='%231e293b99'/></svg>`;
        return { kind: "image", mime: "image/svg+xml", data_base64: btoa(unescape(encodeURIComponent(svg.replace(/%23/g, "#")))), text: null, hex } as Preview as T;
      }
      return { kind: "hex", mime: null, data_base64: null, text: null, hex } as T;
    }
    case "check_destination": { const same = String(a.dest).toUpperCase().startsWith("D:"); return { ok: !same, same_disk: same, free: 412e9, needed: 37.2e6, message: same ? "目标与源在同一磁盘上，写入会覆盖尚未恢复的数据" : "可用空间 412 GB，预计占用 37.2 MB" } as T; }
    case "start_recover": recoverStart = Date.now(); recovering = true; recoverIds = a.args.file_ids; return undefined as T;
    case "recover_progress": {
      const t = (Date.now() - recoverStart) / 1000;
      const n = recoverIds.length; const done = Math.min(n, Math.floor(t * 2));
      recovering = done < n;
      const items = recoverIds.slice(0, done).map((fid, i) => { const f = files.find(x => x.id === fid)!; const st = i % 11 === 10 ? "failed" : i % 7 === 6 ? "ok_with_warning" : "ok"; return { id: fid, name: f.name, path: `E:\\恢复\\${f.name}`, status: st as any, message: st === "failed" ? "读扇区错误 (I/O)" : st === "ok_with_warning" ? "尾部缺少 EOI 标记" : "校验通过", bytes: f.size }; });
      const rep = { items, ok: items.filter(i => i.status === "ok").length, warned: items.filter(i => i.status === "ok_with_warning").length, skipped: 0, failed: items.filter(i => i.status === "failed").length, bytes: items.reduce((s, i) => s + i.bytes, 0) };
      return { running: recovering, progress: { phase: recovering ? "finalize" : "done", done, total: n, found: 0, bytes_read: 0, read_errors: 0, paused: false, message: "恢复文件" }, report: rep, error: null } as RecoverStatus as T;
    }
    case "create_image": return undefined as T;
    case "list_signatures": return sigs as T;
    case "upsert_signature": { const i = sigs.findIndex(s => s.id === a.sig.id); if (i >= 0) sigs[i] = a.sig; else sigs.push({ ...a.sig, builtin: false }); return undefined as T; }
    case "set_signature_enabled": { const s = sigs.find(s => s.id === a.id); if (s) s.enabled = a.enabled; return undefined as T; }
    case "delete_signature": { const i = sigs.findIndex(s => s.id === a.id); if (i >= 0) { if (sigs[i].builtin) sigs[i].enabled = false; else sigs.splice(i, 1); } return undefined as T; }
    case "restore_signature_defaults": for (const s of sigs) if (s.builtin) s.enabled = true; return undefined as T;
    case "export_signatures": case "import_signatures": return 2 as T;
    case "test_signature": return { matched: true, resolved_len: 1258112, actual_len: 1258112, exact: true } as T;
    case "diagnostics": return { app_version: "0.1.0", core_version: "0.1.0", signatures: 52, signatures_enabled: 41, data_dir: "C:\\Users\\me\\AppData\\Roaming\\com.mjgroup.jfilerescue", sessions_bytes: 184e6, webview: "WebView2 124.0", privilege: { elevated: true } } as T;
    case "data_dir": return "C:\\Users\\me\\AppData\\Roaming\\com.mjgroup.jfilerescue" as T;
    default: throw new Error(`demo: unknown command ${cmd}`);
  }
}
type Preview = import("./api").Preview;
