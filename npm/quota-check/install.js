// postinstall：按平台从 GitHub Releases 下载预编译的 Rust 二进制到 vendor/。
// 下载失败不阻断安装（exit 0），给出 cargo 安装的兜底提示。

import fs from "node:fs";
import https from "node:https";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { vendorPath } from "./lib/binary.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const pkg = JSON.parse(fs.readFileSync(path.join(HERE, "package.json"), "utf8"));

const REPO = "erchoc/quota-check";

const TARGETS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-msvc",
};

function download(url, dest, redirects = 5) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { "User-Agent": "quota-check-npm" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          if (redirects === 0) return reject(new Error("重定向次数过多"));
          return resolve(download(res.headers.location, dest, redirects - 1));
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${res.statusCode}`));
        }
        const file = fs.createWriteStream(dest);
        res.pipe(file);
        file.on("finish", () => file.close(resolve));
        file.on("error", reject);
      })
      .on("error", reject);
  });
}

async function main() {
  if (process.env.QUOTA_CHECK_SKIP_DOWNLOAD) {
    console.log("quota-check: QUOTA_CHECK_SKIP_DOWNLOAD 已设置，跳过二进制下载");
    return;
  }

  const key = `${process.platform}-${process.arch}`;
  const target = TARGETS[key];
  if (!target) {
    console.warn(`quota-check: 暂无 ${key} 预编译二进制，可用 cargo install quota-check 安装`);
    return;
  }

  const url = `https://github.com/${REPO}/releases/download/v${pkg.version}/quota-check-${target}.tar.gz`;
  const vendorDir = path.dirname(vendorPath());
  fs.mkdirSync(vendorDir, { recursive: true });

  const tmp = path.join(vendorDir, "download.tar.gz");
  try {
    await download(url, tmp);
    execFileSync("tar", ["-xzf", tmp, "-C", vendorDir]);
    fs.chmodSync(vendorPath(), 0o755);
    console.log(`quota-check: 已安装 ${target} 二进制`);
  } catch (e) {
    console.warn(
      `quota-check: 二进制下载失败（${e.message}）。\n` +
        `  可稍后 npm rebuild quota-check 重试，或用 cargo install quota-check 安装`
    );
  } finally {
    fs.rmSync(tmp, { force: true });
  }
}

main().catch(() => process.exit(0));
