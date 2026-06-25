// Supply-chain scanner (SPEC §11 / JS dependency policy).
// Fails if any package in the resolved Bun lockfile matches the May-2026
// compromised set, or belongs to an org compromised in that wave.
// Run from web/: `bun ../scripts/scan-deps.mjs`
import { readFileSync } from "node:fs";

let lockText;
try {
  lockText = readFileSync("bun.lock", "utf8");
} catch {
  console.error("scan-deps: bun.lock not found (run `bun install --lockfile-only` first).");
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

const findings = [];
let count = 0;
const packageLine = /^\s{4}"([^"]+)": \["([^"]+)"/gm;
for (const match of lockText.matchAll(packageLine)) {
  const [, name, spec] = match;
  const prefix = `${name}@`;
  const version = spec.startsWith(prefix) ? spec.slice(prefix.length) : spec.slice(spec.lastIndexOf("@") + 1);
  const id = `${name}@${version}`;
  count += 1;
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

console.log(`Supply-chain scan OK — ${count} resolved packages, none in the May-2026 compromised set.`);
