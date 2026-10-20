<script setup lang="ts">
import { computed, ref } from "vue";
import { api, pickDirectory, pickFile, pickSavePath, type DiskEntry, type VolumeEntry } from "../api";
import { dict } from "../i18n";
import { disks, disksLoading, go, humanBytes, notify, privilege, setup } from "../store";

const t = dict;
const emit = defineEmits<{ (e: "refresh"): void }>();
const starting = ref(false);
const extInput = ref("");

const isMac = computed(() => privilege.value?.os === "macos");

function selectVolume(d: DiskEntry, v: VolumeEntry | null) {
  if (v) {
    setup.source = { path: v.path, partition: v.partition_index, displayName: `${v.display_name} · ${v.fs_label}`, disk: d, volume: v };
  } else {
    setup.source = { path: d.path, partition: null, displayName: d.display_name, disk: d, volume: null };
    if (d.volumes.length === 0) setup.mode = "full";
  }
}
function isSelected(path: string, partition: number | null) {
  return setup.source?.path === path && setup.source?.partition === partition;
}

async function openImage() {
  const p = await pickFile(t.value.source.image, [{ name: "Disk image", extensions: ["img", "dd", "raw", "bin", "iso", "dmg"] }]);
  if (!p) return;
  const name = p.split(/[\\/]/).pop() ?? p;
  setup.source = { path: p, partition: null, displayName: name, disk: { path: p, display_name: name, model: "", size: 0, sector_size: 512, is_ssd: null, trim_enabled: null, removable: false, health: "ok", health_text: "", table: "", volumes: [], identity: p, capabilities: ["carve", "metadata"] }, volume: null };
}

async function browseDir() {
  const p = await pickDirectory(t.value.settings.dirLabel);
  if (p) setup.directory = p;
}

function addExt() {
  for (const e of extInput.value.split(/[,\s/]+/)) {
    const x = e.trim().replace(/^\./, "").toLowerCase();
    if (x && !setup.extensions.includes(x)) setup.extensions.push(x);
  }
  extInput.value = "";
}
function removeExt(e: string) {
  setup.extensions = setup.extensions.filter((x) => x !== e);
}

const warning = computed(() => {
  const s = setup.source;
  if (!s) return null;
  if (s.volume?.encrypted) return { b: `${s.volume.display_name} ${t.value.source.fv}`, text: t.value.source.fvWarn };
  if (s.disk.is_ssd && s.disk.trim_enabled) return { b: `${s.disk.display_name} ${t.value.source.ssd}`, text: t.value.source.ssdWarn };
  return null;
});

function capText(d: DiskEntry, v: VolumeEntry | null) {
  if (v) return v.fs === "unknown" ? t.value.source.carveOnly : t.value.source.metaCarve;
  return d.capabilities.includes("metadata") ? t.value.source.metaCarve : t.value.source.carveOnly;
}

async function start() {
  if (!setup.source) return notify(t.value.settings.needSource, "error");
  if (setup.mode === "directory" && !setup.directory.trim()) return notify(t.value.settings.needDir, "error");
  starting.value = true;
  try {
    let source = setup.source.path;
    let partition = setup.source.partition;
    if (setup.makeImage && !setup.source.disk.path.endsWith(".img")) {
      const dest = await pickSavePath(t.value.settings.imageDest, `${setup.source.displayName.replace(/[^\w\u4e00-\u9fff-]+/g, "_")}.img`, [{ name: "Disk image", extensions: ["img"] }]);
      if (!dest) { starting.value = false; return; }
      await api.createImage(source, dest);
      // Poll until done, then scan the image instead.
      for (;;) {
        const st = await api.recoverProgress();
        if (!st.running) { if (st.error) throw new Error(st.error); break; }
        await new Promise((r) => setTimeout(r, 500));
      }
      source = dest;
      partition = setup.source.volume ? null : partition;
      setup.imagePath = dest;
    }
    await api.startScan(source, partition, {
      mode: setup.mode, depth: setup.depth, directory: setup.mode === "directory" ? setup.directory : null, extensions: setup.extensions,
      include_damaged_names: setup.includeDamaged, carve_unallocated_only: setup.carveUnallocatedOnly, threads: setup.threads,
    }, setup.source.displayName);
    go("scan");
  } catch (e) {
    notify(String(e), "error");
  } finally {
    starting.value = false;
  }
}
</script>

<template>
  <section>
    <h1>{{ t.mode.title }}</h1>
    <p class="sub">{{ t.mode.sub }}</p>
    <div class="grid-2">
      <div class="mode" :class="{ selected: setup.mode === 'directory' }" @click="setup.mode = 'directory'" role="radio" :aria-checked="setup.mode === 'directory'" tabindex="0" @keydown.enter="setup.mode = 'directory'">
        <span class="chip ok tag">{{ t.mode.recommended }}</span>
        <div class="t"><span class="ico">📁</span>{{ t.mode.dir }}</div>
        <p>{{ t.mode.dirDesc }}</p>
        <p class="small">{{ t.mode.dirFor }}</p>
      </div>
      <div class="mode" :class="{ selected: setup.mode === 'full' }" @click="setup.mode = 'full'" role="radio" :aria-checked="setup.mode === 'full'" tabindex="0" @keydown.enter="setup.mode = 'full'">
        <div class="t"><span class="ico">💽</span>{{ t.mode.full }}</div>
        <p>{{ t.mode.fullDesc }}</p>
        <p class="small">{{ t.mode.fullFor }}</p>
      </div>
    </div>

    <div class="grid-main-side mt">
      <div class="card">
        <h2>{{ t.source.title }} <span class="small muted" style="font-weight: 400">{{ t.source.ro }}</span></h2>
        <div v-if="disksLoading" class="empty"><span class="spin"></span> {{ t.source.loading }}</div>
        <div v-else-if="disks.length === 0" class="empty">{{ t.source.empty }}</div>
        <table v-else>
          <thead><tr><th></th><th>{{ t.source.disk }}</th><th>{{ t.source.fs }}</th><th>{{ t.source.cap }}</th><th>{{ t.source.status }}</th><th>{{ t.source.caps }}</th></tr></thead>
          <tbody>
            <template v-for="d in disks" :key="d.path">
              <tr class="clickable" :class="{ sel: isSelected(d.path, null) }" @click="selectVolume(d, null)">
                <td><input type="radio" name="src" :checked="isSelected(d.path, null)" /></td>
                <td><b>{{ d.display_name }}</b><div class="small muted">{{ d.table }}{{ d.is_ssd === true ? " · SSD" : d.is_ssd === false ? " · HDD" : "" }}{{ d.removable ? " · USB" : "" }}</div></td>
                <td class="muted">—</td>
                <td><div class="cap"><span class="small">{{ humanBytes(d.size) }}</span></div></td>
                <td><span class="chip" :class="d.health"><span class="dot"></span>{{ d.health_text || t.source.health[d.health] }}</span></td>
                <td class="small">{{ capText(d, null) }}</td>
              </tr>
              <tr v-for="v in d.volumes" :key="v.path + v.partition_index" class="clickable" :class="{ sel: isSelected(v.path, v.partition_index) }" @click="selectVolume(d, v)">
                <td><input type="radio" name="src" :checked="isSelected(v.path, v.partition_index)" /></td>
                <td style="padding-left: 28px">{{ v.display_name }}<span v-if="v.is_system" class="chip" style="margin-left: 6px">{{ t.source.system }}</span><div v-for="n in v.notes" :key="n" class="small muted">{{ n }}</div></td>
                <td>{{ v.fs_label }}</td>
                <td><div class="cap"><span class="small">{{ v.used != null ? humanBytes(v.used) + " / " : "" }}{{ humanBytes(v.size) }}</span><div class="bar" :class="v.encrypted ? 'warn' : 'ok'" v-if="v.used != null"><i :style="{ width: Math.min(100, (v.used / Math.max(1, v.size)) * 100) + '%' }"></i></div></div></td>
                <td><span class="chip" :class="v.encrypted ? 'warn' : 'ok'"><span class="dot"></span>{{ v.encrypted ? (v.encryption_unlocked ? t.source.unlocked : t.source.locked) : t.source.health.ok }}</span></td>
                <td class="small">{{ capText(d, v) }}</td>
              </tr>
            </template>
          </tbody>
        </table>
        <div class="row mt-s">
          <button class="btn" @click="emit('refresh')">↻ {{ t.source.refresh }}</button>
          <button class="btn" @click="openImage">{{ t.source.image }}</button>
          <span class="small muted">{{ t.source.noDisk }} <a href="#" @click.prevent="go('perm')">{{ t.source.wizard }}</a></span>
        </div>
        <div v-if="warning" class="notice warn mt"><span>⚠</span><span><span class="b">{{ warning.b }}</span> {{ warning.text }}</span></div>
      </div>

      <div class="card">
        <h2>{{ t.settings.title }}</h2>
        <div v-if="setup.source" class="notice info" style="margin-bottom: 12px"><span>▣</span><span class="ellipsis">{{ setup.source.displayName }}</span></div>
        <div class="field" v-if="setup.mode === 'directory'">
          <label>{{ t.settings.dirLabel }}</label>
          <div class="row"><input type="text" v-model="setup.directory" :placeholder="isMac ? '/Volumes/LaCie/Photos/2025' : 'D:\\项目\\2025-合同'" style="flex: 1" /><button class="btn" @click="browseDir">{{ t.settings.browse }}</button></div>
          <span class="small muted">{{ t.settings.dirHint }}</span>
        </div>
        <div class="field">
          <label>{{ t.settings.depth }}</label>
          <label class="radio" :class="{ on: setup.depth === 'quick' }"><input type="radio" name="depth" value="quick" v-model="setup.depth" /><div><b>{{ t.settings.quick }}</b><span>{{ t.settings.quickDesc }}</span></div></label>
          <label class="radio" :class="{ on: setup.depth === 'deep' }" style="margin-top: 6px"><input type="radio" name="depth" value="deep" v-model="setup.depth" /><div><b>{{ t.settings.deep }}</b><span>{{ t.settings.deepDesc }}</span></div></label>
          <span v-if="setup.mode === 'directory' && setup.depth === 'deep'" class="small muted">{{ t.settings.deepDirNote }}</span>
        </div>
        <div class="field">
          <label>{{ t.settings.types }}</label>
          <div class="row">
            <span v-for="e in setup.extensions" :key="e" class="chip">{{ e }} <button class="x" @click="removeExt(e)" aria-label="remove">×</button></span>
            <input type="text" v-model="extInput" @keydown.enter.prevent="addExt" placeholder="jpg, docx" style="width: 110px" />
            <button class="btn sm" @click="addExt">{{ t.settings.add }}</button>
          </div>
        </div>
        <div class="field">
          <label>{{ t.settings.optional }}</label>
          <div class="row" style="justify-content: space-between"><span class="small">{{ t.settings.imageOpt }}</span><button class="switch" :class="{ on: setup.makeImage }" @click="setup.makeImage = !setup.makeImage" role="switch" :aria-checked="setup.makeImage"></button></div>
          <div class="row" style="justify-content: space-between"><span class="small">{{ t.settings.damaged }}</span><button class="switch" :class="{ on: setup.includeDamaged }" @click="setup.includeDamaged = !setup.includeDamaged" role="switch" :aria-checked="setup.includeDamaged"></button></div>
        </div>
        <button class="btn primary lg" style="width: 100%; justify-content: center" :disabled="starting || !setup.source" @click="start"><span v-if="starting" class="spin"></span>▶ {{ t.settings.start }}</button>
        <p class="small muted mt-s">{{ t.settings.estimate }}</p>
      </div>
    </div>
  </section>
</template>
