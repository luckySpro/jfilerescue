<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { api, type FoundFile, type Phase, type ScanStatus } from "../api";
import { dict } from "../i18n";
import { currentSession, fmtDuration, go, humanBytes, notify, selection, setup } from "../store";

const t = dict;
const status = ref<ScanStatus | null>(null);
const startedAt = ref(Date.now());
const elapsed = ref(0);
const log = ref<string[]>([]);
const recent = ref<FoundFile[]>([]);
const showStop = ref(false);
let timer: number | undefined;
let lastPhase: Phase | null = null;
let lastFound = 0;
let speedSamples: { t: number; b: number }[] = [];

const phases: Phase[] = ["boot", "metadata", "carve", "finalize"];
function phaseState(p: Phase) {
  const cur = status.value?.progress.phase ?? "idle";
  if (cur === "done" || cur === "cancelled" || cur === "failed") return cur === "done" ? "done" : phases.indexOf(p) < phases.indexOf(lastPhase ?? "boot") ? "done" : "";
  const ci = phases.indexOf(cur);
  const pi = phases.indexOf(p);
  return pi < ci ? "done" : pi === ci ? "run" : "";
}
const pct = computed(() => {
  const p = status.value?.progress;
  if (!p || !p.total) return 0;
  return Math.min(100, Math.round((p.done / p.total) * 100));
});
const speed = computed(() => {
  if (speedSamples.length < 2) return 0;
  const a = speedSamples[0], b = speedSamples[speedSamples.length - 1];
  return (b.b - a.b) / Math.max(0.001, (b.t - a.t) / 1000);
});
const remaining = computed(() => {
  const p = status.value?.progress;
  if (!p || !p.total || !p.done || elapsed.value < 3) return null;
  const rate = p.done / elapsed.value;
  return rate > 0 ? (p.total - p.done) / rate : null;
});
const finished = computed(() => status.value && !status.value.running);
const phaseLabel = (p: Phase) => (t.value.scan.phases as Record<string, string>)[p] ?? p;

function stamp() {
  const d = new Date();
  return `[${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}]`;
}

async function poll() {
  try {
    const s = await api.scanProgress();
    status.value = s;
    elapsed.value = (Date.now() - startedAt.value) / 1000;
    const p = s.progress;
    speedSamples.push({ t: Date.now(), b: p.bytes_read });
    if (speedSamples.length > 10) speedSamples.shift();
    if (p.phase !== lastPhase) {
      if (phases.includes(p.phase)) log.value.push(`${stamp()} ${phaseLabel(p.phase)} ${p.message ? "· " + p.message : ""}`);
      if (p.phase === "done") log.value.push(`${stamp()} ${t.value.scan.finished}: ${p.found}`);
      if (p.phase === "failed") log.value.push(`${stamp()} ${t.value.scan.failed}: ${s.error ?? ""}`);
      if (p.phase === "cancelled") log.value.push(`${stamp()} ${t.value.scan.cancelled}`);
      lastPhase = p.phase;
    } else if (p.message && !log.value[log.value.length - 1]?.endsWith(p.message)) {
      log.value.push(`${stamp()} ${p.message}`);
    }
    if (p.found !== lastFound) lastFound = p.found;
    if (s.recent.length) recent.value = s.recent;
    if (log.value.length > 200) log.value.splice(0, log.value.length - 200);
    if (!s.running) {
      window.clearInterval(timer);
      if (s.error) notify(s.error, "error");
      else if (s.session_id && p.phase === "done") {
        currentSession.value = await api.loadSession(s.session_id);
        selection.clear();
        // Peek at the first few results for the live list.
        const page = await api.querySession(s.session_id, { limit: 6 });
        recent.value = page.files;
      }
    }
  } catch (e) {
    notify(String(e), "error");
    window.clearInterval(timer);
  }
}

onMounted(() => {
  poll();
  timer = window.setInterval(poll, 700);
});
onBeforeUnmount(() => window.clearInterval(timer));

async function togglePause() {
  const paused = !status.value?.progress.paused;
  await api.pauseScan(paused);
  log.value.push(`${stamp()} ${paused ? t.value.scan.pause : t.value.scan.resume}`);
}
async function stop() {
  showStop.value = false;
  await api.cancelScan();
}
async function viewResults() {
  if (!currentSession.value && status.value?.session_id) currentSession.value = await api.loadSession(status.value.session_id);
  go("results");
}
</script>

<template>
  <section>
    <h1>{{ finished ? (status?.error ? t.scan.failed : status?.progress.phase === 'cancelled' ? t.scan.cancelled : t.scan.finished) : t.scan.title }} <span class="muted" style="font-weight: 400; font-size: 15px">{{ setup.source?.displayName }}</span></h1>
    <p class="sub">{{ t.scan.target }}：<span class="mono">{{ setup.mode === 'directory' ? setup.directory : t.mode.full }}</span> · {{ setup.mode === 'directory' ? t.mode.dir : t.mode.full }} · {{ setup.depth === 'deep' ? t.sessions.deep : t.sessions.quick }}</p>
    <div class="card">
      <div class="phases">
        <div v-for="p in phases" :key="p" class="phase" :class="phaseState(p)"><b>{{ phaseLabel(p) }}</b>{{ phaseState(p) === 'done' ? t.scan.done : phaseState(p) === 'run' ? `${t.scan.running} · ${pct}%` : t.scan.waiting }}</div>
      </div>
      <div class="bigbar"><i :style="{ width: (finished && !status?.error ? 100 : pct) + '%' }"></i></div>
      <div class="row" style="justify-content: space-between">
        <span class="small muted">{{ t.scan.elapsed }} {{ fmtDuration(elapsed) }}<template v-if="remaining != null && !finished"> · {{ t.scan.remaining }} ≈ {{ fmtDuration(remaining) }}</template></span>
        <span class="small muted">{{ t.scan.speed }} {{ humanBytes(speed) }}/s · {{ t.scan.readErr }} {{ status?.progress.read_errors ?? 0 }}</span>
      </div>
      <div class="stats mt">
        <div class="stat"><div class="v">{{ status?.progress.found ?? 0 }}</div><div class="k">{{ t.scan.found }}</div></div>
        <div class="stat"><div class="v">{{ humanBytes(status?.progress.bytes_read ?? 0) }}</div><div class="k">{{ t.scan.bytes }}</div></div>
        <div class="stat" v-if="currentSession"><div class="v">{{ currentSession.summary.with_name }}</div><div class="k">{{ t.scan.withName }}</div></div>
        <div class="stat" v-if="currentSession"><div class="v">{{ currentSession.summary.carved }}</div><div class="k">{{ t.scan.carved }}</div></div>
        <div class="stat" v-if="currentSession"><div class="v" style="color: var(--ok)">{{ currentSession.summary.total ? Math.round((currentSession.summary.high / currentSession.summary.total) * 100) : 0 }}%</div><div class="k">{{ t.results.qLabel.high }}</div></div>
      </div>
      <div class="row mt">
        <template v-if="!finished">
          <button class="btn" @click="togglePause">{{ status?.progress.paused ? '▶ ' + t.scan.resume : '⏸ ' + t.scan.pause }}</button>
          <button class="btn danger" @click="showStop = true">■ {{ t.scan.stop }}</button>
          <button class="btn" @click="go('home')">{{ t.scan.bg }}</button>
          <span class="chip warn"><span class="dot"></span>{{ t.scan.locked }}</span>
        </template>
        <template v-else>
          <button class="btn primary lg" v-if="!status?.error && status?.progress.phase === 'done'" @click="viewResults">{{ t.scan.viewResults }}</button>
          <button class="btn" @click="go('home')">{{ t.results.rescan }}</button>
          <span v-if="status?.error" class="chip bad"><span class="dot"></span>{{ status.error }}</span>
        </template>
      </div>
    </div>
    <div class="grid-2 mt">
      <div class="card">
        <h2>{{ t.scan.recent }}</h2>
        <div v-if="recent.length === 0" class="empty small">…</div>
        <table v-else><tbody>
          <tr v-for="f in recent" :key="f.id"><td class="ellipsis" style="max-width: 220px" :class="{ mono: f.source === 'carved' }">{{ f.name }}</td><td class="muted small ellipsis">{{ f.dir ?? t.results.noPath }}</td><td><span class="q" :class="f.quality"></span>{{ t.results.qLabel[f.quality] }}</td></tr>
        </tbody></table>
      </div>
      <div class="card">
        <h2>{{ t.scan.log }}</h2>
        <div class="log">{{ log.join("\n") }}</div>
      </div>
    </div>

    <div v-if="showStop" class="modal-bg" @click.self="showStop = false" @keydown.esc="showStop = false">
      <div class="modal" role="dialog" aria-modal="true">
        <div class="hd">{{ t.scan.stopConfirm }}</div>
        <div class="bd">{{ t.scan.stopBody }}</div>
        <div class="ft"><button class="btn" @click="showStop = false" autofocus>{{ t.scan.stopKeep }}</button><button class="btn danger" @click="stop">{{ t.scan.stopDo }}</button></div>
      </div>
    </div>
  </section>
</template>
