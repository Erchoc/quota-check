# quota-check

[中文文档](./README.zh-CN.md)

Check Coding Agent quota usage (5-hour / weekly windows) from the terminal.
Rust core, shipped as prebuilt binaries through npm, cargo and brew.

```bash
# no install needed
npx quota-check codex            # raw JSON
npx quota-check codex --human    # human-readable
npx quota-check codex --whoami   # which account is this credential

npx quota-check claude --human
npx quota-check kimi --human
```

```js
// Programmatic API (currently LOCAL execution — reads credential files on the
// machine it runs on. When the cloud-hosted mode ships, the same API switches
// to cloud execution with unchanged signatures.)
import { check, checkHuman, whoami } from "quota-check";

const usage = await check("codex");       // raw JSON object
const text  = await checkHuman("claude"); // human-readable string
const me    = await whoami("codex");      // account behind the credential
```

## Providers

| Provider | Status | Data source |
|---|---|---|
| `codex` | ✅ | `~/.codex/auth.json` → `chatgpt.com/backend-api/wham/usage` (private endpoint) |
| `claude` | ✅ | Keychain / `~/.claude/.credentials.json` → `api.anthropic.com/api/oauth/usage` (OAuth, Pro/Max only) |
| `kimi` | ✅ | env / kimi-code / claude settings / pi → `api.kimi.com/coding/v1/usages` |
| `gemini` | ❌ | No quota API exists (local session aggregation dropped for now) |

All quota endpoints are private/undocumented and may change without notice.
The human renderer adapts to the response structure (recursive scan for
percentage fields) instead of hardcoding field paths, so minor API changes
don't break it.

## Repository layout

```
crates/
  quota-check-core/   # provider plugins, credential loading, human renderer
  quota-check/        # the CLI binary (clap)
npm/
  quota-check/               # main npm package: JS launcher + programmatic API
  quota-check-<platform>/    # per-platform binary packages (optionalDependencies)
research/             # zero-dependency Node.js reference scripts (archived)
.github/workflows/    # multi-platform release builds on v* tags
```

## How the npm package works

Each release publishes one package per platform (`quota-check-darwin-arm64`,
...) containing the prebuilt Rust binary, plus the main `quota-check` package
which pulls the right one via `optionalDependencies` (the same pattern as
esbuild / Biome / swc). The JS `bin` entry is a thin launcher that locates the
binary and forwards arguments — no postinstall downloads, works with npm's
install-script restrictions.

## Development

```bash
cargo build
./target/debug/quota-check codex --human
cargo test
```

Release: push a `v*` tag → GitHub Actions builds 5 platform binaries and
attaches them to the GitHub Release → publish the npm packages
(`npm/quota-check-*` first, then `npm/quota-check`).

## Roadmap

- [x] codex / claude / kimi providers, JSON + `--human` + `--whoami`
- [x] npm distribution with per-platform binary packages
- [ ] `quota-check daemon`: scheduled polling, webhook alerts, run preset
      tasks before reset, wake agents on a schedule
- [ ] cargo / brew distribution
- [ ] cc-switch SQLite scan for kimi keys
- [ ] Desktop (Tauri) / Web / H5 (subscription)
- [ ] Cloud-hosted execution mode for the JS API
