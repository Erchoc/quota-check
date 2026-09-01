#!/usr/bin/env node
// quota-check-kimi.mjs
// 查询 Kimi Code 的 5 小时窗口 / 周额度用量。
//
// 凭据策略：不猜「哪个文件是对的」，而是收集所有候选，逐个打接口探测，
// 用第一个返回 200 的。命中结果会缓存，下次直接用。
//
//   node quota-check-kimi.mjs                       # 原始 JSON
//   node quota-check-kimi.mjs --human               # 人类可读
//   node quota-check-kimi.mjs --discover --human    # 列出所有候选及探测结果
//   node quota-check-kimi.mjs --key sk-xxx --save   # 手动指定并记住
//   node quota-check-kimi.mjs --forget              # 清缓存重新探测
//   node quota-check-kimi.mjs --base https://api.moonshot.cn/coding/v1
//
// 只兼容当前版本的 Kimi Code CLI / Claude Code / Pi / cc-switch，
// 不适配已下线的 ~/.kimi 旧版 kimi-cli。
// 需要 Node 18+（cc-switch 的 SQLite 扫描需要 Node 22+）。无第三方依赖。

import { readFileSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { createRequire } from "node:module";
import { execSync } from "node:child_process";

const require = createRequire(import.meta.url);

const DEFAULT_BASE = "https://api.kimi.com/coding/v1";
const CACHE_PATH = join(homedir(), ".config", "quota-check", "kimi.json");

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
const BASE = (valueOf("--base") || DEFAULT_BASE).replace(/\/+$/, "");

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

// cc-switch 本地路由模式会往 settings.json 写占位符，不是真 key
const SENTINELS = new Set(["PROXY_MANAGED", "MANAGED", "null", "undefined", "your-api-key"]);
const looksLikeKey = (t) =>
  typeof t === "string" && t.trim().length >= 16 && !SENTINELS.has(t.trim());

// ---------- 候选收集 ----------
// 每个候选是 { token, source }。不判断谁更对，全部交给探测。

function collectCandidates() {
  const out = [];
  const seen = new Set();
  const push = (token, source) => {
    if (!looksLikeKey(token)) return;
    const t = token.trim();
    if (seen.has(t)) return; // 同一个 key 出现在多处只探测一次
    seen.add(t);
    out.push({ token: t, source });
  };

  // 1. 命令行
  push(valueOf("--key"), "--key 参数");

  // 2. 环境变量
  for (const n of ["KIMI_API_KEY", "MOONSHOT_API_KEY", "KIMI_CODE_API_KEY"]) {
    push(process.env[n], `env ${n}`);
  }
  // ANTHROPIC_* 只在 base url 指向 Kimi 时才收
  if (/kimi|moonshot/i.test(process.env.ANTHROPIC_BASE_URL || "")) {
    push(process.env.ANTHROPIC_AUTH_TOKEN, "env ANTHROPIC_AUTH_TOKEN");
    push(process.env.ANTHROPIC_API_KEY, "env ANTHROPIC_API_KEY");
  }

  // 3. Kimi Code CLI 的 OAuth 凭据
  const codeHome = process.env.KIMI_CODE_HOME || join(homedir(), ".kimi-code");
  const credPath = join(codeHome, "credentials", "kimi-code.json");
  const cred = readJson(credPath);
  if (cred) {
    const t = cred.tokens ?? cred.credential ?? cred;
    for (const k of ["access_token", "accessToken", "api_key", "apiKey", "token"]) {
      push(t[k], credPath);
    }
  }

  // 4. Claude Code 各级 settings —— cc-switch 激活的 provider 就落在这里
  const claudeDir = process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude");
  const settingsFiles = [
    join(claudeDir, "settings.json"),
    join(claudeDir, "settings.local.json"),
    join(process.cwd(), ".claude", "settings.json"),
    join(process.cwd(), ".claude", "settings.local.json"),
  ];
  for (const f of settingsFiles) {
    const j = readJson(f);
    if (!j) continue;
    const env = j.env ?? {};
    const url = env.ANTHROPIC_BASE_URL || "";
    // base url 不像 Kimi 也照收，探测会筛掉，只是标注一下方便排查
    const tag = /kimi|moonshot/i.test(url) ? "" : ` [base=${url || "未设置"}]`;
    push(env.ANTHROPIC_AUTH_TOKEN, `${f}${tag}`);
    push(env.ANTHROPIC_API_KEY, `${f}${tag}`);
    if (typeof j.apiKeyHelper === "string" && j.apiKeyHelper.trim()) {
      out.push({ helper: j.apiKeyHelper, source: `${f} (apiKeyHelper)` });
    }
  }

  // 5. Pi
  const piAuth = join(homedir(), ".pi", "agent", "auth.json");
  const pi = readJson(piAuth);
  if (pi) {
    for (const [k, v] of Object.entries(pi)) {
      if (!v || typeof v !== "object") continue;
      if (!/kimi|moonshot/i.test(k)) continue;
      for (const kk of ["access_token", "accessToken", "api_key", "apiKey"]) {
        push(v[kk], `${piAuth} → ${k}`);
      }
    }
    push(pi.access_token, piAuth);
  }
  const piCfgPath = join(homedir(), ".pi", "providers", "kimi-coding", "config.json");
  const piCfg = readJson(piCfgPath);
  if (piCfg) push(piCfg.api_key ?? piCfg.apiKey, piCfgPath);

  // 6. cc-switch 的 SQLite（best effort）
  for (const c of collectFromCcSwitch()) push(c.token, c.source);

  return out;
}

// cc-switch 是 SSOT + SQLite，切换时才写 settings.json。
// 直接扫库能拿到所有 provider 的 key，包括本地路由模式下被占位符挡住的那个。
function collectFromCcSwitch() {
  const dbPath = join(homedir(), ".cc-switch", "cc-switch.db");
  if (!existsSync(dbPath)) return [];

  let DatabaseSync;
  try {
    ({ DatabaseSync } = require("node:sqlite")); // Node 22 需 --experimental-sqlite，24+ 直接可用
  } catch {
    return [];
  }

  const found = [];
  try {
    const conn = new DatabaseSync(dbPath, { readOnly: true });
    const tables = conn.prepare("SELECT name FROM sqlite_master WHERE type='table'").all();
    for (const { name } of tables) {
      let rows;
      try {
        rows = conn.prepare(`SELECT * FROM "${name}"`).all();
      } catch {
        continue;
      }
      for (const row of rows) {
        for (const v of Object.values(row)) {
          if (typeof v !== "string") continue;
          // provider 配置以 JSON 文本存，扫 key 形态的串
          for (const m of v.matchAll(/\b(sk-[A-Za-z0-9_\-]{16,})\b/g)) {
            found.push({ token: m[1], source: `cc-switch.db → ${name}` });
          }
        }
      }
    }
    conn.close();
  } catch {
    return [];
  }
  return found;
}

// apiKeyHelper 是一条 shell 命令，跑一下拿输出
function runHelper(cmd) {
  try {
    const out = execSync(cmd, { timeout: 5000, encoding: "utf8" }).trim();
    return looksLikeKey(out) ? out : null;
  } catch {
    return null;
  }
}

// ---------- 探测 ----------
async function probe(token) {
  try {
    const res = await fetch(`${BASE}/usages`, {
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/json",
        "User-Agent": "quota-check-kimi/2.0",
      },
      signal: AbortSignal.timeout(12000),
    });
    const body = await res.text();
    if (!res.ok) return { ok: false, status: res.status, note: body.slice(0, 140) };
    try {
      return { ok: true, status: res.status, data: JSON.parse(body) };
    } catch {
      return { ok: false, status: res.status, note: "返回不是 JSON" };
    }
  } catch (e) {
    return { ok: false, status: 0, note: e.message };
  }
}

// ---------- 缓存 ----------
const loadCache = () => (FORGET ? null : readJson(CACHE_PATH));
function saveCache(entry) {
  try {
    mkdirSync(dirname(CACHE_PATH), { recursive: true, mode: 0o700 });
    writeFileSync(CACHE_PATH, JSON.stringify(entry, null, 2), { mode: 0o600 });
  } catch {}
}

const mask = (t) => (t.length <= 12 ? "***" : `${t.slice(0, 6)}…${t.slice(-4)}`);

// ---------- 格式化 ----------
const num = (v) => {
  if (v === null || v === undefined) return null;
  const n = typeof v === "number" ? v : Number(String(v).trim());
  return Number.isFinite(n) ? n : null;
};

const UNIT_SECONDS = {
  TIME_UNIT_SECOND: 1, TIME_UNIT_MINUTE: 60, TIME_UNIT_HOUR: 3600,
  TIME_UNIT_DAY: 86400, TIME_UNIT_WEEK: 604800,
  SECOND: 1, MINUTE: 60, HOUR: 3600, DAY: 86400, WEEK: 604800,
};

function windowLabel(w) {
  if (!w) return null;
  const d = num(w.duration);
  const unit = UNIT_SECONDS[w.timeUnit ?? w.time_unit];
  if (d === null || !unit) return null;
  const sec = d * unit;
  if (sec >= 604800 && sec % 604800 === 0) return `${sec / 604800} 周`;
  if (sec >= 86400 && sec % 86400 === 0) return `${sec / 86400} 天`;
  if (sec % 3600 === 0) return `${sec / 3600} 小时`;
  return `${Math.round(sec / 60)} 分钟`;
}

function fmtLeft(iso) {
  if (!iso) return "";
  const t = new Date(iso);
  if (isNaN(t)) return String(iso);
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

function normalize(detail, label) {
  if (!detail) return null;
  const limit = num(detail.limit);
  const used = num(detail.used);
  const remaining = num(detail.remaining);
  if (limit === null && used === null) return null;
  const u = used ?? (limit !== null && remaining !== null ? limit - remaining : null);
  return {
    label,
    used: u,
    limit,
    remaining: remaining ?? (limit !== null && u !== null ? limit - u : null),
    pct: limit ? ((u ?? 0) / limit) * 100 : 0,
    resetTime: detail.resetTime ?? detail.reset_time ?? null,
  };
}

function renderHuman(data, source, token) {
  const rows = [];
  const top = normalize(data.usage ?? data.membership, "周额度");
  if (top) rows.push(top);
  for (const item of Array.isArray(data.limits) ? data.limits : []) {
    const r = normalize(item.detail ?? item.usage ?? item, windowLabel(item.window) ?? "窗口");
    if (r) rows.push(r);
  }

  const o = ["", "  Kimi Code 额度", "  " + "─".repeat(52)];
  o.push(`  凭据  ${source}`);
  o.push(`        ${mask(token)}`);
  o.push("");

  if (!rows.length) {
    o.push("  没解析出额度字段，去掉 --human 看原始 JSON。", "");
    return o.join("\n");
  }

  for (const r of rows) {
    o.push(`  ${r.label.padEnd(10)} ${color(r.pct, bar(r.pct))} ${color(r.pct, (r.pct.toFixed(1) + "%").padStart(6))}`);
    o.push(
      `  ${" ".repeat(10)} ${r.used ?? "?"} / ${r.limit ?? "?"}` +
        (r.remaining !== null ? `  剩 ${r.remaining}` : "") +
        (r.resetTime ? `  ${fmtLeft(r.resetTime)}` : "")
    );
    o.push("");
  }
  return o.join("\n");
}

// ---------- main ----------
let candidates = collectCandidates();

// 展开 apiKeyHelper
candidates = candidates.flatMap((c) => {
  if (!c.helper) return [c];
  const t = runHelper(c.helper);
  return t ? [{ token: t, source: c.source }] : [];
});

// 缓存命中的排最前，省掉无谓探测
const cached = loadCache();
if (cached?.token && looksLikeKey(cached.token)) {
  candidates = [
    { token: cached.token, source: `${cached.source} (缓存)` },
    ...candidates.filter((c) => c.token !== cached.token),
  ];
}

if (!candidates.length) {
  fail(
    "没找到任何候选凭据。已扫描：\n" +
      "  KIMI_API_KEY / MOONSHOT_API_KEY / ANTHROPIC_AUTH_TOKEN 等环境变量\n" +
      "  ~/.kimi-code/credentials/kimi-code.json\n" +
      "  ~/.claude/settings.json 及项目级 .claude/settings*.json\n" +
      "  ~/.pi/agent/auth.json、~/.pi/providers/kimi-coding/config.json\n" +
      "  ~/.cc-switch/cc-switch.db (需 Node 22+)\n\n" +
      "  直接指定：--key sk-xxx --save"
  );
}

if (DISCOVER) {
  const results = [];
  for (const c of candidates) {
    const r = await probe(c.token);
    results.push({ source: c.source, token: mask(c.token), ok: r.ok, status: r.status, note: r.ok ? "可用" : r.note });
  }
  if (HUMAN) {
    process.stdout.write(`\n  候选凭据探测  (${BASE})\n  ` + "─".repeat(52) + "\n");
    for (const r of results) {
      process.stdout.write(
        `  ${r.ok ? "\x1b[32m✓\x1b[0m" : "\x1b[31m✗\x1b[0m"} ${r.token.padEnd(14)} ${String(r.status).padEnd(4)} ${r.source}\n` +
          (r.ok ? "" : `      \x1b[2m${r.note}\x1b[0m\n`)
      );
    }
    process.stdout.write("\n");
  } else {
    process.stdout.write(JSON.stringify(results, null, 2) + "\n");
  }
  process.exit(0);
}

// 逐个探测，第一个通过的就用
let hit = null;
const failures = [];
for (const c of candidates) {
  const r = await probe(c.token);
  if (r.ok) {
    hit = { ...c, data: r.data };
    break;
  }
  failures.push(`  ${String(r.status).padEnd(4)} ${c.source} — ${r.note}`);
}

if (!hit) {
  fail(
    `${candidates.length} 个候选凭据全部探测失败：\n${failures.join("\n")}\n\n` +
      "  常见原因：token 过期（重跑 kimi /login）；\n" +
      "  区域不对（国内订阅试 --base 指向 moonshot.cn 的地址）；\n" +
      "  这些 key 根本不是 Kimi 的（比如 cc-switch 当前激活的是别家供应商）。\n" +
      "  用 --discover --human 看每个候选的详情。"
  );
}

if (SAVE || cached?.token !== hit.token) {
  saveCache({ token: hit.token, source: hit.source, base: BASE, savedAt: new Date().toISOString() });
}

process.stdout.write(
  HUMAN ? renderHuman(hit.data, hit.source, hit.token) + "\n" : JSON.stringify(hit.data, null, 2) + "\n"
);
