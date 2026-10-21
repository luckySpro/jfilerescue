// Builds small NTFS / FAT32 / exFAT test images on the host OS, deletes files
// in them, and leaves .img files under test-images/ for the integration tests.
//
// Windows: uses diskpart + a VHD (needs admin). macOS: hdiutil + newfs.
// Linux: mkfs.* + loop mount (needs root). Images are gitignored.
//
// Usage: node scripts/make-test-images.mjs [--size-mb 64]
import { execSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync, rmSync, renameSync, statSync, readdirSync, openSync, fsyncSync, closeSync } from "node:fs";
import { join, resolve } from "node:path";
import { platform, tmpdir } from "node:os";

const out = resolve("test-images");
mkdirSync(out, { recursive: true });
const sizeMb = Number(process.argv[process.argv.indexOf("--size-mb") + 1] || 64);

function sh(cmd, opts = {}) {
  console.log("$", cmd);
  return execSync(cmd, { stdio: ["ignore", "pipe", "inherit"], ...opts }).toString();
}

// Deterministic payload so tests can verify SHA256.
function populate(root) {
  const d = join(root, "项目", "2025-合同");
  mkdirSync(join(d, "附件"), { recursive: true });
  mkdirSync(join(d, "现场照片"), { recursive: true });
  mkdirSync(join(root, "其他"), { recursive: true });
  // Structurally valid JPEG skeleton (SOI, APP0, SOS, entropy, EOI) so carving resolves its length.
  const jpg = Buffer.concat([Buffer.from([0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]), Buffer.from("JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00", "latin1"), Buffer.from([0xff, 0xda, 0x00, 0x02]), Buffer.alloc(200_000, 0x37), Buffer.from([0xff, 0xd9])]);
  const pdf = Buffer.concat([Buffer.from("%PDF-1.4\n1 0 obj\n<<>>\nendobj\n"), Buffer.alloc(120_000, 0x41), Buffer.from("\n%%EOF\n")]);
  writeFileSync(join(d, "合同-华东区-2025Q2.docx"), Buffer.concat([Buffer.from("PK\x03\x04\x14\x00\x06\x00"), Buffer.alloc(50_000, 0x55)]));
  writeFileSync(join(d, "附件", "报价单-v3.xlsx"), Buffer.concat([Buffer.from("PK\x03\x04\x14\x00\x06\x00"), Buffer.alloc(30_000, 0x66)]));
  writeFileSync(join(d, "现场照片", "IMG_0001.jpg"), jpg);
  writeFileSync(join(d, "签字页.pdf"), pdf);
  writeFileSync(join(d, "small.txt"), "resident-data-" + "x".repeat(300)); // NTFS resident
  writeFileSync(join(d, "recycled.pdf"), pdf); // goes through the Recycle Bin (see recycleThenEmpty)
  writeFileSync(join(root, "其他", "keep.txt"), "this stays");
  writeFileSync(join(root, "其他", "gone.txt"), "deleted outside target dir " + "y".repeat(5000));
}
function deleteSome(root) {
  flushTree(root);
  const d = join(root, "项目", "2025-合同");
  for (const f of ["合同-华东区-2025Q2.docx", "附件/报价单-v3.xlsx", "现场照片/IMG_0001.jpg", "签字页.pdf", "small.txt"]) rmSync(join(d, f));
  rmSync(join(root, "其他", "gone.txt"));
}
// Files written and deleted within milliseconds never reach the disk — the cache
// manager drops the dirty pages on delete and the clusters read back as zeros.
// Force every file's data to the volume before deleting anything.
function flushTree(root) {
  for (const e of readdirSync(root, { withFileTypes: true })) {
    const p = join(root, e.name);
    if (e.isDirectory()) { if (!e.name.startsWith("System Volume")) flushTree(p); continue; }
    try { const fd = openSync(p, "r+"); fsyncSync(fd); closeSync(fd); } catch { /* read-only / locked */ }
  }
}
// Simulate a Recycle-Bin deletion on NTFS: Windows renames the file to
// $Recycle.Bin\<SID>\$R<6>.ext and writes a sibling $I<6>.ext (v2 layout) with the
// original path + deletion FILETIME; emptying the bin deletes both. We do the same
// so the scanner's $I→$R pairing gets exercised on a real volume.
function recycleThenEmpty(root, relPath) {
  const sid = "S-1-5-21-1111111111-2222222222-3333333333-1001";
  const bin = join(root, "$Recycle.Bin", sid);
  mkdirSync(bin, { recursive: true });
  const src = join(root, relPath);
  const ext = relPath.includes(".") ? relPath.slice(relPath.lastIndexOf(".")) : "";
  const key = "JFR" + String(Math.floor(Math.random() * 900 + 100));
  const original = (process.platform === "win32" ? "Q:\\" : "/Volumes/JFRTEST/") + relPath.replace(/\//g, process.platform === "win32" ? "\\" : "/");
  const size = statSync(src).size;
  { const fd = openSync(src, "r+"); fsyncSync(fd); closeSync(fd); }
  const filetime = (BigInt(Date.now()) + 11644473600000n) * 10000n;
  const p = Buffer.from(original + "\0", "utf16le");
  const info = Buffer.alloc(28 + p.length);
  info.writeBigUInt64LE(2n, 0); info.writeBigUInt64LE(BigInt(size), 8); info.writeBigUInt64LE(filetime, 16); info.writeUInt32LE(p.length / 2, 24); p.copy(info, 28);
  renameSync(src, join(bin, `$R${key}${ext}`));
  writeFileSync(join(bin, `$I${key}${ext}`), info);
  { const fd = openSync(join(bin, `$I${key}${ext}`), "r+"); fsyncSync(fd); closeSync(fd); }
  rmSync(join(bin, `$R${key}${ext}`));
  rmSync(join(bin, `$I${key}${ext}`));
}

if (platform() === "win32") {
  // Windows sends TRIM/UNMAP for freed clusters and the VHD driver zero-fills them
  // immediately, which makes "deleted but recoverable" impossible to produce (the
  // scanner then finds names/paths but every data cluster reads as zeros). Disable
  // delete notifications for the duration and restore the previous value afterwards.
  const prev = /NTFS DisableDeleteNotify = (\d)/.exec(sh("fsutil behavior query DisableDeleteNotify"))?.[1] ?? "0";
  sh("fsutil behavior set DisableDeleteNotify 1");
  try {
    for (const fs of ["NTFS", "FAT32", "exFAT"]) {
      const vhd = join(out, `${fs.toLowerCase()}.vhd`);
      const img = join(out, `${fs.toLowerCase()}.img`);
      if (existsSync(img)) { console.log("exists", img); continue; }
      rmSync(vhd, { force: true });
      const script = join(tmpdir(), `jfr-${fs}.txt`);
      writeFileSync(script, [`create vdisk file="${vhd}" maximum=${sizeMb} type=fixed`, `select vdisk file="${vhd}"`, "attach vdisk", "create partition primary", `format fs=${fs} quick label=JFRTEST`, "assign letter=Q", "exit"].join("\r\n"));
      sh(`diskpart /s "${script}"`);
      populate("Q:\\");
      if (fs === "NTFS") recycleThenEmpty("Q:\\", "项目/2025-合同/recycled.pdf");
      deleteSome("Q:\\");
      sh("cmd /c \"echo done\"");
      writeFileSync(script, [`select vdisk file="${vhd}"`, "detach vdisk", "exit"].join("\r\n"));
      sh(`diskpart /s "${script}"`);
      // A fixed VHD is raw sectors + 512-byte footer; the parser ignores the trailing footer.
      sh(`cmd /c move /y "${vhd}" "${img}"`);
    }
  } finally {
    sh(`fsutil behavior set DisableDeleteNotify ${prev}`);
    console.log("DisableDeleteNotify restored to", prev);
  }
} else if (platform() === "darwin") {
  for (const [fs, newfs] of [["apfs", "APFS"], ["exfat", "ExFAT"], ["fat32", "MS-DOS FAT32"]]) {
    const dmg = join(out, `${fs}.dmg`);
    const img = join(out, `${fs}.img`);
    if (existsSync(img)) { console.log("exists", img); continue; }
    rmSync(dmg, { force: true });
    sh(`hdiutil create -size ${sizeMb}m -fs "${newfs}" -volname JFRTEST -layout NONE "${dmg}"`);
    const mount = sh(`hdiutil attach "${dmg}" -mountpoint /tmp/jfrtest -nobrowse`).trim();
    populate("/tmp/jfrtest");
    deleteSome("/tmp/jfrtest");
    sh("sync");
    sh(`hdiutil detach /tmp/jfrtest`);
    sh(`mv "${dmg}" "${img}"`); // -layout NONE → raw volume image
    void mount;
  }
} else {
  console.log("Linux: run as root — mkfs.ntfs / mkfs.vfat / mkfs.exfat + mount loop; not automated here.");
}
console.log("done →", out);
