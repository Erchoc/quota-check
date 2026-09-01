//! quota-check 核心库：provider 插件、凭据加载、额度归一化与人类可读渲染。
//!
//! 后续新增 provider（claude / gemini / ...）时：
//! 1. 在 `providers/` 下加一个模块，实现和 `codex` 相同的 `fetch_usage(auth) -> Value`
//! 2. 在 CLI 里注册子命令即可，human 渲染层无需改动（它是结构自适应的）。

pub mod auth;
pub mod human;
pub mod providers;
