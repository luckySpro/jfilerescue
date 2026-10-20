<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { api, pickSavePath, type FoundFile, type Preview, type Quality, type ResultFilter, type TreeNode } from "../api";
import { dict } from "../i18n";
import { currentSession, fmtTime, go, humanBytes, notify, selectedFiles, selection } from "../store";

const t = dict;
const session = currentSession;
const browseBy = ref<"dir" | "type" | "time">("dir");
const tree = ref<TreeNode | null>(null);
const types = ref<Record<string, number>>({});
const files = ref<FoundFile[]>([]);
const total = ref(0);
const loading = ref(false);
const active = ref<FoundFile | null>(null);
const preview = ref<Preview | null>(null);
const previewLoading = ref(false);
const expanded = ref(new Set<string>());

const filter = ref<ResultFilter & { group: "all" | "dir" | "carved" | "damaged" }>({ query: "", min_quality: null, min_size: null, deleted_after: null, source: null, dir_prefix: null, ext: null, hide_empty: true, offset: 0, limit: 300, group: "all" });
const qualityPick = ref<"all" | "high" | "highmed">("all");
const sizePick = ref<"all" | "1m" | "100m">("all");
const timePick = ref<"all" | "24h" | "7d" | "30d">("all");

const PAGE = 300;

function buildFilter(): ResultFilter {
  const f = filter.value;
  const now = Math.floor(Date.now() / 1000);
  const damagedQuery = f.group === "damaged" ? "FILE_#" : "";
  return {
    query: f.query || damagedQuery,
    min_quality: qualityPick.value === "high" ? "high" : qualityPick.value === "highmed" ? "medium" : null,
    min_size: sizePick.value === "1m" ? 1 << 20 : sizePick.value === "100m" ? 100 << 20 : null,
    deleted_after: timePick.value === "24h" ? now - 86400 : timePick.value === "7d" ? now - 7 * 86400 : timePick.value === "30d" ? now - 30 * 86400 : null,
    source: f.group === "carved" ? "carved" : f.group === "dir" ? "metadata" : null,
    dir_prefix: f.group === "dir" ? f.dir_prefix : null,
    ext: f.ext,
    hide_empty: f.hide_empty,
    offset: f.offset,
    limit: PAGE,
  };
}

async function load(append = false) {
  if (!session.value) return;
  loading.value = true;
  try {
    if (!append) filter.value.offset = 0;
    const page = await api.querySession(session.value.id, buildFilter());
    total.value = page.total;
    files.value = append ? files.value.concat(page.files) : page.files;
    if (!append && !selection.size && files.value.length) defaultSelect(files.value);
  } catch (e) {
    notify(String(e), "error");
  } finally {
    loading.value = false;
  }
}
function more() {
  filter.value.offset = files.value.length;
  load(true);
}
function defaultSelect(list: FoundFile[]) {
  for (const f of list) if (f.quality !== "low") selection.add(f.id);
}

onMounted(async () => {
  if (!session.value) { go("sessions"); return; }
  try {
    [tree.value, types.value] = await Promise.all([api.sessionTree(session.value.id), api.sessionTypes(session.value.id)]);
    for (const c of tree.value.children) expanded.value.add(c.path);
  } catch (e) { notify(String(e), "error"); }
  await load();
});
watch([() => filter.value.query, qualityPick, sizePick, timePick, () => filter.value.hide_empty, () => filter.value.group, () => filter.value.dir_prefix, () => filter.value.ext], () => load());

function pickDir(path: string | null) { filter.value.group = path == null ? "all" : "dir"; filter.value.dir_prefix = path; filter.value.ext = null; }
function pickGroup(g: "carved" | "damaged") { filter.value.group = g; filter.value.dir_prefix = null; filter.value.ext = null; }
function pickType(ext: string | null) { filter.value.group = "all"; filter.value.dir_prefix = null; filter.value.ext = ext; }
function toggleExpand(p: string) { expanded.value.has(p) ? expanded.value.delete(p) : expanded.value.add(p); }

function toggle(f: FoundFile) { selection.has(f.id) ? selection.delete(f.id) : selection.add(f.id); }
const allPageSelected = computed(() => files.value.length > 0 && files.value.every((f) => selection.has(f.id)));
function togglePage() { if (allPageSelected.value) for (const f of files.value) selection.delete(f.id); else for (const f of files.value) selection.add(f.id); }
async function selectAllHigh() {
  if (!session.value) return;
  const page = await api.querySession(session.value.id, { min_quality: "high", hide_empty: true, limit: 1_000_000 });
  for (const f of page.files) selection.add(f.id);
}
const selectedCount = computed(() => selection.size);
const selectedBytes = computed(() => files.value.filter((f) => selection.has(f.id)).reduce((a, f) => a + f.size, 0));

async function show(f: FoundFile) {
  active.value = f;
  preview.value = null;
  if (!session.value) return;
  previewLoading.value = true;
  try { preview.value = await api.previewFile(session.value.id, f.id); } catch (e) { preview.value = { kind: "hex", mime: null, data_base64: null, text: null, hex: String(e) }; } finally { previewLoading.value = false; }
}

async function exportCsv() {
  if (!session.value) return;
  const p = await pickSavePath(t.value.results.exportCsv, `jfilerescue-${session.value.id}.csv`, [{ name: "CSV", extensions: ["csv"] }]);
  if (!p) return;
  try { const n = await api.exportCsv(session.value.id, p); notify(`${n} → ${p}`, "ok"); } catch (e) { notify(String(e), "error"); }
}

async function toRecover() {
  if (!session.value) return;
  if (selection.size === 0) return notify(t.value.recover.noSelection, "error");
  // Materialize the selected files (all of them, not just the loaded page).
  const all = await api.querySession(session.value.id, { hide_empty: false, limit: 1_000_000 });
  selectedFiles.value = all.files.filter((f) => selection.has(f.id));
  go("recover");
}
const qLabel = (q: Quality) => t.value.results.qLabel[q];
</script>

<template>
  <section v-if="session">
    <div class="row" style="justify-content: space-between; margin-bottom: 12px">
      <div><h1 style="margin: 0">{{ t.results.title }} · {{ session.device.display_name }}</h1><span class="small muted">{{ t.results.completed }} {{ fmtTime(session.finished_at) }} · {{ session.summary.total }} {{ t.results.files }} · {{ humanBytes(session.summary.bytes) }} · {{ t.results.saved }}</span><div v-if="session.warning" class="chip warn mt-s"><span class="dot"></span>{{ session.warning }}</div></div>
      <div class="row"><button class="btn" @click="exportCsv">{{ t.results.exportCsv }}</button><button class="btn" @click="go('home')">{{ t.results.rescan }}</button></div>
    </div>
    <div class="results">
      <div class="card">
        <div class="hd">{{ t.results.browse }} <select v-model="browseBy" style="margin-left: auto; padding: 2px 6px; font-size: 12px"><option value="dir">{{ t.results.byDir }}</option><option value="type">{{ t.results.byType }}</option><option value="time">{{ t.results.byTime }}</option></select></div>
        <div class="bd tree">
          <button :class="{ on: filter.group === 'all' && !filter.ext }" @click="pickDir(null); pickType(null)">▣ {{ t.results.all }} <span class="cnt">{{ session.summary.total }}</span></button>
          <template v-if="browseBy === 'dir' && tree">
            <TreeBranch v-for="c in tree.children" :key="c.path" :node="c" :depth="0" :expanded="expanded" :current="filter.group === 'dir' ? (filter.dir_prefix ?? null) : null" @pick="pickDir" @toggle="toggleExpand" />
            <button style="margin-top: 8px" :class="{ on: filter.group === 'carved' }" @click="pickGroup('carved')">🧩 {{ t.results.carvedGroup }} <span class="cnt">{{ session.summary.carved }}</span></button>
            <button :class="{ on: filter.group === 'damaged' }" @click="pickGroup('damaged')">⚠ {{ t.results.damagedGroup }} <span class="cnt">{{ session.summary.damaged_name }}</span></button>
          </template>
          <template v-else-if="browseBy === 'type'">
            <button v-for="(n, ext) in types" :key="ext" :class="{ on: filter.ext === ext }" @click="pickType(String(ext))">📄 .{{ ext }} <span class="cnt">{{ n }}</span></button>
          </template>
          <template v-else>
            <button :class="{ on: timePick === '24h' }" @click="timePick = '24h'">{{ t.results.h24 }}</button>
            <button :class="{ on: timePick === '7d' }" @click="timePick = '7d'">{{ t.results.d7 }}</button>
            <button :class="{ on: timePick === '30d' }" @click="timePick = '30d'">{{ t.results.d30 }}</button>
            <button :class="{ on: timePick === 'all' }" @click="timePick = 'all'">{{ t.results.timeAll }}</button>
          </template>
        </div>
      </div>

      <div class="card">
        <div class="filters">
          <input type="search" v-model.trim="filter.query" :placeholder="t.results.search" />
          <select v-model="qualityPick"><option value="all">{{ t.results.quality }}：{{ t.results.qAll }}</option><option value="high">{{ t.results.qHigh }}</option><option value="highmed">{{ t.results.qHighMed }}</option></select>
          <select v-model="sizePick"><option value="all">{{ t.results.size }}：{{ t.results.sizeAll }}</option><option value="1m">&gt; 1 MB</option><option value="100m">&gt; 100 MB</option></select>
          <select v-model="timePick"><option value="all">{{ t.results.time }}：{{ t.results.timeAll }}</option><option value="24h">{{ t.results.h24 }}</option><option value="7d">{{ t.results.d7 }}</option><option value="30d">{{ t.results.d30 }}</option></select>
          <label class="small row" style="gap: 4px"><input type="checkbox" v-model="filter.hide_empty" /> {{ t.results.hideEmpty }}</label>
        </div>
        <div class="bd" style="padding: 0">
          <table class="files">
            <thead><tr><th><input type="checkbox" :checked="allPageSelected" @change="togglePage" /></th><th>{{ t.results.colName }}</th><th>{{ t.results.colPath }}</th><th>{{ t.results.colSize }}</th><th class="col-time">{{ t.results.colTime }}</th><th>{{ t.results.colQuality }}</th><th class="col-src">{{ t.results.colSource }}</th></tr></thead>
            <tbody>
              <tr v-for="f in files" :key="f.id" class="clickable" :class="{ sel: active?.id === f.id }" @click="show(f)">
                <td @click.stop><input type="checkbox" :checked="selection.has(f.id)" @change="toggle(f)" /></td>
                <td class="name" :class="{ mono: f.source === 'carved' || f.name_damaged }" :title="f.name">{{ f.name }}<span v-if="f.name_damaged" class="chip warn" style="margin-left: 6px">{{ t.results.damagedTag }}</span></td>
                <td class="path muted small" :title="f.dir ?? ''">{{ f.dir ?? t.results.noPath }}</td>
                <td class="nowrap">{{ humanBytes(f.size) }}</td>
                <td class="small nowrap col-time">{{ f.deleted_at ? fmtTime(f.deleted_at) : t.results.unknown }}</td>
                <td class="nowrap" :title="f.note"><span class="q" :class="f.quality"></span>{{ qLabel(f.quality) }}<span v-if="f.overwritten_pct" class="small muted"> · {{ f.overwritten_pct }}%</span></td>
                <td class="small nowrap col-src">{{ f.source === 'metadata' ? t.results.srcMeta : t.results.srcCarved }}</td>
              </tr>
            </tbody>
          </table>
          <div v-if="loading" class="empty"><span class="spin"></span> {{ t.results.loading }}</div>
          <div v-else-if="files.length < total" class="empty"><button class="btn" @click="more">{{ t.results.more }} ({{ files.length }} / {{ total }})</button></div>
          <div v-else class="empty small">{{ t.results.matched }} {{ total }}</div>
        </div>
      </div>

      <div class="card">
        <div class="hd">{{ t.results.preview }}</div>
        <div class="bd">
          <div v-if="!active" class="empty small">{{ t.results.previewHint }}</div>
          <template v-else>
            <div class="preview">
              <span v-if="previewLoading" class="spin"></span>
              <img v-else-if="preview?.kind === 'image' && preview.data_base64" :src="`data:${preview.mime};base64,${preview.data_base64}`" alt="" />
              <pre v-else-if="preview?.kind === 'text'">{{ preview.text }}</pre>
              <span v-else>📄 .{{ active.ext || '?' }}</span>
            </div>
            <div class="kv">
              <b>{{ t.results.kvName }}</b><span style="overflow-wrap: anywhere">{{ active.name }}</span>
              <b>{{ t.results.kvPath }}</b><span class="mono">{{ active.dir ?? t.results.noPath }}</span>
              <b>{{ t.results.kvSize }}</b><span>{{ active.size.toLocaleString() }} {{ t.results.bytesUnit }}</span>
              <b>{{ t.results.kvDeleted }}</b><span>{{ active.deleted_at ? fmtTime(active.deleted_at) : t.results.unknown }}</span>
              <b>{{ t.results.kvModified }}</b><span>{{ fmtTime(active.modified_at) }}</span>
              <b>{{ t.results.kvOverwritten }}</b><span>{{ active.extents.length }} · <span :style="{ color: active.overwritten_pct ? 'var(--warn)' : 'var(--ok)' }">{{ active.overwritten_pct }}% {{ t.results.overwritten }}</span></span>
              <b>{{ t.results.kvQuality }}</b><span><span class="q" :class="active.quality"></span>{{ qLabel(active.quality) }}</span>
              <b>{{ t.results.kvNote }}</b><span>{{ active.note }}</span>
            </div>
            <div class="hex" v-if="preview?.hex">{{ preview.hex }}</div>
          </template>
        </div>
      </div>
    </div>
    <div class="footbar">
      <span>{{ t.results.selected }} <b>{{ selectedCount }}</b> · {{ humanBytes(selectedBytes) }}</span>
      <span class="small muted">· {{ t.results.lowDefault }}</span>
      <div style="flex: 1"></div>
      <button class="btn" @click="selectAllHigh">{{ t.results.selectHigh }}</button>
      <button class="btn" @click="selection.clear()">{{ t.results.clear }}</button>
      <button class="btn primary lg" :disabled="selectedCount === 0" @click="toRecover">{{ t.results.toRecover }}</button>
    </div>
  </section>
</template>

<script lang="ts">
import { defineComponent, h, type DefineComponent, type PropType, type VNode } from "vue";
import type { TreeNode as TN } from "../api";
type BranchProps = { node: TN; depth: number; expanded: Set<string>; current: string | null };
// eslint-disable-next-line prefer-const
let TreeBranch: DefineComponent<BranchProps>;
TreeBranch = defineComponent({
  name: "TreeBranch",
  props: { node: { type: Object as PropType<TN>, required: true }, depth: { type: Number, required: true }, expanded: { type: Object as PropType<Set<string>>, required: true }, current: { type: String as PropType<string | null>, default: null } },
  emits: ["pick", "toggle"],
  setup(props, { emit }) {
    return (): VNode[] => {
      const open = props.expanded.has(props.node.path);
      const has = props.node.children.length > 0;
      const row = h("button", { class: { on: props.current?.toLowerCase() === props.node.path.toLowerCase() }, style: { paddingLeft: `${6 + props.depth * 14}px` }, onClick: () => emit("pick", props.node.path), title: props.node.path }, [
        h("span", { style: { display: "inline-block", width: "14px", cursor: "pointer" }, onClick: (e: Event) => { e.stopPropagation(); if (has) emit("toggle", props.node.path); } }, has ? (open ? "▾" : "▸") : ""),
        `📁 ${props.node.name} `,
        h("span", { class: "cnt" }, String(props.node.count)),
      ]);
      const kids: VNode[] = open ? props.node.children.map((c) => h(TreeBranch, { key: c.path, node: c, depth: props.depth + 1, expanded: props.expanded, current: props.current, onPick: (p: string) => emit("pick", p), onToggle: (p: string) => emit("toggle", p) })) : [];
      return [row, ...kids];
    };
  },
}) as unknown as DefineComponent<BranchProps>;
export default { components: { TreeBranch } };
</script>
