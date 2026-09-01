"use strict";

// 解析 quota-check 二进制位置：
//   1. QUOTA_CHECK_BINARY 环境变量（调试 / 云端托管时指定）
//   2. 本包 vendor/ 目录（postinstall 下载的平台二进制）
//   3. PATH 里的 quota-check（用户自己用 cargo / brew 装的）

const fs = require("node:fs");
const path = require("node:path");

const IS_WIN = process.platform === "win32";
const BIN_NAME = IS_WIN ? "quota-check.exe" : "quota-check";

function vendorPath() {
  return path.join(__dirname, "..", "vendor", BIN_NAME);
}

function binaryPath() {
  if (process.env.QUOTA_CHECK_BINARY) return process.env.QUOTA_CHECK_BINARY;
  const local = vendorPath();
  if (fs.existsSync(local)) return local;
  return BIN_NAME; // 交给 PATH
}

module.exports = { binaryPath, vendorPath, BIN_NAME };
