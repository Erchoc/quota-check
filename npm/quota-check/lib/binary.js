// 解析 quota-check 二进制位置：
//   1. QUOTA_CHECK_BINARY 环境变量（调试 / 云端托管时指定）
//   2. 本包 vendor/ 目录（postinstall 下载的平台二进制）
//   3. PATH 里的 quota-check（用户自己用 cargo / brew 装的）

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));

const IS_WIN = process.platform === "win32";
export const BIN_NAME = IS_WIN ? "quota-check.exe" : "quota-check";

export function vendorPath() {
  return path.join(HERE, "..", "vendor", BIN_NAME);
}

export function binaryPath() {
  if (process.env.QUOTA_CHECK_BINARY) return process.env.QUOTA_CHECK_BINARY;
  const local = vendorPath();
  if (fs.existsSync(local)) return local;
  return BIN_NAME; // 交给 PATH
}
