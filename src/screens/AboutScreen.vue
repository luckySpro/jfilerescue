<script setup lang="ts">
import { onMounted, ref } from "vue";
import { api, openPath, pickSavePath } from "../api";
import { dict } from "../i18n";
import { humanBytes, notify } from "../store";

const t = dict;
const diag = ref<Record<string, any>>({});
onMounted(async () => { try { diag.value = await api.diagnostics(); } catch (e) { notify(String(e), "error"); } });

async function exportDiag() {
  const p = await pickSavePath(t.value.about.exportDiag, "jfilerescue-diagnostics.json", [{ name: "JSON", extensions: ["json"] }]);
  if (!p) return;
  // Redacted: only versions, counts and platform facts.
  const d = diag.value;
  const redacted = { app_version: d.app_version, core_version: d.core_version, signatures: d.signatures, signatures_enabled: d.signatures_enabled, webview: d.webview, privilege: { elevated: d.privilege?.elevated, os: d.privilege?.os, arch: d.privilege?.arch, os_version: d.privilege?.os_version, full_disk_access: d.privilege?.full_disk_access, helper_version: d.privilege?.helper_version }, sessions_bytes: d.sessions_bytes, exported_at: new Date().toISOString() };
  try { await navigator.clipboard.writeText(JSON.stringify(redacted, null, 2)); notify(t.value.common.copied, "ok"); } catch { /* ignore */ }
}
const oss = [
  ["Tauri 2", "MIT / Apache-2.0"], ["Vue 3", "MIT"], ["aho-corasick", "MIT / Unlicense"], ["memchr", "MIT / Unlicense"], ["serde / serde_json", "MIT / Apache-2.0"], ["toml", "MIT / Apache-2.0"], ["windows-sys", "MIT / Apache-2.0"], ["plist", "MIT"], ["clap", "MIT / Apache-2.0"],
];
</script>

<template>
  <section>
    <h1>{{ t.about.title }}</h1>
    <div class="grid-2">
      <div class="card">
        <h2>{{ t.about.components }}</h2>
        <table><tbody>
          <tr><td>{{ t.about.gui }}</td><td class="mono">{{ diag.app_version ?? '…' }}</td><td><span class="chip ok"><span class="dot"></span>{{ t.about.ok }}</span></td></tr>
          <tr><td>{{ t.about.core }}</td><td class="mono">{{ diag.core_version ?? '…' }}</td><td><span class="chip" :class="diag.core_version === diag.app_version ? 'ok' : 'warn'"><span class="dot"></span>{{ t.about.ok }}</span></td></tr>
          <tr><td>{{ t.about.sigs }}</td><td class="mono">{{ diag.signatures_enabled ?? '…' }} / {{ diag.signatures ?? '…' }} {{ t.about.items }}</td><td><span class="chip ok"><span class="dot"></span>{{ t.about.loaded }}</span></td></tr>
          <tr><td>{{ t.about.webview }}</td><td class="mono">{{ diag.webview || '—' }}</td><td><span class="chip ok"><span class="dot"></span>{{ t.about.ok }}</span></td></tr>
          <tr><td>{{ t.about.sessionsUsage }}</td><td class="mono">{{ humanBytes(diag.sessions_bytes) }}</td><td></td></tr>
        </tbody></table>
        <div class="row mt"><button class="btn" @click="exportDiag">{{ t.about.exportDiag }}</button><button class="btn" @click="openPath(diag.data_dir)">{{ t.about.openLogs }}</button></div>
        <p class="small muted mt-s">{{ t.about.diagNote }}</p>
      </div>
      <div class="card">
        <h2>{{ t.about.about }}</h2>
        <p><b>{{ t.app.name }}</b> {{ diag.app_version ?? '' }} · {{ t.app.tagline }}<br /><span class="muted">{{ t.app.org }} · 473276@qq.com</span></p>
        <p class="small muted">{{ t.about.platforms }}</p>
        <h2 class="mt">{{ t.about.oss }}</h2>
        <table><tbody><tr v-for="[n, l] in oss" :key="n"><td>{{ n }}</td><td class="small muted">{{ l }}</td></tr></tbody></table>
        <div class="row mt"><a class="btn" href="https://github.com/luckySpro/jfilerescue" target="_blank" rel="noopener">{{ t.about.source }}</a></div>
      </div>
    </div>
  </section>
</template>
