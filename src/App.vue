<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { api, isTauri } from "./api";
import { dict, locale, setLocale, type Locale } from "./i18n";
import { disks, disksLoading, go, notify, privilege, screen, setup, toast, toggleTheme, type Screen } from "./store";
import HomeScreen from "./screens/HomeScreen.vue";
import ScanScreen from "./screens/ScanScreen.vue";
import ResultsScreen from "./screens/ResultsScreen.vue";
import RecoverScreen from "./screens/RecoverScreen.vue";
import SessionsScreen from "./screens/SessionsScreen.vue";
import SettingsScreen from "./screens/SettingsScreen.vue";
import PermScreen from "./screens/PermScreen.vue";
import AboutScreen from "./screens/AboutScreen.vue";

const t = dict;
const flow: Screen[] = ["home", "scan", "results", "recover"];
const stepIndex = computed(() => flow.indexOf(screen.value));
const version = ref("0.1.0");

const privChip = computed(() => {
  const p = privilege.value;
  if (!p) return { cls: "", text: "…" };
  if (p.os === "macos") {
    if (p.full_disk_access === false) return { cls: "warn", text: t.value.top.fdaPending };
    return { cls: "ok", text: t.value.top.elevated };
  }
  return p.elevated ? { cls: "ok", text: t.value.top.elevated } : { cls: "warn", text: t.value.top.notElevated };
});
const osChip = computed(() => {
  const p = privilege.value;
  if (!p) return "";
  const os = p.os === "macos" ? "macOS" : p.os === "windows" ? "Windows" : p.os;
  const arch = p.arch === "aarch64" ? "Apple Silicon" : p.arch === "x86_64" ? "x64" : p.arch;
  return `${os} · ${arch}`;
});
const modeText = computed(() => (setup.source ? (setup.mode === "directory" ? t.value.mode.dir : t.value.mode.full) : t.value.top.none));

async function refreshDisks() {
  disksLoading.value = true;
  try {
    disks.value = await api.listDisks();
  } catch (e) {
    notify(String(e), "error");
  } finally {
    disksLoading.value = false;
  }
}

onMounted(async () => {
  try {
    privilege.value = await api.privilegeState();
  } catch (e) {
    notify(String(e), "error");
  }
  await refreshDisks();
  try {
    const d = (await api.diagnostics()) as { app_version?: string };
    if (d.app_version) version.value = d.app_version;
  } catch { /* ignore */ }
  // Resume view of a running scan after reload.
  try {
    const s = await api.scanProgress();
    if (s.running) go("scan");
  } catch { /* ignore */ }
});

const langs: { v: Locale; l: string }[] = [
  { v: "system", l: "跟随系统 / System" }, { v: "zh-CN", l: "简体中文" }, { v: "zh-TW", l: "繁體中文" }, { v: "en", l: "English" }, { v: "ja", l: "日本語" },
];
</script>

<template>
  <div class="app" :class="{ 'with-banner': !isTauri }">
    <div v-if="!isTauri" class="demo-banner">{{ t.top.demo }}</div>
    <header class="topbar">
      <div class="brand"><img src="/icon.svg" alt="" /><span class="name">{{ t.app.name }}</span></div>
      <span class="chip info hide-narrow">{{ t.top.mode }}：{{ modeText }}</span>
      <div class="spacer"></div>
      <span class="chip" :class="privChip.cls"><span class="dot"></span>{{ privChip.text }}</span>
      <span class="chip hide-narrow" v-if="osChip">{{ osChip }}</span>
      <select :value="locale" @change="setLocale(($event.target as HTMLSelectElement).value as Locale)" :title="t.top.lang">
        <option v-for="l in langs" :key="l.v" :value="l.v">{{ l.l }}</option>
      </select>
      <button class="btn" @click="toggleTheme" :title="t.top.theme">◐</button>
    </header>

    <nav class="sidebar">
      <div class="group">{{ t.nav.flow }}</div>
      <button v-for="(s, i) in flow" :key="s" class="nav" :data-s="s" :title="t.nav[s]" :class="{ active: screen === s, done: stepIndex > i }" @click="go(s)">
        <span class="step">{{ stepIndex > i ? "✓" : i + 1 }}</span><span class="lbl">{{ t.nav[s] }}</span>
      </button>
      <div class="group">{{ t.nav.other }}</div>
      <button class="nav" data-s="sessions" :title="t.nav.sessions" :class="{ active: screen === 'sessions' }" @click="go('sessions')"><span class="ico">🕘</span><span class="lbl">{{ t.nav.sessions }}</span></button>
      <button class="nav" data-s="settings" :title="t.nav.settings" :class="{ active: screen === 'settings' }" @click="go('settings')"><span class="ico">⚙</span><span class="lbl">{{ t.nav.settings }}</span></button>
      <button class="nav" data-s="perm" :title="t.nav.perm" :class="{ active: screen === 'perm' }" @click="go('perm')"><span class="ico">🔐</span><span class="lbl">{{ t.nav.perm }}</span></button>
      <div class="bottom">
        <button class="nav" data-s="about" :title="t.nav.about" :class="{ active: screen === 'about' }" @click="go('about')"><span class="ico">ⓘ</span><span class="lbl">{{ t.nav.about }}</span></button>
        <div class="small muted ver" style="padding: 8px 10px">v{{ version }} · {{ t.app.org }}</div>
      </div>
    </nav>

    <main class="main">
      <HomeScreen v-if="screen === 'home'" @refresh="refreshDisks" />
      <ScanScreen v-else-if="screen === 'scan'" />
      <ResultsScreen v-else-if="screen === 'results'" />
      <RecoverScreen v-else-if="screen === 'recover'" />
      <SessionsScreen v-else-if="screen === 'sessions'" />
      <SettingsScreen v-else-if="screen === 'settings'" />
      <PermScreen v-else-if="screen === 'perm'" @refresh="refreshDisks" />
      <AboutScreen v-else />
    </main>

    <div v-if="toast" class="toast" :class="toast.kind">{{ toast.text }}</div>
  </div>
</template>
