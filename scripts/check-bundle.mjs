#!/usr/bin/env node
// First-screen bundle budget (CLAUDE.md §12 / CONTRIBUTING): the JS that index.html loads
// before anything is lazily imported must stay within 250 KiB after gzip.
// Run after `pnpm --dir app build`. Exits with 1 when the budget is exceeded.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const BUDGET_KIB = 250;
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(root, "app", "dist");
const indexPath = join(dist, "index.html");

if (!existsSync(indexPath)) {
  console.error("app/dist/index.html not found. Run `pnpm --dir app build` first.");
  process.exit(1);
}

const html = readFileSync(indexPath, "utf8");
const entry = [...html.matchAll(/(?:src|href)="\/?(assets\/[^"]+\.js)"/g)].map((m) => m[1]);
// Static imports only: `import"./x.js"` / `from"./x.js"`. Dynamic `import("./x.js")` is lazy.
const staticImport = /(?:import|from)\s*["']\.\/([^"']+\.js)["']/g;

const seen = new Set();
const queue = [...entry];
while (queue.length > 0) {
  const file = queue.pop();
  if (seen.has(file)) continue;
  const path = join(dist, file);
  if (!existsSync(path)) continue;
  seen.add(file);
  const code = readFileSync(path, "utf8");
  for (const match of code.matchAll(staticImport)) {
    queue.push(join(dirname(file), match[1]).replaceAll("\\", "/"));
  }
}

const sizes = [...seen]
  .map((file) => ({ file, gzip: gzipSync(readFileSync(join(dist, file)), { level: 9 }).length }))
  .sort((a, b) => b.gzip - a.gzip);
const total = sizes.reduce((sum, item) => sum + item.gzip, 0) / 1024;

for (const { file, gzip } of sizes.slice(0, 5)) {
  console.log(`${(gzip / 1024).toFixed(1).padStart(7)} KiB  ${file}`);
}
console.log(
  `First-screen JS: ${total.toFixed(1)} KiB gzip across ${sizes.length} files (budget ${BUDGET_KIB} KiB)`,
);

if (total > BUDGET_KIB) {
  console.error(`Over budget by ${(total - BUDGET_KIB).toFixed(1)} KiB.`);
  process.exit(1);
}
