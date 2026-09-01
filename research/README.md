# research

Zero-dependency reference implementations (plain Node.js, no packages) used to
research each provider's private quota endpoints and local credential layouts.

These scripts are **archived for reference only** — the shipping implementation
is the Rust workspace at the repo root. They may lag behind; check the Rust
providers under `crates/quota-check-core/src/providers/` for current behavior.

| Script | Notes |
|---|---|
| `quota-check-codex.js` | `chatgpt.com/backend-api/wham/usage` + `~/.codex/auth.json` |
| `quota-check-claude.js` | `api.anthropic.com/api/oauth/usage`, Keychain / `.credentials.json` |
| `quota-check-gemini.js` | No quota API exists — aggregates local session files (estimate) |
| `quota-check-kimi.js` | `api.kimi.com/coding/v1/usages`, multi-source key probing |
