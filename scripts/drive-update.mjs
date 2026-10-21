// Drive the *installed* JFileRescue (real WebView2) over CDP to exercise the
// in-app updater end to end: check → (optionally) download & install.
//
// Launch the app first with the debugging port exposed:
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
//   & "D:\Program Files\JFileRescue\JFileRescue.exe"
// Then: node scripts/drive-update.mjs [--install]
import { chromium } from "playwright";

const install = process.argv.includes("--install");
const browser = await chromium.connectOverCDP("http://127.0.0.1:9222");
const ctx = browser.contexts()[0];
const page = ctx.pages().find((p) => /tauri\.localhost|localhost|index\.html/.test(p.url())) ?? ctx.pages()[0];
console.log("page:", page.url());
await page.waitForSelector("button.nav[data-s='settings']", { timeout: 15000 });
const version = await page.evaluate(() => document.querySelector(".sidebar .ver")?.textContent?.trim());
console.log("sidebar version:", version);
await page.click("button.nav[data-s='settings']");
await page.click("button.tab >> nth=3"); // 更新
await page.waitForTimeout(300);
await page.click("text=检查更新");
// Wait for the state text to settle (checking… → available/none/error).
let state = "";
for (let i = 0; i < 60; i++) {
  await page.waitForTimeout(500);
  state = await page.evaluate(() => document.querySelector(".setrow .d")?.textContent?.trim() ?? "");
  if (state && !state.includes("正在检查")) break;
}
console.log("update state:", state);
if (!/有新版本/.test(state)) { console.log("no update offered — stopping"); await browser.close(); process.exit(2); }
if (!install) { console.log("(dry run; pass --install to download and install)"); await browser.close(); process.exit(0); }
await page.click("text=下载并安装");
console.log("clicked 下载并安装; waiting for the app to exit/relaunch…");
// The updater downloads, verifies the signature, runs the NSIS installer and relaunches; the CDP connection drops.
const started = Date.now();
await new Promise((r) => setTimeout(r, 1000));
try { await page.waitForTimeout(120000); } catch { /* connection closed = app exited */ }
console.log("elapsed", Math.round((Date.now() - started) / 1000), "s");
try { await browser.close(); } catch { /* already gone */ }
