//! Load local Coding Agent credential files (currently ~/.codex/auth.json).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

/// A usable access credential.
pub struct Auth {
    pub access_token: String,
    pub account_id: Option<String>,
    pub id_token: Option<String>,
    pub path: PathBuf,
}

/// Account info decoded from the id_token (JWT). Display-only, not verified.
pub struct Identity {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<String>,
}

/// Codex default credential path: $CODEX_HOME/auth.json or ~/.codex/auth.json
pub fn default_codex_auth_path() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return PathBuf::from(home).join("auth.json");
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex")
        .join("auth.json")
}

/// Load auth.json. Handles both the new `{ tokens: {...} }` shape and flat layouts.
pub fn load_auth(path: &Path) -> Result<Auth> {
    let raw = fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow!(
                "cannot read credentials file: {}\n  file does not exist, run `codex login` first",
                path.display()
            )
        } else {
            anyhow!("cannot read credentials file: {}\n  {e}", path.display())
        }
    })?;

    let json: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|_| anyhow!("credentials file is not valid JSON: {}", path.display()))?;

    let t = json.get("tokens").unwrap_or(&json);
    let get = |snake: &str, camel: &str| -> Option<String> {
        t.get(snake)
            .or_else(|| t.get(camel))
            .and_then(|v| v.as_str())
            .map(String::from)
    };

    let access_token = get("access_token", "accessToken")
        .ok_or_else(|| anyhow!("no access_token in credentials: {}", path.display()))?;

    Ok(Auth {
        access_token,
        account_id: get("account_id", "accountId"),
        id_token: get("id_token", "idToken"),
        path: path.to_path_buf(),
    })
}

/// Decode the id_token payload to see which account this credential belongs to.
/// Parse failure is not an error.
pub fn identity(id_token: Option<&str>) -> Option<Identity> {
    let payload = id_token?.split('.').nth(1)?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;

    let auth = &json["https://api.openai.com/auth"];
    let s = |v: &serde_json::Value| v.as_str().map(String::from);
    let expires_at = json["exp"]
        .as_i64()
        .and_then(|exp| chrono::DateTime::from_timestamp(exp, 0).map(|t| t.to_rfc3339()));

    Some(Identity {
        email: s(&json["email"]),
        plan: s(&auth["chatgpt_plan_type"]),
        account_id: s(&auth["chatgpt_account_id"]),
        expires_at,
    })
}
