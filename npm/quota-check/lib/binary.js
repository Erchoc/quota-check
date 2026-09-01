// Resolve the quota-check binary location:
//   1. QUOTA_CHECK_BINARY env var (debugging / future cloud-hosted mode)
//   2. vendor/ inside this package (local development)
//   3. the platform package shipped via optionalDependencies
//      (quota-check-<platform>-<arch>, the standard esbuild/biome pattern)
//   4. PATH (cargo / brew installs)

import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const HERE = path.dirname(fileURLToPath(import.meta.url));

const IS_WIN = process.platform === "win32";
export const BIN_NAME = IS_WIN ? "quota-check.exe" : "quota-check";

const PLATFORM_PKGS = {
  "darwin-arm64": "quota-check-darwin-arm64",
  "darwin-x64": "quota-check-darwin-x64",
  "linux-x64": "quota-check-linux-x64",
  "linux-arm64": "quota-check-linux-arm64",
  "win32-x64": "quota-check-windows-x64",
};

export function vendorPath() {
  return path.join(HERE, "..", "vendor", BIN_NAME);
}

export function binaryPath() {
  if (process.env.QUOTA_CHECK_BINARY) return process.env.QUOTA_CHECK_BINARY;

  const local = vendorPath();
  if (fs.existsSync(local)) return local;

  const pkg = PLATFORM_PKGS[`${process.platform}-${process.arch}`];
  if (pkg) {
    try {
      const pkgJson = require.resolve(`${pkg}/package.json`);
      const bin = path.join(path.dirname(pkgJson), BIN_NAME);
      if (fs.existsSync(bin)) return bin;
    } catch {
      // platform package not installed (e.g. --no-optional) — fall through
    }
  }

  return BIN_NAME; // PATH lookup (cargo / brew install)
}
