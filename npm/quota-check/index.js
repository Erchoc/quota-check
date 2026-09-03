// Programmatic API: invokes the local Rust binary and returns parsed data.
//
//   import { check, checkAll, checkHuman, whoami } from "quota-check";
//   const usage = await check("codex");            // raw JSON object
//   const all   = await checkAll();                // every provider at once
//   const text  = await checkHuman("claude");      // human-readable string
//   const me    = await whoami("codex");           // account behind the credential
//
// Providers: "codex" | "claude" | "kimi"
//
// Note: this is currently LOCAL execution — it reads credential files on the
// machine it runs on (e.g. ~/.codex/auth.json, macOS Keychain). The same API
// will switch to cloud-hosted execution when that mode ships; signatures
// stay unchanged.

import { spawn } from "node:child_process";
import { binaryPath } from "./lib/binary.js";

export { binaryPath } from "./lib/binary.js";

function runRaw(args) {
  return new Promise((resolve, reject) => {
    const child = spawn(binaryPath(), args, { stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    let err = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (err += d));
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve(out);
      else reject(new Error(err.trim() || `quota-check exited with code ${code}`));
    });
  });
}

function buildArgs(provider, options = {}, extra = []) {
  const args = [provider];
  // provider-specific credential flags
  if (options.auth) args.push("--auth", options.auth); // codex
  if (options.token) args.push("--token", options.token); // claude
  if (options.noRefresh) args.push("--no-refresh"); // claude
  if (options.key) args.push("--key", options.key); // kimi
  if (options.base) args.push("--base", options.base); // kimi
  return args.concat(extra);
}

/** Query quota, returns the raw JSON object. */
export async function check(provider = "codex", options = {}) {
  const out = await runRaw(buildArgs(provider, options, ["--json"]));
  return JSON.parse(out);
}

/**
 * Query every provider in one call. Resolves to
 * `{ codex: {ok, data|error}, claude: {...}, kimi: {...} }`, and only rejects
 * when no provider produced a quota at all.
 */
export async function checkAll() {
  // `all` uses each provider's own credential discovery; it takes no
  // per-provider flags.
  const out = await runRaw(["all", "--json"]);
  return JSON.parse(out);
}

/** Query quota, returns the human-readable rendering. */
export async function checkHuman(provider = "codex", options = {}) {
  return runRaw(buildArgs(provider, options, ["--human"]));
}

/** Show which account the credential belongs to (codex only). */
export async function whoami(provider = "codex", options = {}) {
  const out = await runRaw(buildArgs(provider, options, ["--whoami", "--json"]));
  return JSON.parse(out);
}
