# quota-check

[English](./README.md)

在终端里查看 Coding Agent 的小时 / 周额度用量。Rust 核心，通过 npm、cargo、brew 分发预编译二进制。

```bash
# 免安装直接跑
npx quota-check codex            # 原始 JSON
npx quota-check codex --human    # 人类可读
npx quota-check codex --whoami   # 这份凭据是谁

npx quota-check claude --human
npx quota-check kimi --human
```

```js
// 编程式 API（当前为本机执行模式，读取本机凭据；
// 未来云端托管上线后，同一套 API 切到云端执行，签名不变）
import { check, checkHuman, whoami } from "quota-check";

const usage = await check("codex");       // 原始 JSON 对象
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

## npm 包是怎么组织的

每个版本会为每个平台发布一个只含预编译 Rust 二进制的包
（`quota-check-darwin-arm64` 等），主包 `quota-check` 通过
`optionalDependencies` 按平台拉取对应二进制（与 esbuild / Biome / swc 相同的模式）。
JS 的 bin 入口只是一个定位二进制并转发参数的薄启动器——没有 postinstall 下载，
兼容 npm 对安装脚本的限制。

## 开发

```bash
cargo build
./target/debug/quota-check codex --human
cargo test
```

发版：推 `v*` tag → GitHub Actions 构建 5 平台二进制并传到 Release →
发布 npm 包（先 `npm/quota-check-*` 平台包，再 `npm/quota-check` 主包）。

## Roadmap

- [x] codex / claude / kimi provider，JSON + `--human` + `--whoami`
- [x] npm 平台二进制分发
- [ ] `quota-check daemon`：定时轮询、Webhook 告警、Reset 前自动执行预设任务、定时唤醒 Agent
- [ ] cargo / brew 分发
- [ ] kimi 的 cc-switch SQLite 扫描
- [ ] 桌面端（Tauri）/ Web / H5（订阅制）
- [ ] JS API 云端托管执行模式
