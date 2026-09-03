# quota-check

Check Coding Agent quota usage (5-hour / weekly windows) from the terminal.
Prebuilt Rust binary, zero runtime dependencies.

```bash
npx quota-check all               # every provider you are logged into
npx quota-check codex             # OpenAI Codex
npx quota-check claude            # Claude Code (OAuth, Pro/Max)
npx quota-check kimi              # Kimi Code
```

Installing globally puts two commands on your PATH — `quota-check` and the
short alias `qc`:

```bash
npm i -g quota-check
qc all
```

Output is human-readable on a terminal and raw JSON when piped or redirected;
`--human` / `--json` force either one. Quota windows are always listed
shortest-first: the hourly window on top, the weekly one below.

```
  Codex quota
  ──────────────────────────────────────────────────────────
  you@example.com  ·  plus
  creds  /Users/you/.codex/auth.json

  5h    ██████████░░░░░░░░░░░░░░   41.0%  resets in 59m
  week  ██░░░░░░░░░░░░░░░░░░░░░░    8.0%  resets in 6d 13h
```

For `claude`, an expired stored access token is refreshed automatically (and
written back to the Keychain / credentials file, the way Claude Code does it);
`--no-refresh` opts out.

```js
// Programmatic API — local execution (reads credentials on this machine).
import { check, checkAll, checkHuman, whoami } from "quota-check";

const usage = await check("codex");            // raw JSON object
const all   = await checkAll();                // every provider in one call
const text  = await checkHuman("claude");      // human-readable string
const me    = await whoami("codex");           // account behind the credential

// provider-specific options:
await check("codex", { auth: "/path/to/auth.json" });
await check("claude", { token: "sk-ant-oat...", noRefresh: true });
await check("kimi", { key: "sk-...", base: "https://api.moonshot.cn/coding/v1" });
```

Supported providers: `codex`, `claude`, `kimi`.

Docs, source and issues: <https://github.com/Erchoc/quota-check> ·
[中文文档](https://github.com/Erchoc/quota-check/blob/main/README.zh-CN.md)
