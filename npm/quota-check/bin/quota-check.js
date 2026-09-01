#!/usr/bin/env node

// CLI entry: forward all arguments to the Rust binary.

import { spawnSync } from "node:child_process";
import { binaryPath } from "../lib/binary.js";

const r = spawnSync(binaryPath(), process.argv.slice(2), { stdio: "inherit" });

if (r.error) {
  if (r.error.code === "ENOENT") {
    console.error(
      "\n  ✗ quota-check binary not found.\n" +
        "    Reinstall: npm rebuild quota-check\n" +
        "    Or via cargo: cargo install quota-check\n"
    );
  } else {
    console.error(`\n  ✗ failed to launch: ${r.error.message}\n`);
  }
  process.exit(1);
}
process.exit(r.status ?? 1);
