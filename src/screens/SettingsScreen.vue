<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { api, confirmDialog, isTauri, openPath, pickFile, pickSavePath, type LengthRule, type Signature } from "../api";
import { dict, fmt, locale, setLocale, type Locale } from "../i18n";
import { humanBytes, notify, theme } from "../store";

const t = dict;
const tab = ref<"general" | "sigs" | "perf" | "update">("general");
const dataDir = ref("");
const sigs = ref<Signature[]>([]);
const sigQuery = ref("");
const editing = ref<Signature | null>(null);
const editOrig = ref<string | null>(null);
const testResult = ref<string | null>(null);
const updateState = ref<"idle" | "checking" | "available" | "none" | "error">("idle");
const updateVersion = ref("");
const updateErr = ref("");
const currentVersion = ref("0.1.0");

// Persisted preferences (localStorage; mirrored to tauri store when available).
const prefs = ref({
  onClose: localStorage.getItem("jfr.onClose") ?? "tray",
  threads: Number(localStorage.getItem("jfr.threads") ?? 0),
  badSector: localStorage.getItem("jfr.badSector") ?? "retry",
  cacheLimit: localStorage.getItem("jfr.cacheLimit") ?? "2",
  unalloc: (localStorage.getItem("jfr.unalloc") ?? "1") === "1",
  autoCheck: (localStorage.getItem("jfr.autoCheck") ?? "1") === "1",
  channel: localStorage.getItem("jfr.channel") ?? "auto",
});
function save() {
  const p = prefs.value;
  localStorage.setItem("jfr.onClose", p.onClose);
  localStorage.setItem("jfr.threads", String(p.threads));
  localStorage.setItem("jfr.badSector", p.badSector);
  localStorage.setItem("jfr.cacheLimit", p.cacheLimit);
  localStorage.setItem("jfr.unalloc", p.unalloc ? "1" : "0");
  localStorage.setItem("jfr.autoCheck", p.autoCheck ? "1" : "0");
  localStorage.setItem("jfr.channel", p.channel);
}

async function loadSigs() { try { sigs.value = await api.listSignatures(); } catch (e) { notify(String(e), "error"); } }
onMounted(async () => {
  try { dataDir.value = await api.dataDir(); } catch { /* ignore */ }
  try { const d = (await api.diagnostics()) as { app_version?: string }; if (d.app_version) currentVersion.value = d.app_version; } catch { /* ignore */ }
  await loadSigs();
});

const filteredSigs = computed(() => {
  const q = sigQuery.value.toLowerCase();
  return sigs.value.filter((s) => !q || s.id.includes(q) || s.name.toLowerCase().includes(q) || s.extensions.some((e) => e.includes(q)));
});
const builtinCount = computed(() => sigs.value.filter((s) => s.builtin).length);
const customCount = computed(() => sigs.value.length - builtinCount.value);

function lengthText(l: LengthRule) {
  switch (l.kind) {
    case "footer": return `footer ${l.hex}`;
    case "parser": return `parser:${l.name}`;
    case "fixed": return `fixed ${l.size}`;
    default: return "max_size";
  }
}

async function toggle(s: Signature) {
  try { await api.setSignatureEnabled(s.id, !s.enabled); s.enabled = !s.enabled; } catch (e) { notify(String(e), "error"); }
}
async function del(s: Signature) {
  if (!s.builtin && !(await confirmDialog(t.value.cfg.delConfirm, t.value.common.delete))) return;
  try { await api.deleteSignature(s.id); await loadSigs(); } catch (e) { notify(String(e), "error"); }
}
async function restore() {
  if (!(await confirmDialog(t.value.cfg.restoreConfirm, t.value.cfg.restore))) return;
  try { await api.restoreSignatureDefaults(); await loadSigs(); } catch (e) { notify(String(e), "error"); }
}
async function exportSigs() {
  const p = await pickSavePath(t.value.cfg.export, "jfilerescue-signatures.toml", [{ name: "TOML", extensions: ["toml"] }]);
  if (!p) return;
  try { await api.exportSignatures(p); notify(p, "ok"); } catch (e) { notify(String(e), "error"); }
}
async function importSigs() {
  const p = await pickFile(t.value.cfg.import, [{ name: "TOML", extensions: ["toml"] }]);
  if (!p) return;
  try { const n = await api.importSignatures(p); notify(`+${n}`, "ok"); await loadSigs(); } catch (e) { notify(String(e), "error"); }
}

// ---- editor ----
const form = ref({ id: "", name: "", exts: "", hex: "", offset: 0, rule: "max_size" as LengthRule["kind"], footer: "", parser: "zip", fixed: 0, maxMB: 512, minKB: 0, aligned: true, category: "" });
const parsers = ["jpeg", "png", "bmp", "riff", "bmff", "zip", "pdf", "ole", "sevenzip", "tar", "asf", "pe", "sqlite", "text"];
function openNew() {
  editing.value = { id: "", name: "", extensions: [], headers: [], length: { kind: "max_size" }, min_size: 64, max_size: 512 << 20, aligned: true, enabled: true, builtin: false, category: "" };
  editOrig.value = null;
  form.value = { id: "", name: "", exts: "", hex: "", offset: 0, rule: "max_size", footer: "", parser: "zip", fixed: 0, maxMB: 512, minKB: 0, aligned: true, category: "" };
  testResult.value = null;
}
function openEdit(s: Signature) {
  editing.value = s; editOrig.value = s.id;
  const l = s.length;
  form.value = { id: s.id, name: s.name, exts: s.extensions.join("/"), hex: s.headers[0]?.hex ?? "", offset: s.headers[0]?.offset ?? 0, rule: l.kind, footer: l.kind === "footer" ? l.hex : "", parser: l.kind === "parser" ? l.name : "zip", fixed: l.kind === "fixed" ? l.size : 0, maxMB: Math.round(s.max_size / 1048576), minKB: Math.round(s.min_size / 1024), aligned: s.aligned, category: s.category };
  testResult.value = null;
}
function buildSig(): Signature | string {
  const f = form.value;
  const id = f.id.trim().toLowerCase().replace(/[^a-z0-9_-]/g, "");
  if (!id) return t.value.sigForm.id;
  const exts = f.exts.split(/[\/,\s]+/).map((e) => e.trim().replace(/^\./, "").toLowerCase()).filter(Boolean);
  if (!exts.length) return t.value.sigForm.exts;
  if (!/^(\s*([0-9a-fA-F]{2}|\?\?)\s*)+$/.test(f.hex)) return t.value.sigForm.hex;
  if (sigs.value.some((s) => s.id === id && s.id !== editOrig.value)) return `id "${id}" already exists`;
  const length: LengthRule = f.rule === "footer" ? { kind: "footer", hex: f.footer.toUpperCase() } : f.rule === "parser" ? { kind: "parser", name: f.parser } : f.rule === "fixed" ? { kind: "fixed", size: Number(f.fixed) } : { kind: "max_size" };
  if (length.kind === "footer" && !/^(\s*[0-9a-fA-F]{2}\s*)+$/.test(length.hex)) return t.value.sigForm.footer;
  return { id, name: f.name.trim() || id, extensions: exts, headers: [{ hex: f.hex.toUpperCase().trim().replace(/\s+/g, " "), offset: Number(f.offset) || 0 }], length, min_size: Math.max(0, Number(f.minKB)) * 1024, max_size: Math.max(1, Number(f.maxMB)) * 1048576, aligned: f.aligned, enabled: editing.value?.enabled ?? true, builtin: editing.value?.builtin ?? false, category: f.category };
}
async function saveSig() {
  const s = buildSig();
  if (typeof s === "string") return notify(s, "error");
  try { await api.upsertSignature(s); editing.value = null; await loadSigs(); } catch (e) { notify(String(e), "error"); }
}
async function testSig() {
  const s = buildSig();
  if (typeof s === "string") return notify(s, "error");
  const p = await pickFile(t.value.sigForm.test);
  if (!p) return;
  try {
    const r = await api.testSignature(s, p);
    testResult.value = r.matched ? fmt(t.value.sigForm.testOk, { len: humanBytes(r.resolved_len ?? 0), actual: humanBytes(r.actual_len) }) : t.value.sigForm.testBad;
  } catch (e) { testResult.value = String(e); }
}

// ---- updater ----
async function checkUpdate() {
  updateState.value = "checking";
  if (!isTauri) { await new Promise((r) => setTimeout(r, 600)); updateState.value = "none"; return; }
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const u = await check();
    if (u) { updateState.value = "available"; updateVersion.value = u.version; (window as any).__jfrUpdate = u; } else updateState.value = "none";
  } catch (e) { updateState.value = "error"; updateErr.value = String(e); }
}
async function installUpdate() {
  const u = (window as any).__jfrUpdate;
  if (!u) return;
  try {
    await u.downloadAndInstall();
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  } catch (e) { notify(String(e), "error"); }
}
</script>

<template>
  <section>
    <h1>{{ t.cfg.title }}</h1>
    <div class="tabs">
      <button class="tab" :class="{ on: tab === 'general' }" @click="tab = 'general'">{{ t.cfg.general }}</button>
      <button class="tab" :class="{ on: tab === 'sigs' }" @click="tab = 'sigs'">{{ t.cfg.sigs }}</button>
      <button class="tab" :class="{ on: tab === 'perf' }" @click="tab = 'perf'">{{ t.cfg.perf }}</button>
      <button class="tab" :class="{ on: tab === 'update' }" @click="tab = 'update'">{{ t.cfg.update }}</button>
    </div>

    <div v-if="tab === 'general'" class="card">
      <div class="setrow"><div>{{ t.cfg.lang }}</div><select :value="locale" @change="setLocale(($event.target as HTMLSelectElement).value as Locale)"><option value="system">{{ t.top.system }}</option><option value="zh-CN">简体中文</option><option value="zh-TW">繁體中文</option><option value="en">English</option><option value="ja">日本語</option></select></div>
      <div class="setrow"><div>{{ t.cfg.theme }}</div><select v-model="theme"><option value="system">{{ t.top.system }}</option><option value="light">{{ t.top.light }}</option><option value="dark">{{ t.top.dark }}</option></select></div>
      <div class="setrow"><div>{{ t.cfg.dataDir }}<div class="d mono">{{ dataDir }}</div></div><button class="btn" @click="openPath(dataDir)">{{ t.cfg.openDir }}</button></div>
      <div class="setrow"><div>{{ t.cfg.onClose }}<div class="d">{{ t.cfg.onCloseDesc }}</div></div><select v-model="prefs.onClose" @change="save"><option value="tray">{{ t.cfg.tray }}</option><option value="ask">{{ t.cfg.ask }}</option><option value="quit">{{ t.cfg.quit }}</option></select></div>
    </div>

    <div v-else-if="tab === 'sigs'" class="card">
      <div class="row" style="justify-content: space-between; margin-bottom: 10px">
        <div><b>{{ t.cfg.sigTitle }}</b> <span class="small muted">· {{ t.cfg.builtin }} {{ builtinCount }} · {{ t.cfg.custom }} {{ customCount }} · {{ t.cfg.used }}</span></div>
        <div class="row"><input type="search" v-model="sigQuery" :placeholder="t.cfg.searchSig" /><button class="btn" @click="openNew">{{ t.cfg.addSig }}</button><button class="btn" @click="importSigs">{{ t.cfg.import }}</button><button class="btn" @click="exportSigs">{{ t.cfg.export }}</button><button class="btn" @click="restore">{{ t.cfg.restore }}</button></div>
      </div>
      <div style="max-height: 60vh; overflow: auto">
      <table>
        <thead><tr><th>{{ t.cfg.enabled }}</th><th>{{ t.cfg.ext }}</th><th>{{ t.cfg.name }}</th><th>{{ t.cfg.header }}</th><th>{{ t.cfg.length }}</th><th>{{ t.cfg.origin }}</th><th></th></tr></thead>
        <tbody>
          <tr v-for="s in filteredSigs" :key="s.id">
            <td><button class="switch" :class="{ on: s.enabled }" @click="toggle(s)" role="switch" :aria-checked="s.enabled"></button></td>
            <td>{{ s.extensions.join(" / ") }}</td>
            <td>{{ s.name }}</td>
            <td class="mono">{{ s.headers.map(h => (h.offset ? `@${h.offset} ` : "") + h.hex).join(" | ") }}</td>
            <td class="mono">{{ lengthText(s.length) }} · ≤{{ humanBytes(s.max_size) }}</td>
            <td><span class="chip" :class="s.builtin ? '' : 'info'">{{ s.builtin ? t.cfg.builtin : t.cfg.custom }}</span></td>
            <td style="white-space: nowrap"><button class="btn sm" @click="openEdit(s)">{{ t.cfg.edit }}</button> <button class="btn sm" :class="{ danger: !s.builtin }" @click="del(s)">{{ s.builtin ? t.cfg.disable : t.cfg.delete }}</button></td>
          </tr>
        </tbody>
      </table>
      </div>
      <p class="small muted mt-s">{{ t.cfg.sigNote }}</p>
    </div>

    <div v-else-if="tab === 'perf'" class="card">
      <div class="setrow"><div>{{ t.cfg.threads }}<div class="d">{{ t.cfg.threadsDesc }}</div></div><select v-model.number="prefs.threads" @change="save"><option :value="0">{{ t.cfg.auto }}</option><option :value="1">1</option><option :value="2">2</option><option :value="4">4</option><option :value="8">8</option><option :value="16">16</option></select></div>
      <div class="setrow"><div>{{ t.cfg.badSector }}<div class="d">{{ t.cfg.badSectorDesc }}</div></div><select v-model="prefs.badSector" @change="save"><option value="retry">{{ t.cfg.retrySkip }}</option><option value="skip">{{ t.cfg.skipNow }}</option></select></div>
      <div class="setrow"><div>{{ t.cfg.cache }}<div class="d">{{ t.cfg.cacheDesc }}</div></div><select v-model="prefs.cacheLimit" @change="save"><option value="2">2 GB</option><option value="10">10 GB</option><option value="0">{{ t.cfg.unlimited }}</option></select></div>
      <div class="setrow"><div>{{ t.cfg.unalloc }}<div class="d">{{ t.cfg.unallocDesc }}</div></div><button class="switch" :class="{ on: prefs.unalloc }" @click="prefs.unalloc = !prefs.unalloc; save()" role="switch" :aria-checked="prefs.unalloc"></button></div>
      <p class="small muted mt-s">{{ t.cfg.restartNote }}</p>
    </div>

    <div v-else class="card">
      <div class="setrow"><div>{{ t.cfg.version }} <span class="mono">{{ currentVersion }}</span><div class="d">
        <span v-if="updateState === 'checking'"><span class="spin"></span> {{ t.cfg.checking }}</span>
        <span v-else-if="updateState === 'available'" style="color: var(--ok)">{{ t.cfg.available }}: {{ updateVersion }}</span>
        <span v-else-if="updateState === 'none'">{{ t.cfg.noUpdate }}</span>
        <span v-else-if="updateState === 'error'" style="color: var(--bad)">{{ t.cfg.updateErr }}: {{ updateErr }}</span>
      </div></div><div class="row"><button class="btn" @click="checkUpdate" :disabled="updateState === 'checking'">{{ t.cfg.check }}</button><button v-if="updateState === 'available'" class="btn primary" @click="installUpdate">{{ t.cfg.installNow }}</button></div></div>
      <div class="setrow"><div>{{ t.cfg.autoCheck }}<div class="d">{{ t.cfg.autoCheckDesc }}</div></div><button class="switch" :class="{ on: prefs.autoCheck }" @click="prefs.autoCheck = !prefs.autoCheck; save()" role="switch" :aria-checked="prefs.autoCheck"></button></div>
      <div class="setrow"><div>{{ t.cfg.channel }}<div class="d">{{ t.cfg.channelDesc }}</div></div><select v-model="prefs.channel" @change="save"><option value="auto">{{ t.cfg.chAuto }}</option><option value="github">{{ t.cfg.chGithub }}</option><option value="proxy">{{ t.cfg.chProxy }}</option></select></div>
    </div>

    <div v-if="editing" class="modal-bg" @click.self="editing = null" @keydown.esc="editing = null">
      <div class="modal" role="dialog" aria-modal="true">
        <div class="hd">{{ editOrig ? t.sigForm.edit : t.sigForm.add }}</div>
        <div class="bd">
          <div class="grid-2">
            <div class="field"><label>{{ t.sigForm.id }}</label><input type="text" v-model="form.id" :disabled="!!editOrig" placeholder="psd" /></div>
            <div class="field"><label>{{ t.sigForm.exts }}</label><input type="text" v-model="form.exts" placeholder="psd" /></div>
          </div>
          <div class="field"><label>{{ t.sigForm.name }}</label><input type="text" v-model="form.name" placeholder="Photoshop 文档" /></div>
          <div class="grid-2">
            <div class="field"><label>{{ t.sigForm.hex }}</label><input type="text" class="mono" v-model="form.hex" placeholder="38 42 50 53" /></div>
            <div class="field"><label>{{ t.sigForm.offset }}</label><input type="number" class="mono" v-model.number="form.offset" min="0" /></div>
          </div>
          <div class="field"><label>{{ t.sigForm.rule }}</label>
            <select v-model="form.rule"><option value="max_size">{{ t.sigForm.maxOnly }}</option><option value="footer">{{ t.sigForm.footer }}</option><option value="parser">{{ t.sigForm.parser }}</option><option value="fixed">{{ t.sigForm.fixed }}</option></select>
          </div>
          <div class="field" v-if="form.rule === 'footer'"><label>{{ t.sigForm.footer }}</label><input type="text" class="mono" v-model="form.footer" placeholder="FF D9" /></div>
          <div class="field" v-if="form.rule === 'parser'"><label>{{ t.sigForm.parser }}</label><select v-model="form.parser"><option v-for="p in parsers" :key="p" :value="p">{{ p }}</option></select></div>
          <div class="field" v-if="form.rule === 'fixed'"><label>{{ t.sigForm.fixed }}</label><input type="number" v-model.number="form.fixed" min="1" /></div>
          <div class="grid-2">
            <div class="field"><label>{{ t.sigForm.minSize }}</label><input type="number" v-model.number="form.minKB" min="0" /></div>
            <div class="field"><label>{{ t.sigForm.maxSize }}</label><input type="number" v-model.number="form.maxMB" min="1" /></div>
          </div>
          <label class="row small"><input type="checkbox" v-model="form.aligned" /> {{ t.sigForm.aligned }}</label>
          <div class="notice info mt-s"><span>ⓘ</span><span>{{ t.sigForm.hint }}</span></div>
          <div v-if="testResult" class="notice mt-s" :class="testResult.startsWith('未') || testResult.startsWith('No') ? 'warn' : 'ok'"><span>⚗</span><span>{{ testResult }}</span></div>
        </div>
        <div class="ft"><button class="btn" @click="testSig">{{ t.sigForm.test }}</button><button class="btn" @click="editing = null">{{ t.sigForm.cancel }}</button><button class="btn primary" @click="saveSig">{{ t.sigForm.save }}</button></div>
      </div>
    </div>
  </section>
</template>
