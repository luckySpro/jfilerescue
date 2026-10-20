<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { api, openPath, pickDirectory, pickSavePath, type Conflict, type DestCheck, type RecoverStatus } from "../api";
import { dict, fmt } from "../i18n";
import { currentSession, go, humanBytes, notify, selectedFiles } from "../store";

const t = dict;
const session = currentSession;
const dest = ref(localStorage.getItem("jfr.lastDest") ?? "");
const keep = ref(true);
const conflict = ref<Conflict>("rename");
const verify = ref(true);
const times = ref(true);
const check = ref<DestCheck | null>(null);
const status = ref<RecoverStatus | null>(null);
const started = ref(false);
let timer: number | undefined;
let checkTimer: number | undefined;

const ids = computed(() => selectedFiles.value.map((f) => f.id));
const totalBytes = computed(() => selectedFiles.value.reduce((a, f) => a + f.size, 0));

async function doCheck() {
  if (!session.value || !dest.value.trim()) { check.value = null; return; }
  try { check.value = await api.checkDestination(session.value.id, dest.value, ids.value); } catch (e) { check.value = { ok: false, same_disk: false, free: 0, needed: totalBytes.value, message: String(e) }; }
}
watch(dest, () => { window.clearTimeout(checkTimer); checkTimer = window.setTimeout(doCheck, 300); });
onMounted(() => { if (!session.value || selectedFiles.value.length === 0) { go("results"); return; } doCheck(); });
onBeforeUnmount(() => { window.clearInterval(timer); window.clearTimeout(checkTimer); });

async function browse() { const p = await pickDirectory(t.value.recover.saveTo); if (p) dest.value = p; }

async function start() {
  if (!session.value) return;
  if (!check.value?.ok) return notify(check.value?.message ?? t.value.recover.notSame, "error");
  try {
    await api.startRecover(session.value.id, ids.value, { dest: dest.value, keep_structure: keep.value, conflict: conflict.value, verify: verify.value, restore_times: times.value });
    localStorage.setItem("jfr.lastDest", dest.value);
    started.value = true;
    timer = window.setInterval(async () => {
      status.value = await api.recoverProgress();
      if (!status.value.running) { window.clearInterval(timer); if (status.value.error) notify(status.value.error, "error"); }
    }, 500);
  } catch (e) { notify(String(e), "error"); }
}
const pct = computed(() => { const p = status.value?.progress; return p && p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : 0; });
const report = computed(() => status.value?.report ?? null);
async function exportReport() {
  if (!report.value) return;
  const p = await pickSavePath(t.value.recover.report, "jfilerescue-recover-report.json", [{ name: "JSON", extensions: ["json"] }]);
  if (!p) return;
  // The report is small; write it via a data URL download in browser, or via the backend path in Tauri (reuse export_csv path writer is not applicable) — keep it simple: copy JSON to clipboard as fallback.
  try { await navigator.clipboard.writeText(JSON.stringify(report.value, null, 2)); notify(t.value.common.copied, "ok"); } catch { /* ignore */ }
}
function chipClass(s: string) { return s === "ok" ? "ok" : s === "ok_with_warning" ? "warn" : s === "failed" ? "bad" : ""; }
</script>

<template>
  <section v-if="session">
    <h1>{{ t.recover.title }}</h1>
    <p class="sub">{{ t.results.selected }} {{ selectedFiles.length }} · {{ humanBytes(totalBytes) }}</p>
    <div class="grid-main-side">
      <div>
        <div class="card">
          <h2>{{ t.recover.saveTo }}</h2>
          <div class="row"><input type="text" v-model="dest" style="flex: 1" :disabled="started" /><button class="btn" @click="browse" :disabled="started">{{ t.settings.browse }}</button></div>
          <div v-if="check && !check.ok" class="notice bad mt-s"><span>⛔</span><span><span class="b">{{ check.same_disk ? t.recover.notSame : '' }}</span> {{ check.same_disk ? t.recover.notSameBody : check.message }}</span></div>
          <div v-else-if="check" class="notice ok mt-s"><span>✓</span><span>{{ check.message }}</span></div>
          <div v-else class="notice warn mt-s"><span>⚠</span><span><span class="b">{{ t.recover.notSame }}</span> {{ t.recover.notSameBody }}</span></div>
        </div>
        <div class="card mt">
          <h2>{{ t.recover.options }}</h2>
          <div class="setrow"><div>{{ t.recover.keep }}<div class="d">{{ t.recover.keepDesc }}</div></div><button class="switch" :class="{ on: keep }" @click="keep = !keep" :disabled="started" role="switch" :aria-checked="keep"></button></div>
          <div class="setrow"><div>{{ t.recover.conflict }}<div class="d">{{ t.recover.conflictDesc }}</div></div><select v-model="conflict" :disabled="started"><option value="rename">{{ t.recover.rename }}</option><option value="skip">{{ t.recover.skip }}</option><option value="overwrite">{{ t.recover.overwrite }}</option></select></div>
          <div class="setrow"><div>{{ t.recover.verify }}<div class="d">{{ t.recover.verifyDesc }}</div></div><button class="switch" :class="{ on: verify }" @click="verify = !verify" :disabled="started" role="switch" :aria-checked="verify"></button></div>
          <div class="setrow"><div>{{ t.recover.times }}<div class="d">{{ t.recover.timesDesc }}</div></div><button class="switch" :class="{ on: times }" @click="times = !times" :disabled="started" role="switch" :aria-checked="times"></button></div>
        </div>
        <div class="row mt"><button class="btn primary lg" :disabled="started || !check?.ok" @click="start">{{ t.recover.start }}</button><button class="btn" @click="go('results')">{{ t.recover.back }}</button></div>
      </div>
      <div class="card">
        <h2>{{ t.recover.progress }}</h2>
        <template v-if="status">
          <div class="bigbar"><i :style="{ width: pct + '%' }"></i></div>
          <div class="small muted">{{ status.progress.done }} / {{ status.progress.total }}<template v-if="report"> · {{ humanBytes(report.bytes) }}</template></div>
          <div v-if="report && !status.running" class="notice" :class="report.failed ? 'warn' : 'ok'" style="margin-top: 8px"><span>{{ report.failed ? '⚠' : '✓' }}</span><span>{{ fmt(t.recover.summary, { ok: report.ok, warned: report.warned, skipped: report.skipped, failed: report.failed }) }}</span></div>
          <table class="mt-s rec-items" v-if="report">
            <tbody>
              <tr v-for="i in report.items.slice(-40)" :key="i.id"><td class="ellipsis" style="max-width: 160px" :title="i.path ?? ''">{{ i.name }}</td><td><span class="chip" :class="chipClass(i.status)"><span class="dot"></span>{{ t.recover.st[i.status] }}<template v-if="i.message && i.status !== 'ok'"> · {{ i.message }}</template></span></td></tr>
              <tr v-if="status.running"><td class="muted">…</td><td><span class="chip"><span class="dot"></span>{{ t.recover.writing }}</span></td></tr>
            </tbody>
          </table>
          <div class="row mt"><button class="btn" @click="openPath(dest)" :disabled="status.running">{{ t.recover.openDest }}</button><button class="btn" @click="exportReport" :disabled="!report">{{ t.recover.report }}</button></div>
        </template>
        <div v-else class="empty small">—</div>
      </div>
    </div>
  </section>
</template>
