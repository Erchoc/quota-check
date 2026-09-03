# quota-check — 项目说明（给 Claude 用）

在终端查询 Coding Agent 额度（小时窗口 / 周窗口）的 CLI。Rust 核心 +
npm 薄壳分发。中文回复，代码与注释用英文。

## 结构

- `crates/quota-check-core/`
  - `auth.rs` — Codex 凭据加载与 id_token 解析
  - `human.rs` — 通用渲染层：递归扫描百分比字段，**按窗口时长升序排序**
    （小时在前、周在后），不硬编码字段路径
  - `providers/{codex,claude,kimi}.rs` — 各家额度接口 + 凭据发现
- `crates/quota-check/` — CLI：`src/cli.rs` 是实现，`src/main.rs` 和
  `src/bin/qc.rs` 是两个一行的壳（对应 `quota-check` 与 `qc`）
- `npm/` — 主包（JS 启动器 + 编程式 API）与 5 个平台二进制包
- `research/` — 早期零依赖 Node 参考脚本（存档，勿当作实现依据）

## 约定

- 版本号必须在 `Cargo.toml` 和 6 个 `npm/*/package.json` 里保持一致，
  CI（`.github/workflows/ci.yml`）会校验，不一致直接失败。
- 默认输出：stdout 是 TTY 时人类可读，被管道 / 重定向时输出原始 JSON；
  `--human` / `--json` 可强制。
- 新增 provider：在 `providers/` 加一个模块实现取数，在 `cli.rs` 注册子命令，
  渲染层通常不用改。响应里没有百分比字段时，写一个 `normalize()`
  转成 `{used_percent, window_size_seconds, resets_in_seconds}` 的形状
  （参考 `kimi::normalize`）。
- 凭据处理三条硬规矩：只读已知条目（绝不 dump 整个 Keychain）、
  展示前必须 `human::mask()`、写回凭据时保留原文件其他字段。

## 当前进度

- [x] codex / claude / kimi 三个 provider，JSON + `--human` + `--whoami`
- [x] npm 平台二进制分发（optionalDependencies 模式）
- [x] `qc` 短别名：cargo 装两个 bin，npm 走 bin map
- [x] `all` 子命令：一次跑完所有 provider，单个失败不影响其他
- [x] 渲染层重做：窗口按时长排序、标签按字符宽度对齐、失败信息压成单行
- [x] Claude OAuth 自动刷新（refresh token 换新 access token 并写回
      Keychain / `.credentials.json`），`--no-refresh` 可关；429 限流短路
- [x] CI：fmt / clippy `-D warnings` / test 三平台 + npm 包与版本一致性校验
- [x] LICENSE（MIT）

未完成 / 下一步见 README 的 Roadmap：daemon 模式、cargo & brew 分发、
kimi 的 cc-switch SQLite 扫描、桌面端、JS API 云端执行。

## 已知坑

- Anthropic 的 `console.anthropic.com/v1/oauth/token` 限流很凶，
  短时间多次刷新会连续返回 429（连 `api.anthropic.com` 的 usage 也会被带上）。
  代码遇到 429 会停止本次运行的后续刷新尝试。
- Claude Code 桌面端在内存里刷新 token，Keychain 里那份常常是过期的——
  这正是自动刷新存在的理由。
