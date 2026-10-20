<script setup lang="ts">
import { onMounted, ref } from "vue";
import { api, confirmDialog, type SessionMeta } from "../api";
import { dict } from "../i18n";
import { currentSession, fmtTime, go, humanBytes, notify, selection } from "../store";

const t = dict;
const list = ref<SessionMeta[]>([]);
const usage = ref(0);
const loading = ref(true);

async function load() {
  loading.value = true;
  try { [list.value, usage.value] = await Promise.all([api.listSessions(), api.sessionsBytes()]); } catch (e) { notify(String(e), "error"); } finally { loading.value = false; }
}
onMounted(load);

async function open(m: SessionMeta) { currentSession.value = m; selection.clear(); go("results"); }
async function del(m: SessionMeta) {
  if (!(await confirmDialog(t.value.sessions.delConfirm, t.value.common.delete))) return;
  try { await api.deleteSession(m.id); if (currentSession.value?.id === m.id) currentSession.value = null; await load(); } catch (e) { notify(String(e), "error"); }
}
function stateClass(s: string) { return s === "completed" ? "ok" : s === "running" ? "info" : s === "aborted" ? "warn" : "bad"; }
</script>

<template>
  <section>
    <h1>{{ t.sessions.title }}</h1>
    <p class="sub">{{ t.sessions.sub }}</p>
    <div class="card">
      <div v-if="loading" class="empty"><span class="spin"></span></div>
      <div v-else-if="list.length === 0" class="empty">{{ t.sessions.empty }}</div>
      <table v-else>
        <thead><tr><th>{{ t.sessions.time }}</th><th>{{ t.sessions.source }}</th><th>{{ t.sessions.mode }}</th><th>{{ t.sessions.found }}</th><th>{{ t.sessions.recovered }}</th><th>{{ t.sessions.status }}</th><th></th></tr></thead>
        <tbody>
          <tr v-for="m in list" :key="m.id">
            <td style="white-space: nowrap">{{ fmtTime(m.created_at) }}</td>
            <td>{{ m.device.display_name }} · {{ m.fs }}<div v-if="m.request.directory" class="small muted mono">{{ m.request.directory }}</div></td>
            <td>{{ m.request.mode === 'directory' ? t.sessions.modeDir : t.sessions.modeFull }} · {{ m.request.depth === 'deep' ? t.sessions.deep : t.sessions.quick }}</td>
            <td>{{ m.summary.total }}</td>
            <td>{{ m.recovered }}</td>
            <td><span class="chip" :class="stateClass(m.state)"><span class="dot"></span>{{ t.sessions.st[m.state] }}</span><div v-if="m.error" class="small muted">{{ m.error }}</div></td>
            <td style="white-space: nowrap"><button class="btn sm" @click="open(m)" :disabled="m.state === 'running' || m.state === 'failed'">{{ t.sessions.open }}</button> <button class="btn sm" @click="del(m)" :disabled="m.state === 'running'">{{ t.sessions.del }}</button></td>
          </tr>
        </tbody>
      </table>
      <p class="small muted mt-s">{{ t.sessions.usage }} {{ humanBytes(usage) }}</p>
    </div>
  </section>
</template>
