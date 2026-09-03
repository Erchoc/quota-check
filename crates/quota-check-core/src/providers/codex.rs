//! OpenAI Codex quota query.
//!
//! Note: wham/usage is a private endpoint, its shape can change at any time.
//! The raw JSON is passed through by default; the human renderer adapts to the
//! structure instead of hardcoding field paths.

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
        .header(
            "User-Agent",
            concat!("quota-check/", env!("CARGO_PKG_VERSION")),
        );
    if let Some(id) = &auth.account_id {
        req = req.header("ChatGPT-Account-Id", id);
    }

    let res = req.send().map_err(|e| anyhow!("request failed: {e}"))?;
    let status = res.status();
    let body = res
        .text()
        .map_err(|e| anyhow!("failed to read response: {e}"))?;

    if status.as_u16() == 401 || status.as_u16() == 403 {
        bail!(
            "HTTP {status} — token expired or account has no access.\n  run `codex login` to refresh {}\n  response: {}",
            auth.path.display(),
            &body[..body.len().min(300)]
        );
    }
    if !status.is_success() {
        bail!(
            "HTTP {status}\n  response: {}",
            &body[..body.len().min(500)]
        );
    }

    serde_json::from_str(&body).map_err(|_| {
        anyhow!(
            "response is not JSON (the API may have changed):\n{}",
            &body[..body.len().min(500)]
        )
    })
}
