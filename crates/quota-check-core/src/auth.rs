//! 读取本地 Coding Agent 的凭据文件（目前支持 ~/.codex/auth.json）。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

/// 一份可用的访问凭据。
pub struct Auth {
    pub access_token: String,
    pub account_id: Option<String>,
    pub id_token: Option<String>,
    pub path: PathBuf,
}

/// 从 id_token (JWT) 里解出的账号信息，仅用于展示，不做验签。
pub struct Identity {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<String>,
}

/// Codex 默认凭据路径：$CODEX_HOME/auth.json 或 ~/.codex/auth.json
pub fn default_codex_auth_path() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return PathBuf::from(home).join("auth.json");
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex")
        .join("auth.json")
}

/// 加载 auth.json。兼容新版 `{ tokens: {...} }` 和平铺两种结构。
pub fn load_auth(path: &Path) -> Result<Auth> {
    let raw = fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow!(
                "读不到凭据文件：{}\n  文件不存在，先跑一次 codex login",
                path.display()
            )
        } else {
            anyhow!("读不到凭据文件：{}\n  {e}", path.display())
        }
    })?;

    let json: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|_| anyhow!("凭据文件不是合法 JSON：{}", path.display()))?;

    let t = json.get("tokens").unwrap_or(&json);
    let get = |snake: &str, camel: &str| -> Option<String> {
        t.get(snake)
            .or_else(|| t.get(camel))
            .and_then(|v| v.as_str())
            .map(String::from)
    };

    let access_token = get("access_token", "accessToken")
        .ok_or_else(|| anyhow!("凭据里没有 access_token：{}", path.display()))?;

    Ok(Auth {
        access_token,
        account_id: get("account_id", "accountId"),
        id_token: get("id_token", "idToken"),
        path: path.to_path_buf(),
    })
}

/// 解开 id_token 的 payload，确认这份凭据属于哪个账号。解析失败不视为错误。
pub fn identity(id_token: Option<&str>) -> Option<Identity> {
    let payload = id_token?.split('.').nth(1)?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;

    let auth = &json["https://api.openai.com/auth"];
    let s = |v: &serde_json::Value| v.as_str().map(String::from);
    let expires_at = json["exp"].as_i64().and_then(|exp| {
        chrono::DateTime::from_timestamp(exp, 0).map(|t| t.to_rfc3339())
    });

    Some(Identity {
        email: s(&json["email"]),
        plan: s(&auth["chatgpt_plan_type"]),
        account_id: s(&auth["chatgpt_account_id"]),
        expires_at,
    })
}
