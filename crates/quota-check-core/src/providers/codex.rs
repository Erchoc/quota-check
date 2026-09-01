//! OpenAI Codex 额度查询。
//!
//! 注意：wham/usage 是私有接口，结构可能随时变动。
//! 默认原样透传 JSON；human 渲染层做结构自适应，不硬编码字段路径。

use std::time::Duration;

use anyhow::{anyhow, bail, Result};

use crate::auth::Auth;

const ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

pub fn fetch_usage(auth: &Auth) -> Result<serde_json::Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let mut req = client
        .get(ENDPOINT)
        .bearer_auth(&auth.access_token)
        .header("Accept", "application/json")
        .header("User-Agent", concat!("quota-check/", env!("CARGO_PKG_VERSION")));
    if let Some(id) = &auth.account_id {
        req = req.header("ChatGPT-Account-Id", id);
    }

    let res = req.send().map_err(|e| anyhow!("请求失败：{e}"))?;
    let status = res.status();
    let body = res.text().map_err(|e| anyhow!("读取响应失败：{e}"))?;

    if status.as_u16() == 401 || status.as_u16() == 403 {
        bail!(
            "HTTP {status} — token 已过期或账号无权访问。\n  跑一次 codex login 刷新 {}\n  响应：{}",
            auth.path.display(),
            &body[..body.len().min(300)]
        );
    }
    if !status.is_success() {
        bail!("HTTP {status}\n  响应：{}", &body[..body.len().min(500)]);
    }

    serde_json::from_str(&body)
        .map_err(|_| anyhow!("返回的不是 JSON（接口可能变了）：\n{}", &body[..body.len().min(500)]))
}
