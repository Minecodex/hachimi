#!/usr/bin/env node
// Cross-platform port of scripts/release/check-disk-space.ps1 (kept for the
// Windows gates). Verifies the volume holding the workspace has enough free
// space before a release build. Usage: node check-disk-space.mjs [minGiB]

import { statfsSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const minimumFreeGiB = Number(process.argv[2] ?? 40);
if (!Number.isFinite(minimumFreeGiB) || minimumFreeGiB <= 0) {
  throw new Error(`release_disk_space_argument_invalid:${process.argv[2]}`);
}

const stats = statfsSync(workspaceRoot);
const freeGiB = Math.round(((Number(stats.bavail) * stats.bsize) / 1024 ** 3) * 100) / 100;
if (freeGiB < minimumFreeGiB) {
  throw new Error(`release_disk_space_low: available=${freeGiB}GiB required=${minimumFreeGiB}GiB`);
}
console.log(`release_disk_space_ok: available=${freeGiB}GiB required=${minimumFreeGiB}GiB`);
