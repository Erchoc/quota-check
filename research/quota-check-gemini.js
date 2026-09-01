#!/usr/bin/env node
// quota-check-gemini.mjs
// 统计 Gemini CLI 的当日用量。
//
// ⚠️ 跟 Claude / Codex / Kimi 不同：Gemini 没有任何可查额度的接口。
//    /stats model 只能在交互式会话里用（Ink 渲染，管道和重定向都拿不到输出）。
//    所以这个脚本走的是本地会话文件聚合 —— 是估算，不是官方口径。
//    session 文件和 API 请求不是 1:1（一个 session 里有多轮），所以数出来的
//    是下界，能当预警信号，但不会在第 999 次请求时精确拦住你。
//
//   node quota-check-gemini.mjs                   # 原始 JSON
//   node quota-check-gemini.mjs --human           # 人类可读
//   node quota-check-gemini.mjs --limit 1500      # 覆盖每日额度上限
//   node quota-check-gemini.mjs --days 7          # 看最近 7 天趋势
//   node quota-check-gemini.mjs --dir ~/.gemini   # 指定数据目录
//
// 需要 Node 18+。无第三方依赖。

import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

// 额度按账号类型不同，默认按 Code Assist 个人版的 1000。
// Standard 1500 / Enterprise 2000 / 纯 API Key 免费档 250，用 --limit 覆盖。
const DEFAULT_LIMIT = 1000;

const argv = process.argv.slice(2);
const has = (f) => argv.includes(f);
const valueOf = (f) => {
  const i = argv.indexOf(f);
  return i >= 0 ? argv[i + 1] : undefined;
};

const HUMAN = has("--human");
const LIMIT = Number(valueOf("--limit")) || DEFAULT_LIMIT;
const DAYS = Math.max(1, Number(valueOf("--days")) || 1);
const GEMINI_DIR =
  valueOf("--dir")?.replace(/^~/, homedir()) ||
  process.env.GEMINI_DIR ||
  join(homedir(), ".gemini");

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

// 配额按太平洋时间午夜重置
function pacificDateKey(d = new Date()) {
  return new Intl.DateTimeFormat("en-CA", {
    timeZone: "America/Los_Angeles",
    year: "numeric", month: "2-digit", day: "2-digit",
  }).format(d);
}

function pacificResetLeft() {
  // 下一个太平洋午夜距现在多久
  const now = new Date();
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone: "America/Los_Angeles", hour12: false,
    hour: "2-digit", minute: "2-digit", second: "2-digit",
  }).formatToParts(now);
  const g = (t) => Number(parts.find((p) => p.type === t).value);
  const secsToday = (g("hour") % 24) * 3600 + g("minute") * 60 + g("second");
  return 86400 - secsToday;
}

// ---------- 扫会话文件 ----------
// 布局：~/.gemini/tmp/<project-hash>/chats/session-YYYY-MM-DD*.json
function findSessionFiles() {
  const tmp = join(GEMINI_DIR, "tmp");
  if (!existsSync(tmp)) return [];
  const files = [];
  let projects = [];
  try {
    projects = readdirSync(tmp, { withFileTypes: true }).filter((d) => d.isDirectory());
  } catch {
    return [];
  }
  for (const p of projects) {
    const chats = join(tmp, p.name, "chats");
    if (!existsSync(chats)) continue;
    let entries = [];
    try {
      entries = readdirSync(chats).filter((f) => f.endsWith(".json"));
    } catch {
      continue;
    }
    for (const f of entries) {
      files.push({ path: join(chats, f), project: p.name, name: f });
    }
  }
  return files;
}

// 从文件名或 mtime 推出属于哪一天（太平洋时区）
function dateKeyOf(file) {
  const m = file.name.match(/(\d{4}-\d{2}-\d{2})/);
  if (m) return m[1];
  try {
    return pacificDateKey(statSync(file.path).mtime);
  } catch {
    return null;
  }
}

// 深挖 token 统计。各版本字段名不一，能找到多少算多少。
function sumTokens(node, acc = { total: 0, input: 0, output: 0, turns: 0 }) {
  if (!node || typeof node !== "object") return acc;
  if (Array.isArray(node)) {
    for (const v of node) sumTokens(v, acc);
    return acc;
  }
  const t = node.tokens ?? node.usage ?? node.usageMetadata;
  if (t && typeof t === "object") {
    const n = (v) => (Number.isFinite(Number(v)) ? Number(v) : 0);
    acc.total += n(t.total ?? t.totalTokenCount ?? t.total_tokens);
    acc.input += n(t.input ?? t.promptTokenCount ?? t.input_tokens);
    acc.output += n(t.output ?? t.candidatesTokenCount ?? t.output_tokens);
  }
  // 对话轮次：messages / history / turns 数组的长度
  for (const k of ["messages", "history", "turns"]) {
    if (Array.isArray(node[k])) acc.turns += node[k].length;
  }
  for (const v of Object.values(node)) {
    if (v && typeof v === "object") sumTokens(v, acc);
  }
  return acc;
}

// ---------- 账号信息（尽力而为） ----------
function accountInfo() {
  const out = {};
  const accounts = readJson(join(GEMINI_DIR, "google_accounts.json"));
  if (accounts) {
    out.active = accounts.active ?? accounts.activeAccount ?? null;
    if (Array.isArray(accounts.old)) out.others = accounts.old.length;
  }
  const settings = readJson(join(GEMINI_DIR, "settings.json"));
  if (settings) {
    out.authType = settings.selectedAuthType ?? settings.security?.auth?.selectedType ?? null;
  }
  out.hasOAuth = existsSync(join(GEMINI_DIR, "oauth_creds.json"));
  return out;
}

// ---------- 格式化 ----------
const bar = (pct, w = 22) => {
  const f = Math.min(w, Math.max(0, Math.round((pct / 100) * w)));
  return "█".repeat(f) + "░".repeat(w - f);
};

const color = (pct, s) =>
  process.stdout.isTTY ? `\x1b[${pct >= 90 ? 31 : pct >= 80 ? 33 : 32}m${s}\x1b[0m` : s;

const fmtLeft = (secs) => {
  const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m`;
};

const fmtNum = (n) => n.toLocaleString("en-US");

// ---------- main ----------
if (!existsSync(GEMINI_DIR)) {
  fail(`找不到 Gemini 数据目录：${GEMINI_DIR}\n  用 --dir 指定，或先跑一次 gemini`);
}

const files = findSessionFiles();
const today = pacificDateKey();

// 按日期分组
const byDay = new Map();
for (const f of files) {
  const key = dateKeyOf(f);
  if (!key) continue;
  if (!byDay.has(key)) byDay.set(key, { date: key, sessions: 0, tokens: { total: 0, input: 0, output: 0, turns: 0 }, projects: new Set() });
  const day = byDay.get(key);
  day.sessions += 1;
  day.projects.add(f.project);
  const data = readJson(f.path);
  if (data) {
    const t = sumTokens(data);
    day.tokens.total += t.total;
    day.tokens.input += t.input;
    day.tokens.output += t.output;
    day.tokens.turns += t.turns;
  }
}

const sorted = [...byDay.values()].sort((a, b) => b.date.localeCompare(a.date)).slice(0, DAYS);
const todayRow = byDay.get(today) ?? { date: today, sessions: 0, tokens: { total: 0, input: 0, output: 0, turns: 0 }, projects: new Set() };

const payload = {
  source: "本地会话文件聚合（估算，非官方额度）",
  geminiDir: GEMINI_DIR,
  account: accountInfo(),
  today: {
    date: today,
    sessions: todayRow.sessions,
    turns: todayRow.tokens.turns,
    tokens: { total: todayRow.tokens.total, input: todayRow.tokens.input, output: todayRow.tokens.output },
    limit: LIMIT,
    limitBasis: "每日请求数上限；--limit 可覆盖",
    resetsInSeconds: pacificResetLeft(),
    resetsTimezone: "America/Los_Angeles",
  },
  recent: sorted.map((d) => ({
    date: d.date,
    sessions: d.sessions,
    turns: d.tokens.turns,
    tokens: d.tokens.total,
    projects: d.projects.size,
  })),
};

if (!HUMAN) {
  process.stdout.write(JSON.stringify(payload, null, 2) + "\n");
  process.exit(0);
}

const o = ["", "  Gemini CLI 用量", "  " + "─".repeat(56)];
o.push("  \x1b[33m估算值\x1b[0m — Gemini 没有额度查询接口，这是本地会话文件统计");
o.push(`  目录  ${GEMINI_DIR}`);
const acc = payload.account;
if (acc.active) o.push(`  账号  ${acc.active}${acc.authType ? `  (${acc.authType})` : ""}`);
o.push("");

// 用轮次数当请求数的近似（比 session 数更接近真实请求量，但仍是下界）
const approxRequests = todayRow.tokens.turns || todayRow.sessions;
const pct = Math.min(100, (approxRequests / LIMIT) * 100);

o.push(`  今日  ${color(pct, bar(pct))} ${color(pct, (pct.toFixed(1) + "%").padStart(6))}`);
o.push(`        约 ${approxRequests} / ${LIMIT} 次   重置 ${fmtLeft(payload.today.resetsInSeconds)} (太平洋午夜)`);
o.push(`        ${todayRow.sessions} 个会话 · ${fmtNum(todayRow.tokens.total)} tokens`);
o.push("");

if (sorted.length > 1) {
  o.push("  最近几天");
  const maxT = Math.max(...sorted.map((d) => d.tokens.turns || d.sessions), 1);
  for (const d of sorted) {
    const v = d.tokens.turns || d.sessions;
    const w = Math.max(1, Math.round((v / maxT) * 18));
    const mark = d.date === today ? "›" : " ";
    o.push(
      `  ${mark} ${d.date}  ${"▇".repeat(w).padEnd(18)} ` +
        `${String(v).padStart(4)} 次 · ${fmtNum(d.tokens.total)} tokens`
    );
  }
  o.push("");
}

o.push("  \x1b[2m额度参考：Code Assist 个人版 1000/天 · Standard 1500 · Enterprise 2000 · API Key 免费档 250\x1b[0m");
o.push("");

process.stdout.write(o.join("\n") + "\n");
