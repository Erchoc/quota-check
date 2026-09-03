# quota-check

[English](./README.md)

在终端里查看 Coding Agent 的小时 / 周额度用量。Rust 核心，通过 npm、cargo、brew 分发预编译二进制。

```bash
# 免安装直接跑
npx quota-check all              # 所有已登录的 provider
npx quota-check codex            # 单个 provider
npx quota-check codex --whoami   # 这份凭据是谁

# npm i -g quota-check（或 cargo install quota-check）之后
qc all
qc claude
```

安装后会同时提供两个命令：`quota-check` 和短别名 `qc`。

```
  Codex quota
  ──────────────────────────────────────────────────────────
  you@example.com  ·  plus
  creds  /Users/you/.codex/auth.json

  5h    ██████████░░░░░░░░░░░░░░   41.0%  resets in 59m
  week  ██░░░░░░░░░░░░░░░░░░░░░░    8.0%  resets in 6d 13h
```

无论接口按什么顺序返回，渲染层都按窗口长度从短到长排序——小时窗口在上，周额度在下。

### 输出格式

在终端里默认输出人类可读视图，被管道 / 重定向时默认输出原始 JSON。
需要强制时用 `--human` / `--json`：

```bash
qc codex                  # 进度条
qc codex | jq .           # JSON，不用加参数
qc codex --json           # 终端里也强制 JSON
```

```js
// 编程式 API（当前为本机执行模式，读取本机凭据；
// 未来云端托管上线后，同一套 API 切到云端执行，签名不变）
import { check, checkAll, checkHuman, whoami } from "quota-check";

const usage = await check("codex");       // 原始 JSON 对象
const all   = await checkAll();           // { codex: {...}, claude: {...}, kimi: {...} }
const text  = await checkHuman("claude"); // 人类可读字符串
const me    = await whoami("codex");      // 凭据对应的账号
```

## Provider 现状

| Provider | 状态 | 数据来源 |
|---|---|---|
| `codex` | ✅ | `~/.codex/auth.json` → `wham/usage`（私有接口） |
| `claude` | ✅ | Keychain / `~/.claude/.credentials.json` → OAuth usage（仅 Pro/Max 订阅） |
| `kimi` | ✅ | env / kimi-code / claude settings / pi → `api.kimi.com/coding/v1/usages` |
| `gemini` | ❌ | 没有可查额度的接口（本地会话聚合方案已搁置） |

各家额度接口均为私有 / 未文档化，可能随时变动。human 渲染层做了结构自适应
（递归扫描百分比字段，不硬编码路径），接口微调时尽量不受影响。

## Claude 的 token 会过期，quota-check 会自动续

Claude Code 存的 access token 只有几个小时寿命，旁边配一个长期有效的
refresh token。Claude Code 一停，存储里那份 access token 很快就过期了——
直接读出来用，多半会撞上 `401 OAuth access token has expired`。

所以 `qc claude` 做的事和 Claude Code 一样：发现存储里的 token 已过
`expiresAt`，就用 refresh token 换一份新的，并**写回原来的存储**
（Keychain 条目或 `.credentials.json`），避免轮换后的 refresh token 丢失。
文件里其他字段（MCP OAuth 条目、`subscriptionType`、scopes）原样保留。

- `--no-refresh` 关掉这套逻辑，直接拿存储里的 token 去探测。
- 失败原因会分类：`expired` / `revoked` / `invalid` / `rate_limited`，各给各的处理建议。
- token 端点限流很凶。一旦返回 `429`，本次运行不再重试，并提示你稍后再试或先跑一次 `claude`。

## 仓库结构

```
crates/
  quota-check-core/   # provider 插件、凭据加载、human 渲染
  quota-check/        # CLI（clap）：一个 lib + 两个薄二进制（quota-check、qc）
npm/
  quota-check/               # 主包：JS 启动器 + 编程式 API
  quota-check-<platform>/    # 各平台二进制包（optionalDependencies）
research/             # 零依赖 Node.js 参考脚本（存档）
.github/workflows/    # ci.yml（fmt/clippy/test）· release.yml（v* tag 发版）
```

## npm 包是怎么组织的

每个版本会为每个平台发布一个只含预编译 Rust 二进制的包
（`quota-check-darwin-arm64` 等），主包 `quota-check` 通过
`optionalDependencies` 按平台拉取对应二进制（与 esbuild / Biome / swc 相同的模式）。
JS 的 bin 入口（`quota-check` 与 `qc`）只是定位二进制并转发参数的薄启动器——
没有 postinstall 下载，兼容 npm 对安装脚本的限制。

## 开发

```bash
cargo build
./target/debug/qc codex
cargo test
cargo clippy --all-targets -- -D warnings
```

发版：先把 `Cargo.toml` 和 6 个 `npm/*/package.json` 的版本号一起改掉
（CI 会校验版本一致性），推 `v*` tag → GitHub Actions 构建 5 平台二进制并传到
Release → 发布 npm 包（先 `npm/quota-check-*` 平台包，再 `npm/quota-check` 主包）。

## Roadmap

- [x] codex / claude / kimi provider，JSON + `--human` + `--whoami`
- [x] npm 平台二进制分发
- [x] `qc` 短别名、`all` 子命令、按 TTY 自适应的默认输出
- [x] Claude OAuth 自动刷新并写回存储
- [ ] `quota-check daemon`：定时轮询、Webhook 告警、Reset 前自动执行预设任务、定时唤醒 Agent
- [ ] cargo / brew 分发
- [ ] kimi 的 cc-switch SQLite 扫描
- [ ] 桌面端（Tauri）/ Web / H5（订阅制）
- [ ] JS API 云端托管执行模式

## License

MIT，见 [LICENSE](./LICENSE)。
