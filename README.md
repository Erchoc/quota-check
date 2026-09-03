# quota-check

[中文文档](./README.zh-CN.md)

Check Coding Agent quota usage (5-hour / weekly windows) from the terminal.
Rust core, shipped as prebuilt binaries through npm, cargo and brew.

```bash
# no install needed
npx quota-check all              # every provider you are logged into
npx quota-check codex            # one provider
npx quota-check codex --whoami   # which account is this credential

# after `npm i -g quota-check` (or `cargo install quota-check`)
qc all
qc claude
```

Both names are installed: `quota-check` and the short alias `qc`.

```
  Codex quota
  ──────────────────────────────────────────────────────────
  you@example.com  ·  plus
  creds  /Users/you/.codex/auth.json

  5h    ██████████░░░░░░░░░░░░░░   41.0%  resets in 59m
  week  ██░░░░░░░░░░░░░░░░░░░░░░    8.0%  resets in 6d 13h
```

Windows are always ordered shortest-first — the hourly window on top, weekly
below — for every provider, regardless of the order the API returns them in.

### Output format

Human-readable on a terminal, raw JSON when the output is piped or
redirected. Force either one with `--human` / `--json`:

```bash
qc codex                  # bars
qc codex | jq .           # JSON, no flag needed
qc codex --json           # JSON on a terminal too
```

```js
// Programmatic API (currently LOCAL execution — reads credential files on the
// machine it runs on. When the cloud-hosted mode ships, the same API switches
// to cloud execution with unchanged signatures.)
import { check, checkAll, checkHuman, whoami } from "quota-check";

const usage = await check("codex");       // raw JSON object
const all   = await checkAll();           // { codex: {...}, claude: {...}, kimi: {...} }
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

## Claude tokens expire — quota-check refreshes them

Claude Code stores a short-lived access token (hours) next to a long-lived
refresh token. The stored access token goes stale as soon as Claude Code stops
running, so reading the store directly usually yields an expired token and a
`401 OAuth access token has expired`.

`quota-check claude` therefore does what Claude Code does: when the stored
token is past its `expiresAt`, it exchanges the refresh token for a new access
token and **writes the result back to the same store** (Keychain entry or
`.credentials.json`), so the rotated refresh token is not lost. Other keys in
the file — MCP OAuth entries, `subscriptionType`, scopes — are preserved.

- `--no-refresh` skips all of that and probes the stored token as-is.
- Failures are classified: `expired` / `revoked` / `invalid` / `rate_limited`,
  each with its own remediation line.
- The token endpoint throttles aggressively. On a `429`, quota-check stops
  retrying for the rest of the run and tells you to wait or run `claude` once.

## Repository layout

```
crates/
  quota-check-core/   # provider plugins, credential loading, human renderer
  quota-check/        # the CLI (clap); one lib + two thin bins (quota-check, qc)
npm/
  quota-check/               # main npm package: JS launcher + programmatic API
  quota-check-<platform>/    # per-platform binary packages (optionalDependencies)
research/             # zero-dependency Node.js reference scripts (archived)
.github/workflows/    # ci.yml (fmt/clippy/test) · release.yml (v* tags)
```

## How the npm package works

Each release publishes one package per platform (`quota-check-darwin-arm64`,
...) containing the prebuilt Rust binary, plus the main `quota-check` package
which pulls the right one via `optionalDependencies` (the same pattern as
esbuild / Biome / swc). The JS `bin` entries (`quota-check` and `qc`) are thin
launchers that locate the binary and forward arguments — no postinstall
downloads, works with npm's install-script restrictions.

## Development

```bash
cargo build
./target/debug/qc codex
cargo test
cargo clippy --all-targets -- -D warnings
```

Release: bump the version in `Cargo.toml` **and** all six `npm/*/package.json`
files (CI fails on drift), push a `v*` tag → GitHub Actions builds 5 platform
binaries and attaches them to the GitHub Release → publish the npm packages
(`npm/quota-check-*` first, then `npm/quota-check`).

## Roadmap

- [x] codex / claude / kimi providers, JSON + `--human` + `--whoami`
- [x] npm distribution with per-platform binary packages
- [x] `qc` short alias, `all` subcommand, TTY-aware default output
- [x] Claude OAuth auto-refresh with write-back
- [ ] `quota-check daemon`: scheduled polling, webhook alerts, run preset
      tasks before reset, wake agents on a schedule
- [ ] cargo / brew distribution
- [ ] cc-switch SQLite scan for kimi keys
- [ ] Desktop (Tauri) / Web / H5 (subscription)
- [ ] Cloud-hosted execution mode for the JS API

## License

MIT — see [LICENSE](./LICENSE).
