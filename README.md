# quota-check

查看 Coding Agent 的小时 / 周额度用量。Rust 核心 + 多形态分发。

```bash
# CLI（npx 免安装）
npx quota-check codex            # 原始 JSON
npx quota-check codex --human    # 人类可读
npx quota-check codex --whoami   # 这份凭据是谁

# 其他安装方式（规划）
cargo install quota-check
brew install quota-check
```

```js
// 编程式 API（当前为本机执行模式，读取本机凭据如 ~/.codex/auth.json；
// 未来云端托管上线后，同一套 API 改为云端环境触发，签名不变）
import { check, checkHuman, whoami } from "quota-check";

const usage = await check("codex");       // 原始 JSON 对象
const text  = await checkHuman("codex");  // 人类可读字符串
const me    = await whoami("codex");      // 凭据对应的账号
```

## 仓库结构

```
crates/
  quota-check-core/   # provider 插件、凭据加载、额度归一化、human 渲染
  quota-check/        # CLI 二进制（clap）
npm/quota-check/      # npm 薄壳：postinstall 下载平台二进制 + JS API
.github/workflows/    # tag 触发多平台 Release 构建
```

## 开发

```bash
cargo build
./target/debug/quota-check codex --human
```

发版：打 `v*` tag → GitHub Actions 构建 5 平台二进制并传到 Release →
`cd npm/quota-check && npm publish`（postinstall 会按版本号下载对应二进制）。

## Provider 现状

| Provider | 状态 | 数据来源 |
|---|---|---|
| codex | ✅ | `~/.codex/auth.json` → `wham/usage`（私有接口） |
| claude | 规划 | OAuth usage 端点 |
| gemini | 规划 | 待定 |

注意：各家额度接口均为私有 / 未文档化，human 渲染层做了结构自适应
（递归扫描百分比字段，不硬编码路径），接口微调时尽量仍能渲染。

## Roadmap

- [x] `codex` provider + JSON / --human / --whoami
- [ ] npm 发布（`npx quota-check`）
- [ ] claude / gemini provider
- [ ] `quota-check daemon`：定时轮询、Webhook 告警、Reset 前自动执行预设任务、定时唤醒 Agent
- [ ] cargo / brew 分发
- [ ] 桌面端（Tauri）/ Web / H5（订阅制）
- [ ] 云端托管执行模式（JS API 切云端触发）
