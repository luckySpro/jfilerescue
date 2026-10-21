// Dump raw directory entries / allocation state from the test images so parser bugs
// can be diagnosed against real bytes. Usage: node scripts/dump-fs.mjs fat32|exfat
import { openSync, readSync, closeSync } from "node:fs";
const kind = process.argv[2] ?? "fat32";
const fd = openSync(`test-images/${kind}.img`, "r");
const rdAbs = (off, len) => { const b = Buffer.alloc(len); readSync(fd, b, 0, len, Number(off)); return b; };
// diskpart images carry an MBR; the volume starts at partition 1.
const lba0 = rdAbs(0, 512);
let base = 0;
if (lba0[510] === 0x55 && lba0[511] === 0xaa && lba0[446 + 4] !== 0 && lba0.subarray(3, 11).toString("latin1") !== "NTFS    ") base = lba0.readUInt32LE(446 + 8) * 512;
console.log({ volumeBase: base });
const rd = (off, len) => rdAbs(base + off, len);
const boot = rd(0, 512);
const hex = (b) => [...b].map((x) => x.toString(16).padStart(2, "0")).join(" ");

function walkFatDir(bytes, label) {
  console.log(`--- ${label} (${bytes.length} bytes) ---`);
  for (let i = 0; i + 32 <= bytes.length; i += 32) {
    const e = bytes.subarray(i, i + 32);
    if (e[0] === 0) break;
    const attr = e[11];
    const first = e[0];
    if ((attr & 0x0f) === 0x0f) {
      const u = (s) => s.toString("utf16le").replace(/\0.*$/s, "").replace(/￿.*$/s, "");
      console.log(`  LFN first=0x${first.toString(16).padStart(2, "0")} seq=${first & 0x1f}${first & 0x40 ? " LAST" : ""} chk=${e[13]} text="${u(e.subarray(1, 11)) + u(e.subarray(14, 26)) + u(e.subarray(28, 32))}"`);
    } else {
      const short = e.subarray(0, 11).toString("latin1");
      const clus = (e.readUInt16LE(20) << 16) | e.readUInt16LE(26);
      console.log(`  8.3  first=0x${first.toString(16).padStart(2, "0")} "${short}" attr=0x${attr.toString(16)} cluster=${clus} size=${e.readUInt32LE(28)}`);
    }
  }
}

if (kind === "fat32") {
  const bps = boot.readUInt16LE(11), spc = boot[13], reserved = boot.readUInt16LE(14), nfats = boot[16], fatsz = boot.readUInt32LE(36), rootClus = boot.readUInt32LE(44);
  const cluster = bps * spc;
  const dataOff = (reserved + nfats * fatsz) * bps;
  const clusOff = (c) => dataOff + (c - 2) * cluster;
  console.log({ bps, spc, cluster, dataOff, rootClus });
  const fat = rd(reserved * bps, fatsz * bps);
  const chain = (c) => { const out = []; while (c >= 2 && c < 0x0ffffff8 && out.length < 1000) { out.push(c); c = fat.readUInt32LE(c * 4) & 0x0fffffff; } return out; };
  const readDir = (c) => Buffer.concat(chain(c).map((x) => rd(clusOff(x), cluster)));
  const root = readDir(rootClus);
  walkFatDir(root, "root");
  // Descend into every directory (live or deleted-but-cluster-intact).
  const seen = new Set();
  const stack = [[root, ""]];
  while (stack.length) {
    const [buf, path] = stack.pop();
    for (let i = 0; i + 32 <= buf.length; i += 32) {
      const e = buf.subarray(i, i + 32);
      if (e[0] === 0) break;
      const attr = e[11];
      if ((attr & 0x0f) === 0x0f || !(attr & 0x10)) continue;
      const clus = (e.readUInt16LE(20) << 16) | e.readUInt16LE(26);
      const short = e.subarray(0, 11).toString("latin1").trim();
      if (clus < 2 || seen.has(clus) || short.startsWith(".")) continue;
      seen.add(clus);
      const sub = readDir(clus);
      const p = `${path}/${short}`;
      walkFatDir(sub, p);
      stack.push([sub, p]);
    }
  }
} else {
  const fatOff = boot.readUInt32LE(80), fatLen = boot.readUInt32LE(84), heapOff = boot.readUInt32LE(88), clusCount = boot.readUInt32LE(92), rootClus = boot.readUInt32LE(96);
  const bps = 1 << boot[108], spc = 1 << boot[109], cluster = bps * spc;
  const clusOff = (c) => heapOff * bps + (c - 2) * cluster;
  console.log({ bps, spc, cluster, heapOffBytes: heapOff * bps, clusCount, rootClus });
  const fat = rd(fatOff * bps, fatLen * bps);
  const chainOf = (c, nfc, size) => { if (nfc) { const n = Math.ceil(size / cluster); return Array.from({ length: n }, (_, i) => c + i); } const out = []; while (c >= 2 && c < 0xfffffff7 && out.length < 1000) { out.push(c); c = fat.readUInt32LE(c * 4); } return out; };
  let bitmap = null;
  const walk = (c, nfc, size, label) => {
    const buf = Buffer.concat(chainOf(c, nfc, size).map((x) => rd(clusOff(x), cluster)));
    console.log(`--- ${label} ---`);
    const dirs = [];
    for (let i = 0; i + 32 <= buf.length; ) {
      const e = buf.subarray(i, i + 32);
      const t = e[0];
      if (t === 0) break;
      const inUse = !!(t & 0x80), base = t & 0x7f;
      if (base === 0x01 && inUse) { const bc = e.readUInt32LE(20), bl = Number(e.readBigUInt64LE(24)); bitmap = rd(clusOff(bc), bl); console.log(`  BITMAP cluster=${bc} len=${bl}`); }
      if (base === 0x05) {
        const sec = e[1]; const attr = e.readUInt16LE(4);
        let name = "", fc = 0, sz = 0, flags = 0;
        for (let k = 1; k <= sec; k++) {
          const s = buf.subarray(i + k * 32, i + k * 32 + 32);
          if ((s[0] & 0x7f) === 0x40) { flags = s[1]; fc = s.readUInt32LE(20); sz = Number(s.readBigUInt64LE(24)); }
          if ((s[0] & 0x7f) === 0x41) name += s.subarray(2, 32).toString("utf16le").replace(/\0+$/, "");
        }
        const chain = chainOf(fc, !!(flags & 2), sz);
        const alloc = bitmap ? chain.slice(0, 8).map((x) => (bitmap[(x - 2) >> 3] >> ((x - 2) & 7)) & 1).join("") : "?";
        console.log(`  ${inUse ? "LIVE" : "DEL "} ${attr & 0x10 ? "DIR " : "FILE"} "${name}" first=${fc} size=${sz} NoFatChain=${!!(flags & 2)} chain=[${chain.slice(0, 6).join(",")}${chain.length > 6 ? ",…" : ""}] bitmap(first8)=${alloc} fatNext(first)=${fc >= 2 ? fat.readUInt32LE(fc * 4).toString(16) : "-"}`);
        if (attr & 0x10 && fc >= 2) dirs.push([fc, !!(flags & 2), sz, `${label}/${name}`]);
        i += 32 * (1 + sec);
      } else i += 32;
    }
    for (const d of dirs) walk(...d);
  };
  walk(rootClus, false, 0, "root");
}
closeSync(fd);
