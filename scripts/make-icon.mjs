// Rasterize public/icon.svg → src-tauri/icons/source.png (1024px), then
// `tauri icon` produces all platform sizes (ico / icns / png set).
import { Resvg } from "@resvg/resvg-js";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";

const svg = readFileSync(new URL("../public/icon.svg", import.meta.url), "utf8");
const png = new Resvg(svg, { fitTo: { mode: "width", value: 1024 } }).render().asPng();
mkdirSync(new URL("../src-tauri/icons/", import.meta.url), { recursive: true });
writeFileSync(new URL("../src-tauri/icons/source.png", import.meta.url), png);
console.log("icons/source.png written:", png.length, "bytes");
