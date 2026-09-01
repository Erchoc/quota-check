// 编程式 API：在本机调用 Rust 二进制，返回解析后的数据。
//
//   import { check, checkHuman, whoami } from "quota-check";
//   const usage = await check("codex");            // => 原始 JSON 对象
//   const text  = await checkHuman("codex");       // => 人类可读字符串
//   const me    = await whoami("codex");           // => 凭据对应的账号信息
//
// 注意：当前是「本机执行」模式，读取的是本机凭据文件（如 ~/.codex/auth.json），
// 所以这段代码必须跑在用户自己的机器上。未来云端托管模式上线后，
// 同一套 API 会改为走云端环境触发，签名保持不变。

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
  if (options.auth) args.push("--auth", options.auth);
  return args.concat(extra);
}

/** 查询额度，返回原始 JSON 对象。 */
export async function check(provider = "codex", options = {}) {
  const out = await runRaw(buildArgs(provider, options));
  return JSON.parse(out);
}

/** 查询额度，返回人类可读字符串。 */
export async function checkHuman(provider = "codex", options = {}) {
  return runRaw(buildArgs(provider, options, ["--human"]));
}

/** 只看凭据属于哪个账号，返回账号信息对象。 */
export async function whoami(provider = "codex", options = {}) {
  const out = await runRaw(buildArgs(provider, options, ["--whoami"]));
  return JSON.parse(out);
}
