//! Claude Code subscription quota (5h / 7d windows).
//!
//! The endpoint only makes sense for Pro/Max subscriptions (OAuth login).
//! API-key billing, or setups redirected to a third party (cc-switch etc.),
//! have no subscription quota to query.
//!
//! Credential priority (mirrors Claude Code's own storage strategy):
//!   1. --token argument
//!   2. CLAUDE_CODE_OAUTH_TOKEN env var
//!   3. macOS Keychain (primary store on macOS)
//!   4. ~/.claude/.credentials.json (fallback when Keychain write fails;
//!      primary store on Linux/Windows)

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::Value;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// One credential candidate with a human-readable origin.
pub struct Candidate {
    pub token: String,
    pub source: String,
}

/// Anthropic OAuth tokens carry a fixed prefix; use it to reject unrelated secrets.
pub fn is_anthropic_token(t: &str) -> bool {
    let t = t.trim();
    t.starts_with("sk-ant-oat") || t.starts_with("sk-ant-ort")
}

pub fn claude_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    dirs::home_dir().unwrap_or_default().join(".claude")
}

pub fn credentials_path() -> PathBuf {
    claude_dir().join(".credentials.json")
}

/// Read only the well-known Claude Code keychain entry. Never dump the whole
/// keychain: it can match unrelated entries (e.g. "Claude Safe Storage", the
/// desktop app's Electron safeStorage key) and may trigger auth prompts.
fn read_keychain() -> Vec<Candidate> {
    if !cfg!(target_os = "macos") {
        return vec![];
    }
    let Ok(out) = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
    else {
        return vec![];
    };
    if !out.status.success() {
        return vec![];
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return vec![];
    }
    // The entry is usually a JSON blob; some setups store a bare token.
    let token = serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|j| extract_oauth_token(&j))
        .unwrap_or(raw);
    if !is_anthropic_token(&token) {
        return vec![];
    }
    vec![Candidate {
        token: token.trim().into(),
        source: format!("Keychain: {KEYCHAIN_SERVICE}"),
    }]
}

/// Pull the access token out of the various credential JSON shapes.
fn extract_oauth_token(j: &Value) -> Option<String> {
    let o = j
        .get("claudeAiOauth")
        .or_else(|| j.get("claudeAi"))
        .or_else(|| j.get("oauth"))
        .unwrap_or(j);
    o.get("accessToken")
        .or_else(|| o.get("access_token"))
        .or_else(|| o.get("access"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn read_credentials_file() -> Vec<Candidate> {
    let path = credentials_path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    let Ok(j) = serde_json::from_str::<Value>(&raw) else {
        return vec![];
    };
    let Some(t) = extract_oauth_token(&j) else {
        return vec![];
    };
    if !is_anthropic_token(&t) {
        return vec![];
    }
    vec![Candidate {
        token: t.trim().into(),
        source: path.display().to_string(),
    }]
}

/// Collect candidates in priority order, deduplicated by token value.
pub fn collect_candidates(arg_token: Option<&str>) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut push = |token: Option<&str>, source: String| {
        let Some(t) = token else { return };
        let t = t.trim();
        if t.len() < 20 || seen.contains(t) {
            return;
        }
        seen.insert(t.to_string());
        out.push(Candidate {
            token: t.into(),
            source,
        });
    };

    push(arg_token, "--token arg".into());
    push(
        std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok().as_deref(),
        "env CLAUDE_CODE_OAUTH_TOKEN".into(),
    );
    for c in read_keychain() {
        push(Some(&c.token), c.source);
    }
    for c in read_credentials_file() {
        push(Some(&c.token), c.source);
    }
    out
}

/// Classify server-side failures: revoked / expired / invalid need very
/// different remediation.
pub fn classify(status: u16, body: &str) -> &'static str {
    let s = body.to_lowercase();
    if s.contains("revoked") {
        "revoked"
    } else if s.contains("expired") {
        "expired"
    } else if status == 401 || status == 403 {
        "invalid"
    } else if status == 429 {
        "rate_limited"
    } else {
        "error"
    }
}

pub struct Probe {
    pub ok: bool,
    pub status: u16,
    pub reason: &'static str,
    pub note: String,
    pub data: Option<Value>,
}

pub fn probe(token: &str) -> Probe {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Probe {
                ok: false,
                status: 0,
                reason: "network",
                note: e.to_string(),
                data: None,
            }
        }
    };

    // The claude-code User-Agent is required; the endpoint rejects other UAs.
    let res = client
        .get(ENDPOINT)
        .bearer_auth(token)
        .header("anthropic-beta", OAUTH_BETA)
        .header("Accept", "application/json")
        .header("User-Agent", "claude-code/2.1.0 (quota-check)")
        .send();

    let res = match res {
        Ok(r) => r,
        Err(e) => {
            return Probe {
                ok: false,
                status: 0,
                reason: "network",
                note: e.to_string(),
                data: None,
            }
        }
    };
    let status = res.status().as_u16();
    let body = res.text().unwrap_or_default();

    if !res_ok(status) {
        return Probe {
            ok: false,
            status,
            reason: classify(status, &body),
            note: body.chars().take(160).collect(),
            data: None,
        };
    }
    match serde_json::from_str::<Value>(&body) {
        Ok(data) => Probe {
            ok: true,
            status,
            reason: "ok",
            note: String::new(),
            data: Some(data),
        },
        Err(_) => Probe {
            ok: false,
            status,
            reason: "error",
            note: "response is not JSON".into(),
            data: None,
        },
    }
}

fn res_ok(status: u16) -> bool {
    (200..300).contains(&status)
}

/// Probe candidates in priority order; first 200 wins.
/// Returns the usage document plus the winning candidate.
pub fn fetch_usage(candidates: &[Candidate]) -> Result<(Value, Candidate)> {
    let mut failures = vec![];
    for c in candidates {
        // Candidates from env/arg are not prefix-filtered at collection time;
        // skip obvious non-Anthropic shapes before hitting the network.
        if !is_anthropic_token(&c.token) {
            failures.push(format!(
                "  skip [{}] {} — not an Anthropic OAuth token",
                c.source,
                crate::human::mask(&c.token)
            ));
            continue;
        }
        let r = probe(&c.token);
        if r.ok {
            return Ok((
                r.data.unwrap(),
                Candidate {
                    token: c.token.clone(),
                    source: c.source.clone(),
                },
            ));
        }
        failures.push(format!(
            "  {:<4} [{}] {} — {}",
            r.status, r.reason, c.source, r.note
        ));
    }
    bail!(
        "all {} credential candidates failed:\n{}\n\n  revoked → credential was invalidated, log in again and clean up leftovers\n  expired → run `claude update` or re-login to trigger a refresh\n  invalid → token format or account problem",
        candidates.len(),
        failures.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn token_prefix_check() {
        assert!(is_anthropic_token("sk-ant-oat01-xxxxxxxxxxxxxxxxxxxx"));
        assert!(is_anthropic_token("  sk-ant-ort01-xxxxxxxx  "));
        assert!(!is_anthropic_token("sk-ant-api03-xxxxxxxx"));
        assert!(!is_anthropic_token("sk-xxxxxxxxxxxx"));
        assert!(!is_anthropic_token("Claude Safe Storage"));
    }

    #[test]
    fn classify_failures() {
        assert_eq!(classify(401, "{}"), "invalid");
        assert_eq!(classify(403, "token revoked"), "revoked");
        assert_eq!(classify(401, "OAuth token expired"), "expired");
        assert_eq!(classify(429, "slow down"), "rate_limited");
        assert_eq!(classify(500, "boom"), "error");
    }

    #[test]
    fn extracts_token_from_json_shapes() {
        let a = json!({"claudeAiOauth": {"accessToken": "sk-ant-oat01-aaaaaaaaaaaaaaaaaaaa"}});
        assert!(extract_oauth_token(&a).is_some());
        let b = json!({"access_token": "sk-ant-oat01-bbbbbbbbbbbbbbbbbbbb"});
        assert!(extract_oauth_token(&b).is_some());
        let c = json!({"unrelated": true});
        assert!(extract_oauth_token(&c).is_none());
    }

    #[test]
    fn collects_arg_and_env_first() {
        // SAFETY: tests run in a single process; env mutation is serialized here.
        unsafe {
            std::env::set_var(
                "CLAUDE_CODE_OAUTH_TOKEN",
                "sk-ant-oat01-env-env-env-env-env-env",
            );
        }
        let cs = collect_candidates(Some("sk-ant-oat01-arg-arg-arg-arg-arg-arg"));
        assert_eq!(cs[0].source, "--token arg");
        assert_eq!(cs[1].source, "env CLAUDE_CODE_OAUTH_TOKEN");
        // dedupe: same token twice collapses to one
        let cs2 = collect_candidates(Some("sk-ant-oat01-env-env-env-env-env-env"));
        assert_eq!(cs2.len(), 1);
        unsafe {
            std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        }
    }
}
