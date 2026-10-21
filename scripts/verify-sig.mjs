// Verify a Tauri updater (minisign) signature against the public key embedded in tauri.conf.json.
// Usage: node scripts/verify-sig.mjs <file> <file.sig>
import { readFileSync } from "node:fs";
import { createHash, createPublicKey, verify } from "node:crypto";

const [file, sigFile] = process.argv.slice(2);
const b64 = (s) => Buffer.from(s, "base64");
const conf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const pubLine = b64(conf.plugins.updater.pubkey).toString("utf8").split("\n")[1];
const pk = b64(pubLine); // [2 alg][8 keyid][32 ed25519 key]
const sigLines = b64(readFileSync(sigFile, "utf8").trim()).toString("utf8").split("\n");
const sig = b64(sigLines[1]); // [2 alg][8 keyid][64 signature]
const alg = sig.subarray(0, 2).toString();
const keyMatch = sig.subarray(2, 10).equals(pk.subarray(2, 10));
const data = readFileSync(file);
const msg = alg === "ED" ? createHash("blake2b512").update(data).digest() : data; // "ED" = prehashed
const key = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), pk.subarray(10, 42)]), format: "der", type: "spki" });
const ok = keyMatch && verify(null, msg, key, sig.subarray(10, 74));
console.log(`${file}\n  alg=${alg} keyid-match=${keyMatch} size=${data.length}\n  signature valid: ${ok}`);
console.log(`  trusted comment: ${sigLines[2]?.replace(/^trusted comment: /, "")}`);
process.exit(ok ? 0 : 1);
