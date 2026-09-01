#!/usr/bin/env node
// quota-check-codex.mjs
// 读取本地 ~/.codex/auth.json 的凭据，查询 Codex 的 5 小时 / 周额度用量。
// 默认输出原始 JSON；加 --human 输出人类可读格式。
//
//   node quota-check-codex.mjs
//   node quota-check-codex.mjs --human
//   node quota-check-codex.mjs --human --auth /path/to/other/auth.json
//   node quota-check-codex.mjs --whoami        # 只看这份凭据属于哪个账号
//
// 需要 Node 18+（用到全局 fetch）。无第三方依赖。

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const ENDPOINT = "https://chatgpt.com/backend-api/wham/usage";

// ---------- 参数 ----------
const argv = process.argv.slice(2);
const has = (f) => argv.includes(f);
const valueOf = (f) => {
  const i = argv.indexOf(f);
  return i >= 0 ? argv[i + 1] : undefined;
};

const HUMAN = has("--human");
const WHOAMI = has("--whoami");

const authPath =
  valueOf("--auth") ||
  join(process.env.CODEX_HOME || join(homedir(), ".codex"), "auth.json");

// ---------- 读凭据 ----------
function loadAuth(path) {
  let raw;
  try {
    raw = readFileSync(path, "utf8");
  } catch (e) {
    fail(
      `读不到凭据文件：${path}\n` +
        `  ${e.code === "ENOENT" ? "文件不存在，先跑一次 codex login" : e.message}`
    );
  }
  let json;
  try {
    json = JSON.parse(raw);
  } catch {
    fail(`凭据文件不是合法 JSON：${path}`);
  }
  // 新版结构是 { tokens: { access_token, account_id, id_token } }，
  // 老版本 / 代理写出来的可能是平铺的。
  const t = json.tokens ?? json;
  const accessToken = t.access_token ?? t.accessToken;
  const accountId = t.account_id ?? t.accountId;
  if (!accessToken) fail(`凭据里没有 access_token：${path}`);
  return { accessToken, accountId, idToken: t.id_token ?? t.idToken };
}

// 解出 id_token 里的账号信息，用来确认这份凭据到底是谁。
function identity(idToken) {
  if (!idToken) return null;
  try {
    const payload = JSON.parse(
      Buffer.from(idToken.split(".")[1], "base64url").toString("utf8")
    );
    const auth = payload["https://api.openai.com/auth"] ?? {};
    return {
      email: payload.email ?? null,
      plan: auth.chatgpt_plan_type ?? null,
      accountId: auth.chatgpt_account_id ?? null,
      expiresAt: payload.exp ? new Date(payload.exp * 1000).toISOString() : null,
    };
  } catch {
    return null;
  }
}

// ---------- 抓数据 ----------
async function fetchUsage({ accessToken, accountId }) {
  const headers = {
    Authorization: `Bearer ${accessToken}`,
    Accept: "application/json",
    "User-Agent": "quota-check-codex/1.0",
  };
  if (accountId) headers["ChatGPT-Account-Id"] = accountId;

  let res;
  try {
    res = await fetch(ENDPOINT, {
      headers,
      signal: AbortSignal.timeout(15000),
    });
  } catch (e) {
    fail(`请求失败：${e.message}`);
  }

  const body = await res.text();

  if (res.status === 401 || res.status === 403) {
    fail(
      `HTTP ${res.status} — token 已过期或账号无权访问。\n` +
        `  跑一次 codex login 刷新 ${authPath}\n` +
        `  响应：${body.slice(0, 300)}`
    );
  }
  if (!res.ok) {
    fail(`HTTP ${res.status}\n  响应：${body.slice(0, 500)}`);
  }

  try {
    return JSON.parse(body);
  } catch {
    fail(`返回的不是 JSON（接口可能变了）：\n${body.slice(0, 500)}`);
  }
}

// ---------- 人类可读格式化 ----------
// 不硬编码字段名。递归扫描整个响应，凡是「含有百分比字段的对象」都当成一个额度窗口。

const PCT_KEYS = [
  "used_percent", "usedPercent",
  "used_percentage", "usedPercentage",
  "utilization",
  "percent_used", "percentUsed",
];
const RESET_SEC_KEYS = [
  "resets_in_seconds", "resetsInSeconds",
  "reset_after_seconds", "resetAfterSeconds",
  "seconds_until_reset", "secondsUntilReset",
  "reset_in_seconds", "resetInSeconds",
];
const RESET_AT_KEYS = [
  "resets_at", "resetsAt", "reset_at", "resetAt",
  "reset_time", "resetTime", "resets_at_utc",
];
const WINDOW_KEYS = [
  "window_minutes", "windowMinutes",
  "window_size_seconds", "windowSizeSeconds",
  "window_hours", "windowHours",
  "window",
];

const pick = (obj, keys) => {
  for (const k of keys) {
    if (obj[k] !== undefined && obj[k] !== null) return { key: k, value: obj[k] };
  }
  return null;
};

function collectWindows(node, path = [], out = []) {
  if (node === null || typeof node !== "object") return out;
  if (Array.isArray(node)) {
    node.forEach((v, i) => collectWindows(v, [...path, String(i)], out));
    return out;
  }
  const pct = pick(node, PCT_KEYS);
  if (pct && typeof pct.value === "number") {
    out.push({
      path: path.length ? path.join(".") : "(root)",
      percent: pct.value,
      resetSeconds: pick(node, RESET_SEC_KEYS),
      resetAt: pick(node, RESET_AT_KEYS),
      window: pick(node, WINDOW_KEYS),
      raw: node,
    });
  }
  for (const [k, v] of Object.entries(node)) {
    collectWindows(v, [...path, k], out);
  }
  return out;
}

function fmtDuration(seconds) {
  const s = Math.max(0, Math.round(seconds));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  return `${m}m`;
}

// 窗口时长归一化成秒，用来给窗口起个人话名字
function windowLabel(w) {
  if (!w) return null;
  const { key, value } = w;
  if (typeof value !== "number") return String(value);
  let sec;
  if (key.includes("minutes")) sec = value * 60;
  else if (key.includes("hours")) sec = value * 3600;
  else if (key.includes("seconds")) sec = value;
  else return null;
  if (sec >= 604800 * 0.9 && sec <= 604800 * 1.1) return "周";
  if (sec >= 86400 * 0.9 && sec <= 86400 * 1.1) return "日";
  if (sec < 86400) return `${Math.round(sec / 3600)}h`;
  return `${Math.round(sec / 86400)}d`;
}

function bar(pct, width = 24) {
  const filled = Math.min(width, Math.max(0, Math.round((pct / 100) * width)));
  return "█".repeat(filled) + "░".repeat(width - filled);
}

function colorize(pct, text) {
  if (!process.stdout.isTTY) return text;
  const code = pct >= 90 ? 31 : pct >= 75 ? 33 : 32; // 红 / 黄 / 绿
  return `\x1b[${code}m${text}\x1b[0m`;
}

function renderHuman(data, ident) {
  const lines = [];
  lines.push("");
  lines.push("  Codex 额度");
  lines.push("  " + "─".repeat(46));

  if (ident) {
    const bits = [ident.email, ident.plan].filter(Boolean).join("  ·  ");
    if (bits) lines.push(`  ${bits}`);
    lines.push(`  凭据  ${authPath}`);
    lines.push("");
  }

  const windows = collectWindows(data);

  if (windows.length === 0) {
    lines.push("  没在响应里找到百分比字段。");
    lines.push("  去掉 --human 看原始 JSON，接口结构可能变了。");
    lines.push("");
    return lines.join("\n");
  }

  for (const w of windows) {
    const pct = w.percent <= 1 && w.percent > 0 ? w.percent * 100 : w.percent;
    const label = windowLabel(w.window) ?? w.path.split(".").pop();

    let reset = "";
    if (w.resetSeconds && typeof w.resetSeconds.value === "number") {
      reset = `重置 ${fmtDuration(w.resetSeconds.value)}`;
    } else if (w.resetAt) {
      const v = w.resetAt.value;
      const t = typeof v === "number" ? new Date(v * (v > 1e11 ? 1 : 1000)) : new Date(v);
      if (!isNaN(t)) {
        const left = (t - Date.now()) / 1000;
        reset = `重置 ${fmtDuration(left)}  (${t.toLocaleString()})`;
      }
    }

    lines.push(
      `  ${String(label).padEnd(6)} ${colorize(pct, bar(pct))} ` +
        `${colorize(pct, (pct.toFixed(1) + "%").padStart(6))}   ${reset}`
    );
    lines.push(`  ${" ".repeat(6)} \x1b[2m${w.path}\x1b[0m`);
  }

  lines.push("");
  return lines.join("\n");
}

function fail(msg) {
  process.stderr.write(`\n  ✗ ${msg}\n\n`);
  process.exit(1);
}

// ---------- main ----------
const auth = loadAuth(authPath);
const ident = identity(auth.idToken);

if (WHOAMI) {
  const payload = { authPath, accountId: auth.accountId, ...ident };
  process.stdout.write(
    HUMAN
      ? "\n" +
          Object.entries(payload)
            .map(([k, v]) => `  ${k.padEnd(12)} ${v ?? "-"}`)
            .join("\n") +
          "\n\n"
      : JSON.stringify(payload, null, 2) + "\n"
  );
  process.exit(0);
}

const data = await fetchUsage(auth);

if (HUMAN) {
  process.stdout.write(renderHuman(data, ident) + "\n");
} else {
  process.stdout.write(JSON.stringify(data, null, 2) + "\n");
}
