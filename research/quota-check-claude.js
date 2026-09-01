#!/usr/bin/env node
// quota-check-claude.mjs
// 查询 Claude Code 订阅的 5 小时窗口 / 7 天窗口额度用量。
//
//   node quota-check-claude.mjs                     # 原始 JSON
//   node quota-check-claude.mjs --human             # 人类可读
//   node quota-check-claude.mjs --discover --human  # 列出所有候选凭据及探测结果
//   node quota-check-claude.mjs --token sk-ant-oat-xxx --save
//   node quota-check-claude.mjs --forget            # 清缓存重新探测
//   node quota-check-claude.mjs --no-audit          # 跳过陈旧凭据检查（cron 里用）
//   node quota-check-claude.mjs --keychain-service "自定义条目名"
//
// 凭据优先级（跟 Claude Code 自己的存储策略一致）：
//   1. --token 参数
//   2. CLAUDE_CODE_OAUTH_TOKEN 环境变量
//   3. macOS Keychain            ← 主存储
//   4. .credentials.json 明文文件 ← Keychain 写入失败时的兜底，也是 Linux/Windows 的主存储
//
// 注意：这个接口只对 Pro / Max 订阅（OAuth 登录）有意义。
// 用 API Key 计费、或被 cc-switch 改道到第三方，都没有「订阅额度」可查，脚本会指出来。
//
// 需要 Node 18+。无第三方依赖。

import { readFileSync, writeFileSync, mkdirSync, existsSync, statSync } from "node:fs";
import { homedir, platform } from "node:os";
import { join, dirname } from "node:path";
import { execSync } from "node:child_process";

const ENDPOINT = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA = "oauth-2025-04-20";
const CACHE_PATH = join(homedir(), ".config", "quota-check", "claude.json");
const IS_MAC = platform() === "darwin";

// ---------- 参数 ----------
const argv = process.argv.slice(2);
const has = (f) => argv.includes(f);
const valueOf = (f) => {
  const i = argv.indexOf(f);
  return i >= 0 ? argv[i + 1] : undefined;
};

const HUMAN = has("--human");
const DISCOVER = has("--discover");
const SAVE = has("--save");
const FORGET = has("--forget");
const NO_AUDIT = has("--no-audit");
const CLAUDE_DIR = process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude");

function fail(msg) {
  process.stderr.write(`\n  ✗ ${msg}\n\n`);
  process.exit(1);
}

const readJson = (p) => {
  try {
    return JSON.parse(readFileSync(p, "utf8"));
  } catch {
    return null;
  }
};

const looksLikeToken = (t) => typeof t === "string" && t.trim().length >= 20;
const mask = (t) => (t.length <= 14 ? "***" : `${t.slice(0, 10)}…${t.slice(-4)}`);

const sh = (cmd) => {
  try {
    return execSync(cmd, {
      encoding: "utf8",
      timeout: 8000,
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null;
  }
};

// ---------- Keychain ----------
// 只查已知的 Claude Code 凭据条目名。
//
// 这里刻意不做 `security dump-keychain` 广撒网扫描，两个原因：
//   1. 会误抓同名前缀的无关条目。典型的就是 "Claude Safe Storage" ——
//      那是 Claude 桌面应用的 Electron safeStorage 对称密钥，不是 OAuth token，
//      删掉会导致桌面应用的本地加密数据解不开。
//   2. dump-keychain 本身也可能触发授权弹窗。
// 用了 CLAUDE_CONFIG_DIR 导致条目名变化的，用 --keychain-service 显式指定。
const KEYCHAIN_SERVICES = ["Claude Code-credentials"];

function keychainServiceNames() {
  const explicit = valueOf("--keychain-service");
  return explicit ? [explicit] : KEYCHAIN_SERVICES;
}

// Anthropic 的 OAuth token 有固定前缀，用它把无关的密钥挡在外面
const isAnthropicToken = (t) => typeof t === "string" && /^sk-ant-(oat|ort)/.test(t.trim());

function readKeychain() {
  if (!IS_MAC) return [];
  const out = [];
  for (const service of keychainServiceNames()) {
    const raw = sh(`security find-generic-password -s ${JSON.stringify(service)} -w`);
    if (!raw) continue;

    let token = null, expiresAt = null;
    try {
      const j = JSON.parse(raw);
      const o = j.claudeAiOauth ?? j.claudeAi ?? j.oauth ?? j;
      token = o.accessToken ?? o.access_token ?? o.access;
      expiresAt = o.expiresAt ?? o.expires_at ?? null;
    } catch {
      token = raw; // 有些条目直接存裸 token
    }

    // 形态不对就跳过，绝不当成 Claude 凭据
    if (!isAnthropicToken(token)) continue;
    out.push({ token: token.trim(), source: `Keychain: ${service}`, kind: "keychain", service, expiresAt });
  }
  return out;
}

// ---------- 候选收集 ----------
// 顺序即优先级：Keychain 在明文文件之前。
function collectCandidates() {
  const out = [];
  const seen = new Set();
  const push = (c) => {
    if (!looksLikeToken(c.token)) return;
    const t = c.token.trim();
    if (seen.has(t)) return;
    seen.add(t);
    out.push({ ...c, token: t });
  };

  push({ token: valueOf("--token"), source: "--token 参数", kind: "arg" });
  push({
    token: process.env.CLAUDE_CODE_OAUTH_TOKEN,
    source: "env CLAUDE_CODE_OAUTH_TOKEN",
    kind: "env",
  });

  // 3. Keychain（macOS 主存储）
  for (const c of readKeychain()) push(c);

  // 4. 明文文件（Linux/Windows 主存储；macOS 上是 Keychain 写不进去时的兜底）
  const credPath = join(CLAUDE_DIR, ".credentials.json");
  const cred = readJson(credPath);
  if (cred) {
    const o = cred.claudeAiOauth ?? cred.claudeAi ?? cred.oauth ?? cred;
    let mtime = null;
    try {
      mtime = statSync(credPath).mtime.toISOString();
    } catch {}
    push({
      token: o.accessToken ?? o.access_token ?? o.access,
      source: credPath,
      kind: "file",
      path: credPath,
      expiresAt: o.expiresAt ?? o.expires_at ?? null,
      mtime,
    });
  }

  return out;
}

// 检查是不是被 cc-switch 之类改道到第三方了
function detectRedirect() {
  const files = [
    join(CLAUDE_DIR, "settings.json"),
    join(CLAUDE_DIR, "settings.local.json"),
    join(process.cwd(), ".claude", "settings.json"),
    join(process.cwd(), ".claude", "settings.local.json"),
  ];
  const hits = [];
  if (process.env.ANTHROPIC_BASE_URL) {
    hits.push({ where: "env ANTHROPIC_BASE_URL", url: process.env.ANTHROPIC_BASE_URL });
  }
  for (const f of files) {
    const url = readJson(f)?.env?.ANTHROPIC_BASE_URL;
    if (url) hits.push({ where: f, url });
  }
  return hits.filter((h) => !/^https?:\/\/api\.anthropic\.com/i.test(h.url));
}

// ---------- 探测 ----------
// 把服务端的失败原因分类，revoked / expired / invalid 的处置方式完全不同。
function classify(status, body) {
  const s = String(body).toLowerCase();
  if (s.includes("revoked")) return "revoked";
  if (s.includes("expired")) return "expired";
  if (status === 401 || status === 403) return "invalid";
  if (status === 429) return "rate_limited";
  return "error";
}

async function probe(token) {
  try {
    const res = await fetch(ENDPOINT, {
      headers: {
        Authorization: `Bearer ${token}`,
        "anthropic-beta": OAUTH_BETA,
        Accept: "application/json",
        "User-Agent": "claude-code/2.1.0 (quota-check)", // 少了这个头会被拒
      },
      signal: AbortSignal.timeout(12000),
    });
    const body = await res.text();
    if (!res.ok) {
      return { ok: false, status: res.status, reason: classify(res.status, body), note: body.slice(0, 160) };
    }
    try {
      return { ok: true, status: res.status, data: JSON.parse(body) };
    } catch {
      return { ok: false, status: res.status, reason: "error", note: "返回不是 JSON" };
    }
  } catch (e) {
    return { ok: false, status: 0, reason: "network", note: e.message };
  }
}

// ---------- 陈旧凭据审计 ----------
// 命中之后，检查剩下的候选里有没有已经作废的残留。
// 先看本地 expiresAt（不花网络请求），只有看起来还没过期的才真去探测。
async function auditStale(candidates, hitToken) {
  const stale = [];
  const now = Date.now();

  for (const c of candidates) {
    if (c.token === hitToken) continue;
    if (c.kind !== "file" && c.kind !== "keychain") continue;

    const exp = Number(c.expiresAt);
    if (Number.isFinite(exp) && exp > 0) {
      const ms = exp < 1e11 ? exp * 1000 : exp; // 秒 or 毫秒
      if (ms < now) {
        stale.push({ ...c, reason: "expired", detail: new Date(ms).toLocaleString() });
        continue;
      }
    }

    const r = await probe(c.token);
    if (!r.ok && (r.reason === "revoked" || r.reason === "expired" || r.reason === "invalid")) {
      stale.push({ ...c, reason: r.reason, detail: r.note });
    }
  }
  return stale;
}

function renderStaleWarning(stale) {
  const REASON_CN = { revoked: "已被服务端作废", expired: "已过期", invalid: "无效" };

  // 只对明文文件给清理建议。Keychain 条目一律不建议删 ——
  // 判断依据太弱，删错了（比如误伤桌面应用的条目）代价太大。
  const files = stale.filter((s) => s.kind === "file");
  if (!files.length) return "";

  const o = ["", "  ⚠ 发现陈旧凭据残留："];
  for (const s of files) {
    o.push(`     ${mask(s.token)}  ${REASON_CN[s.reason] ?? s.reason}  ${s.path}`);
    if (s.mtime) o.push(`       最后写入 ${new Date(s.mtime).toLocaleString()}`);
  }
  o.push("");
  o.push("  这是 macOS 上 Keychain 写入失败（比如 SSH 会话里 Keychain 被锁）时留下的");
  o.push("  兜底文件。后来你重新登录，新 token 进了 Keychain，旧的被轮换作废了。");
  o.push("  平时无害，但在 cron / launchd / SSH 里读不到 Keychain 时会回退到它，");
  o.push("  然后报一个跟真实原因无关的鉴权错误。建议归档：");
  o.push("");
  for (const s of files) o.push(`     mv ${s.path} ${s.path}.stale.bak`);
  o.push("");
  o.push("  加 --no-audit 可以跳过这项检查。");
  o.push("");
  return o.join("\n");
}

// ---------- 缓存 ----------
const loadCache = () => (FORGET ? null : readJson(CACHE_PATH));
function saveCache(entry) {
  try {
    mkdirSync(dirname(CACHE_PATH), { recursive: true, mode: 0o700 });
    writeFileSync(CACHE_PATH, JSON.stringify(entry, null, 2), { mode: 0o600 });
  } catch {}
}

// ---------- 格式化 ----------
const num = (v) => {
  if (v === null || v === undefined) return null;
  const n = typeof v === "number" ? v : Number(String(v).trim());
  return Number.isFinite(n) ? n : null;
};

function fmtLeft(iso) {
  if (!iso) return "";
  const t = typeof iso === "number" ? new Date(iso * (iso > 1e11 ? 1 : 1000)) : new Date(iso);
  if (isNaN(t)) return "";
  const s = Math.max(0, Math.round((t - Date.now()) / 1000));
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return `重置 ${d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`}`;
}

const bar = (pct, w = 22) => {
  const f = Math.min(w, Math.max(0, Math.round((pct / 100) * w)));
  return "█".repeat(f) + "░".repeat(w - f);
};

const color = (pct, s) =>
  process.stdout.isTTY ? `\x1b[${pct >= 90 ? 31 : pct >= 80 ? 33 : 32}m${s}\x1b[0m` : s;

const LABELS = {
  five_hour: "5 小时",
  seven_day: "7 天",
  seven_day_opus: "7 天 · Opus",
  seven_day_sonnet: "7 天 · Sonnet",
  seven_day_oauth_apps: "7 天 · OAuth 应用",
};

const PCT_KEYS = ["utilization", "used_percentage", "usedPercentage", "used_percent"];
const RESET_KEYS = ["resets_at", "resetsAt", "reset_at", "resets_in_seconds"];

function extractWindows(data) {
  const root = data.rate_limits ?? data.usage ?? data;
  const rows = [];
  for (const [key, val] of Object.entries(root)) {
    if (!val || typeof val !== "object" || Array.isArray(val)) continue;
    let pct = null;
    for (const k of PCT_KEYS) {
      const n = num(val[k]);
      if (n !== null) { pct = n; break; }
    }
    if (pct === null) continue;
    if (pct > 0 && pct <= 1 && !Number.isInteger(pct)) pct *= 100;
    let reset = null;
    for (const k of RESET_KEYS) {
      if (val[k] !== undefined && val[k] !== null) { reset = val[k]; break; }
    }
    rows.push({ key, label: LABELS[key] ?? key, pct, reset });
  }
  return rows;
}

function renderHuman(data, source, token) {
  const rows = extractWindows(data);
  const o = ["", "  Claude Code 额度", "  " + "─".repeat(52)];
  o.push(`  凭据  ${source}`);
  o.push(`        ${mask(token)}`);
  o.push("");
  if (!rows.length) {
    o.push("  没解析出窗口字段，去掉 --human 看原始 JSON。", "");
    return o.join("\n");
  }
  const width = Math.max(...rows.map((r) => r.label.length)) + 2;
  for (const r of rows) {
    o.push(
      `  ${r.label.padEnd(width)} ${color(r.pct, bar(r.pct))} ` +
        `${color(r.pct, (r.pct.toFixed(1) + "%").padStart(6))}   ${fmtLeft(r.reset)}`
    );
  }
  o.push("");
  return o.join("\n");
}

// ---------- main ----------
let candidates = collectCandidates();
const redirects = detectRedirect();

// 缓存只在候选已有的前提下提前，不破坏 Keychain 优先的顺序
const cached = loadCache();
if (cached?.token && looksLikeToken(cached.token)) {
  const idx = candidates.findIndex((c) => c.token === cached.token);
  if (idx > 0) candidates.unshift(...candidates.splice(idx, 1));
}

if (!candidates.length) {
  let msg = "没找到 Claude 的 OAuth 凭据。已扫描：\n  env CLAUDE_CODE_OAUTH_TOKEN\n";
  if (IS_MAC) msg += `  Keychain: ${keychainServiceNames().join(" / ")}\n`;
  msg += `  ${join(CLAUDE_DIR, ".credentials.json")}\n`;
  if (redirects.length) {
    msg +=
      "\n  另外检测到 Claude Code 被改道到了第三方：\n" +
      redirects.map((r) => `    ${r.url}  (${r.where})`).join("\n") +
      "\n  这种情况下没有 Anthropic 订阅额度可查，去查对应供应商的用量。";
  } else {
    msg += "\n  先跑一次 claude 登录，或者用 --token 指定。";
  }
  fail(msg);
}

if (DISCOVER) {
  const results = [];
  for (const c of candidates) {
    const r = await probe(c.token);
    results.push({
      source: c.source, kind: c.kind, token: mask(c.token),
      ok: r.ok, status: r.status, reason: r.ok ? null : r.reason,
      note: r.ok ? "可用" : r.note,
    });
  }
  if (HUMAN) {
    process.stdout.write("\n  候选凭据探测（按优先级）\n  " + "─".repeat(52) + "\n");
    for (const r of results) {
      const tag = r.ok ? "\x1b[32m✓\x1b[0m" : "\x1b[31m✗\x1b[0m";
      const reason = r.reason ? ` \x1b[31m[${r.reason}]\x1b[0m` : "";
      process.stdout.write(
        `  ${tag} ${r.token.padEnd(18)} ${String(r.status).padEnd(4)} ${r.source}${reason}\n` +
          (r.ok ? "" : `      \x1b[2m${r.note}\x1b[0m\n`)
      );
    }
    const stale = results
      .filter((r) => !r.ok && ["revoked", "expired", "invalid"].includes(r.reason))
      .map((r) => {
        const c = candidates.find((x) => mask(x.token) === r.token);
        return { ...c, reason: r.reason };
      })
      .filter((s) => s.kind === "file" || s.kind === "keychain");
    if (stale.length) process.stdout.write(renderStaleWarning(stale));
    if (redirects.length) {
      process.stdout.write("\n  \x1b[33m⚠\x1b[0m Claude Code 已被改道，订阅额度与实际用量无关：\n");
      for (const r of redirects) process.stdout.write(`    ${r.url}  (${r.where})\n`);
      process.stdout.write("\n");
    }
  } else {
    process.stdout.write(JSON.stringify({ candidates: results, redirects }, null, 2) + "\n");
  }
  process.exit(0);
}

let hit = null;
const failures = [];
for (const c of candidates) {
  const r = await probe(c.token);
  if (r.ok) { hit = { ...c, data: r.data }; break; }
  failures.push({ ...c, ...r });
}

if (!hit) {
  const lines = failures.map(
    (f) => `  ${String(f.status).padEnd(4)} [${f.reason}] ${f.source} — ${f.note}`
  );
  fail(
    `${candidates.length} 个候选凭据全部失败：\n${lines.join("\n")}\n\n` +
      "  revoked  → 该凭据已被作废，重新登录后清掉残留\n" +
      "  expired  → 跑一次 claude update 或重新登录触发刷新\n" +
      "  invalid  → 凭据格式或账号有问题\n\n" +
      "  如果这是在 cron / launchd 里跑，多半是读不到 Keychain。\n" +
      "  用长效 token 绕开：claude setup-token 拿到 token 后\n" +
      "    node quota-check-claude.mjs --token <token> --save\n" +
      "  用 --discover --human 看详情。"
  );
}

if (SAVE || cached?.token !== hit.token) {
  saveCache({ token: hit.token, source: hit.source, kind: hit.kind, savedAt: new Date().toISOString() });
}

process.stdout.write(
  HUMAN ? renderHuman(hit.data, hit.source, hit.token) + "\n" : JSON.stringify(hit.data, null, 2) + "\n"
);

// 审计走 stderr，不污染 JSON 输出；非交互环境（cron）默认跳过
if (!NO_AUDIT && process.stdout.isTTY) {
  const stale = await auditStale(candidates, hit.token);
  if (stale.length) process.stderr.write(renderStaleWarning(stale));
}
