# quota-check

Check Coding Agent quota usage (5-hour / weekly windows) from the terminal.
Prebuilt Rust binary, zero runtime dependencies.

```bash
npx quota-check codex --human     # OpenAI Codex
npx quota-check claude --human    # Claude Code (OAuth, Pro/Max)
npx quota-check kimi --human      # Kimi Code
```

Default output is raw JSON; `--human` renders a readable view with progress
bars, reset countdowns and credential source.

```js
// Programmatic API — local execution (reads credentials on this machine).
import { check, checkHuman, whoami } from "quota-check";

const usage = await check("codex");            // raw JSON object
const text  = await checkHuman("claude");      // human-readable string
const me    = await whoami("codex");           // account behind the credential

// provider-specific options:
await check("codex", { auth: "/path/to/auth.json" });
await check("claude", { token: "sk-ant-oat..." });
await check("kimi", { key: "sk-...", base: "https://api.moonshot.cn/coding/v1" });
```

Supported providers: `codex`, `claude`, `kimi`.

Docs, source and issues: <https://github.com/erchoc/quota-check> ·
[中文文档](https://github.com/erchoc/quota-check/blob/main/README.zh-CN.md)
