<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { api, type WriterProcess } from "../api";
import { dict } from "../i18n";
import { disks, notify, privilege, setup } from "../store";

const t = dict;
const emit = defineEmits<{ (e: "refresh"): void }>();
const writers = ref<WriterProcess[]>([]);
const busy = ref(false);
const isMac = computed(() => privilege.value?.os === "macos");
const elevated = computed(() => privilege.value?.elevated ?? false);
const hasEncrypted = computed(() => disks.value.some((d) => d.volumes.some((v) => v.encrypted)));

async function recheck() {
  busy.value = true;
  try {
    privilege.value = await api.privilegeState();
    writers.value = await api.busyWriters(setup.source?.path ?? "");
    emit("refresh");
  } catch (e) { notify(String(e), "error"); } finally { busy.value = false; }
}
onMounted(recheck);
async function relaunch() { try { await api.relaunchElevated(); } catch (e) { notify(String(e), "error"); } }
async function openPrivacy() { try { await api.openPrivacySettings(); } catch (e) { notify(String(e), "error"); } }
</script>

<template>
  <section>
    <h1>{{ t.perm.title }}</h1>
    <p class="sub">{{ t.perm.sub }}</p>
    <div class="notice bad" style="margin-bottom: 14px"><span>⛔</span><span><b>{{ t.risk.stopWriting }}</b></span></div>

    <div v-if="!isMac" class="steps">
      <div class="wstep" :class="elevated ? 'done' : 'bad'"><div class="n">{{ elevated ? '✓' : '!' }}</div><div><b>{{ t.perm.win.admin }}</b><div class="small muted">{{ elevated ? t.perm.win.adminOk : t.perm.win.adminNo }}</div></div><button v-if="!elevated" class="btn primary" @click="relaunch">{{ t.perm.win.relaunch }}</button></div>
      <div class="wstep" :class="{ done: true }"><div class="n">2</div><div><b>{{ t.perm.win.bitlocker }}</b><div class="small muted">{{ t.perm.win.bitlockerDesc }}</div></div><span class="chip" :class="hasEncrypted ? 'warn' : 'ok'"><span class="dot"></span>{{ hasEncrypted ? t.source.encrypted : t.perm.ok }}</span></div>
      <div class="wstep"><div class="n">3</div><div><b>{{ t.perm.win.writers }}</b><div class="small muted">{{ writers.length ? t.perm.win.writersDesc : t.perm.win.noWriters }}</div><div class="row mt-s"><span v-for="w in writers" :key="w.pid" class="chip warn"><span class="dot"></span>{{ w.name }} ({{ w.pid }}) · {{ w.reason }}</span></div></div></div>
      <div class="notice info"><span>ⓘ</span><span>{{ t.perm.win.uac }}</span></div>
    </div>

    <div v-else class="steps">
      <div class="wstep" :class="elevated || privilege?.helper_version ? 'done' : ''"><div class="n">{{ elevated || privilege?.helper_version ? '✓' : '1' }}</div><div><b>{{ t.perm.mac.admin }}</b><div class="small muted">{{ t.perm.mac.adminDesc }}</div></div></div>
      <div class="wstep" :class="privilege?.full_disk_access === true ? 'done' : privilege?.full_disk_access === false ? 'bad' : ''"><div class="n">{{ privilege?.full_disk_access === true ? '✓' : '2' }}</div><div><b>{{ t.perm.mac.fda }}</b><div class="small muted">{{ t.perm.mac.fdaDesc }}</div></div><button class="btn" @click="openPrivacy">{{ t.perm.mac.openSettings }}</button></div>
      <div class="wstep"><div class="n">3</div><div><b>{{ t.perm.mac.fv }}</b><div class="small muted">{{ t.perm.mac.fvDesc }}</div></div><span class="chip" :class="hasEncrypted ? 'warn' : 'ok'"><span class="dot"></span>{{ hasEncrypted ? t.perm.mac.limited : t.perm.ok }}</span></div>
      <div class="wstep"><div class="n">4</div><div><b>{{ t.perm.win.writers }}</b><div class="small muted">{{ writers.length ? t.perm.win.writersDesc : t.perm.win.noWriters }}</div><div class="row mt-s"><span v-for="w in writers" :key="w.pid" class="chip warn"><span class="dot"></span>{{ w.name }} ({{ w.pid }}) · {{ w.reason }}</span></div></div></div>
    </div>

    <div class="card mt">
      <h2>{{ t.perm.status }}</h2>
      <div class="row">
        <span class="chip" :class="elevated ? 'ok' : 'warn'"><span class="dot"></span>{{ t.perm.elevatedChip }}：{{ elevated ? t.perm.granted : t.perm.notGranted }}</span>
        <span class="chip" :class="disks.length ? 'ok' : 'warn'"><span class="dot"></span>{{ t.perm.devices }}：{{ disks.length }}</span>
        <span v-if="isMac" class="chip" :class="privilege?.helper_version ? 'ok' : ''"><span class="dot"></span>{{ t.perm.helper }}：{{ privilege?.helper_version ?? t.perm.unconfirmed }}</span>
        <span v-for="d in privilege?.details ?? []" :key="d" class="chip">{{ d }}</span>
      </div>
      <div class="row mt-s"><button class="btn" @click="recheck" :disabled="busy"><span v-if="busy" class="spin"></span> {{ t.perm.recheck }}</button></div>
    </div>
  </section>
</template>
