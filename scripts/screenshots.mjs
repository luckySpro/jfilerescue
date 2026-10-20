// Playwright screenshots of the production build in demo mode.
// Run: pnpm build && pnpm screenshots   (needs `npx playwright install chromium` once)
import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { mkdirSync } from "node:fs";

mkdirSync("docs/screenshots", { recursive: true });
const server = spawn(process.platform === "win32" ? "pnpm.cmd" : "pnpm", ["exec", "vite", "preview", "--port", "4173", "--strictPort"], { stdio: "ignore", shell: process.platform === "win32" });
await new Promise((r) => setTimeout(r, 2500));

const browser = await chromium.launch();
const nav = (p, s) => p.click(`button.nav[data-s="${s}"]`);
const shots = [
  ["home", async (p) => { await p.fill("input[type=text] >> nth=0", "D:\\项目\\2025-合同"); await p.waitForTimeout(300); }],
  ["scan", async (p) => { await p.fill("input[type=text] >> nth=0", "D:\\项目\\2025-合同"); await p.waitForTimeout(300); await p.click("text=开始扫描"); await p.waitForTimeout(9000); }],
  ["results", async (p) => { await nav(p, "sessions"); await p.click("table >> text=打开"); await p.waitForTimeout(800); await p.click("tbody tr >> nth=0"); await p.waitForTimeout(500); }],
  ["recover", async (p) => { await nav(p, "sessions"); await p.click("table >> text=打开"); await p.waitForTimeout(800); await p.click("text=恢复所选文件"); await p.fill("input[type=text]", "E:\\恢复"); await p.waitForTimeout(600); await p.click("text=开始恢复"); await p.waitForTimeout(4000); }],
  ["sessions", async (p) => { await nav(p, "sessions"); await p.waitForTimeout(400); }],
  ["signatures", async (p) => { await nav(p, "settings"); await p.click("button.tab >> text=签名库"); await p.waitForTimeout(400); }],
  ["permissions", async (p) => { await nav(p, "perm"); await p.waitForTimeout(600); }],
  ["about", async (p) => { await nav(p, "about"); await p.waitForTimeout(400); }],
];
const overflow = [];
for (const [theme, width] of [["light", 1280], ["dark", 1280], ["light", 700]]) {
  const ctx = await browser.newContext({ viewport: { width, height: 820 }, colorScheme: theme, locale: "zh-CN" });
  const page = await ctx.newPage();
  await page.addInitScript((t) => { localStorage.setItem("jfr.theme", t); localStorage.setItem("jfr.locale", "zh-CN"); }, theme);
  for (const [name, act] of shots) {
    if (width === 700 && !["home", "results"].includes(name)) continue;
    await page.goto("http://localhost:4173/");
    await page.waitForTimeout(500);
    await act(page);
    const ov = await page.evaluate(() => [...document.querySelectorAll("*")].filter((e) => e.scrollWidth > e.clientWidth + 2 && getComputedStyle(e).overflowX === "visible" && e.clientWidth > 0).slice(0, 5).map((e) => `${e.tagName}.${[...e.classList].join(".")} ${e.scrollWidth}>${e.clientWidth}`));
    if (ov.length) overflow.push(`${name}-${theme}-${width}: ${ov.join(", ")}`);
    await page.screenshot({ path: `docs/screenshots/${name}-${theme}${width === 700 ? "-narrow" : ""}.png` });
    console.log("shot", name, theme, width);
  }
  await ctx.close();
}
await browser.close();
server.kill();
if (overflow.length) { console.log("OVERFLOW:\n" + overflow.join("\n")); }
process.exit(0);
