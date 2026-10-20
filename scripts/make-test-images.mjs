// Builds small NTFS / FAT32 / exFAT test images on the host OS, deletes files
// in them, and leaves .img files under test-images/ for the integration tests.
//
// Windows: uses diskpart + a VHD (needs admin). macOS: hdiutil + newfs.
// Linux: mkfs.* + loop mount (needs root). Images are gitignored.
//
// Usage: node scripts/make-test-images.mjs [--size-mb 64]
import { execSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
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
  writeFileSync(join(root, "其他", "keep.txt"), "this stays");
  writeFileSync(join(root, "其他", "gone.txt"), "deleted outside target dir " + "y".repeat(5000));
}
function deleteSome(root) {
  const d = join(root, "项目", "2025-合同");
  for (const f of ["合同-华东区-2025Q2.docx", "附件/报价单-v3.xlsx", "现场照片/IMG_0001.jpg", "签字页.pdf", "small.txt"]) rmSync(join(d, f));
  rmSync(join(root, "其他", "gone.txt"));
}

if (platform() === "win32") {
  for (const fs of ["NTFS", "FAT32", "exFAT"]) {
    const vhd = join(out, `${fs.toLowerCase()}.vhd`);
    const img = join(out, `${fs.toLowerCase()}.img`);
    if (existsSync(img)) { console.log("exists", img); continue; }
    rmSync(vhd, { force: true });
    const script = join(tmpdir(), `jfr-${fs}.txt`);
    writeFileSync(script, [`create vdisk file="${vhd}" maximum=${sizeMb} type=fixed`, `select vdisk file="${vhd}"`, "attach vdisk", "create partition primary", `format fs=${fs} quick label=JFRTEST`, "assign letter=Q", "exit"].join("\r\n"));
    sh(`diskpart /s "${script}"`);
    populate("Q:\\");
    deleteSome("Q:\\");
    sh("cmd /c \"echo done\"");
    writeFileSync(script, [`select vdisk file="${vhd}"`, "detach vdisk", "exit"].join("\r\n"));
    sh(`diskpart /s "${script}"`);
    // A fixed VHD is raw sectors + 512-byte footer; the parser ignores the trailing footer.
    sh(`cmd /c move /y "${vhd}" "${img}"`);
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
