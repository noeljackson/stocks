// Supply-chain scanner (SPEC §11 / JS dependency policy).
// Fails if any package in the resolved lockfile matches the May-2026
// compromised set, or belongs to an org compromised in that wave.
// Run from web/: `node ../scripts/scan-deps.mjs`
import { readFileSync } from "node:fs";

let lock;
try {
  lock = JSON.parse(readFileSync("package-lock.json", "utf8"));
} catch {
  console.error("scan-deps: package-lock.json not found (run `npm install` first).");
  process.exit(2);
}

// Known-bad exact versions (update as advisories land).
const BAD_EXACT = new Set([
  "node-ipc@9.1.6",
  "node-ipc@9.2.3",
  "node-ipc@12.0.1",
  "@bitwarden/cli@2026.4.0",
]);
// Orgs compromised in the May-2026 wave. We don't depend on any of these;
// their presence (even transitive) is a red flag → hard fail.
const BAD_PREFIX = ["@antv/", "@cap-js/", "@tanstack/"];

const pkgs = lock.packages || {};
const findings = [];
for (const [path, info] of Object.entries(pkgs)) {
  const marker = "node_modules/";
  const idx = path.lastIndexOf(marker);
  if (idx < 0) continue;
  const name = path.slice(idx + marker.length);
  const id = `${name}@${info.version ?? ""}`;
  if (BAD_EXACT.has(id)) findings.push(`COMPROMISED: ${id}`);
  for (const p of BAD_PREFIX) {
    if (name.startsWith(p)) findings.push(`UNEXPECTED (compromised org): ${id}`);
  }
}

if (findings.length) {
  console.error("Supply-chain scan FAILED:");
  for (const f of findings) console.error("  - " + f);
  process.exit(1);
}

const count = Object.keys(pkgs).filter((p) => p.includes("node_modules/")).length;
console.log(`Supply-chain scan OK — ${count} resolved packages, none in the May-2026 compromised set.`);
