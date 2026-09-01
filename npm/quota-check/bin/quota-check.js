#!/usr/bin/env node

// CLI 入口：原样转发参数给 Rust 二进制。

import { spawnSync } from "node:child_process";
import { binaryPath } from "../lib/binary.js";

const r = spawnSync(binaryPath(), process.argv.slice(2), { stdio: "inherit" });

if (r.error) {
  if (r.error.code === "ENOENT") {
    console.error(
      "\n  ✗ 找不到 quota-check 二进制。\n" +
        "    尝试重新安装：npm rebuild quota-check\n" +
        "    或用 cargo 安装：cargo install quota-check\n"
    );
  } else {
    console.error(`\n  ✗ 启动失败：${r.error.message}\n`);
  }
  process.exit(1);
}
process.exit(r.status ?? 1);
