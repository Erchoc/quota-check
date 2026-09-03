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
//!
//! Access tokens are short-lived (hours). Claude Code refreshes them in the
//! background, but a stored token goes stale as soon as the app stops running,
//! so a plain read of the store frequently yields an expired token. We do what
//! Claude Code does: exchange the stored refresh token for a fresh access
//! token and write the result back to the same store, keeping the rotated
//! refresh token from being lost. `--no-refresh` opts out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::human::one_line;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_TOKEN_ENDPOINT: &str = "https://console.anthropic.com/v1/oauth/token";
/// Claude Code's public OAuth client id (not a secret; it ships in the CLI).
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
/// Treat a token as expired this long before its stated expiry.
const EXPIRY_SKEW_MS: i64 = 60_000;

/// Where a credential came from — determines where a refreshed token is
/// written back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Arg,
    Env,
    Keychain { account: String },
    File(PathBuf),
}

impl Origin {
    /// Whether a refreshed credential can be persisted back here.
    pub fn is_writable(&self) -> bool {
        matches!(self, Origin::Keychain { .. } | Origin::File(_))
    }
}

/// One credential candidate with a human-readable origin.
pub struct Candidate {
    pub token: String,
    pub source: String,
    pub refresh_token: Option<String>,
    /// Epoch milliseconds, as recorded by Claude Code.
    pub expires_at: Option<i64>,
    pub origin: Origin,
}

impl Candidate {
    fn bare(token: String, source: String, origin: Origin) -> Self {
        Candidate {
            token,
            source,
            refresh_token: None,
            expires_at: None,
            origin,
        }
    }

    /// Locally known to be expired. `None` (no recorded expiry) is not expired.
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(ms) => ms - EXPIRY_SKEW_MS < now_ms(),
            None => false,
        }
    }

    /// "expired 16h ago" / "valid for 3h 2m" — empty when no expiry is recorded.
    pub fn expiry_note(&self) -> String {
        let Some(ms) = self.expires_at else {
            return String::new();
        };
        let delta = (ms - now_ms()) as f64 / 1000.0;
        if delta < 0.0 {
            format!("local token expired {} ago", fmt_span(-delta))
        } else {
            format!("local token valid for {}", fmt_span(delta))
        }
    }

    fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn fmt_span(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
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

/// Epoch value in seconds or milliseconds → milliseconds.
fn as_epoch_ms(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    let n = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))?;
    if n <= 0.0 {
        return None;
    }
    Some(if n < 1e11 {
        (n * 1000.0) as i64
    } else {
        n as i64
    })
}

/// The credential blob nests the OAuth fields under one of a few keys.
fn oauth_container(j: &Value) -> &Value {
    j.get("claudeAiOauth")
        .or_else(|| j.get("claudeAi"))
        .or_else(|| j.get("oauth"))
        .unwrap_or(j)
}

fn container_key(j: &Value) -> Option<&'static str> {
    ["claudeAiOauth", "claudeAi", "oauth"]
        .into_iter()
        .find(|k| j.get(*k).is_some())
}

/// Pull the access token out of the various credential JSON shapes.
fn extract_oauth_token(j: &Value) -> Option<String> {
    let o = oauth_container(j);
    o.get("accessToken")
        .or_else(|| o.get("access_token"))
        .or_else(|| o.get("access"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn extract_refresh_token(j: &Value) -> Option<String> {
    let o = oauth_container(j);
    o.get("refreshToken")
        .or_else(|| o.get("refresh_token"))
        .and_then(|v| v.as_str())
        .filter(|s| s.len() > 12)
        .map(String::from)
}

fn extract_expires_at(j: &Value) -> Option<i64> {
    let o = oauth_container(j);
    as_epoch_ms(o.get("expiresAt")).or_else(|| as_epoch_ms(o.get("expires_at")))
}

/// The Keychain account name for the Claude Code entry, needed to update it
/// in place (`security add-generic-password -U` matches on service+account).
fn keychain_account() -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // Attribute dump line: `    "acct"<blob>="longye"`
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("\"acct\"")?;
        let v = rest.split_once('=')?.1.trim();
        Some(v.trim_matches('"').to_string())
    })
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

    let origin = Origin::Keychain {
        account: keychain_account().unwrap_or_default(),
    };
    let source = format!("Keychain: {KEYCHAIN_SERVICE}");

    // The entry is usually a JSON blob; some setups store a bare token.
    let Ok(j) = serde_json::from_str::<Value>(&raw) else {
        return if is_anthropic_token(&raw) {
            vec![Candidate::bare(raw, source, origin)]
        } else {
            vec![]
        };
    };
    let Some(token) = extract_oauth_token(&j) else {
        return vec![];
    };
    if !is_anthropic_token(&token) {
        return vec![];
    }
    vec![Candidate {
        token: token.trim().into(),
        source,
        refresh_token: extract_refresh_token(&j),
        expires_at: extract_expires_at(&j),
        origin,
    }]
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
        refresh_token: extract_refresh_token(&j),
        expires_at: extract_expires_at(&j),
        origin: Origin::File(path),
    }]
}

/// Collect candidates in priority order, deduplicated by token value.
/// Credentials known to be expired are demoted below the rest, so a stale
/// Keychain entry no longer shadows a still-valid file credential.
pub fn collect_candidates(arg_token: Option<&str>) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut push = |c: Candidate| {
        let t = c.token.trim().to_string();
        if t.len() < 20 || seen.contains(&t) {
            return;
        }
        seen.insert(t.clone());
        out.push(Candidate { token: t, ..c });
    };

    if let Some(t) = arg_token {
        push(Candidate::bare(t.into(), "--token arg".into(), Origin::Arg));
    }
    if let Ok(t) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        push(Candidate::bare(
            t,
            "env CLAUDE_CODE_OAUTH_TOKEN".into(),
            Origin::Env,
        ));
    }
    for c in read_keychain() {
        push(c);
    }
    for c in read_credentials_file() {
        push(c);
    }

    // Stable partition: usable-looking credentials first, priority preserved.
    out.sort_by_key(|c| c.is_expired() && !c.can_refresh());
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

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| anyhow!("cannot build HTTP client: {e}"))
}

// ---------- OAuth refresh ----------

pub struct Refreshed {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
}

/// A refresh attempt that produced no token.
#[derive(Debug)]
pub struct RefreshError {
    /// HTTP status, or 0 for transport-level failures.
    pub status: u16,
    pub message: String,
}

impl RefreshError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        RefreshError {
            status,
            message: message.into(),
        }
    }

    /// The token endpoint throttles per account/IP. Once it says 429, trying
    /// the remaining candidates in the same run only burns more requests.
    pub fn is_rate_limited(&self) -> bool {
        self.status == 429
    }
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "HTTP {} — {}", self.status, self.message)
        }
    }
}

impl std::error::Error for RefreshError {}

/// Exchange a refresh token for a fresh access token, the same way Claude Code
/// does. The response usually carries a rotated refresh token — callers must
/// persist it, or the stored credential becomes unusable.
pub fn refresh(refresh_token: &str) -> std::result::Result<Refreshed, RefreshError> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": OAUTH_CLIENT_ID,
    })
    .to_string();

    let client = client().map_err(|e| RefreshError::new(0, e.to_string()))?;
    let res = client
        .post(OAUTH_TOKEN_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "claude-code/2.1.0 (quota-check)")
        .body(body)
        .send()
        .map_err(|e| RefreshError::new(0, format!("refresh request failed: {e}")))?;

    // The token endpoint throttles hard; surface how long it wants us to wait.
    let retry_after = res
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(|v| format!(" (retry after {v}s)"))
        .unwrap_or_default();

    let status = res.status().as_u16();
    let text = res.text().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(RefreshError::new(
            status,
            format!("{}{retry_after}", one_line(&text, 160)),
        ));
    }
    let j: Value = serde_json::from_str(&text)
        .map_err(|_| RefreshError::new(status, "refresh response is not JSON"))?;
    let access_token = j
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RefreshError::new(status, "refresh response has no access_token"))?
        .to_string();
    let expires_at = j
        .get("expires_in")
        .and_then(|v| v.as_f64())
        .map(|s| now_ms() + (s * 1000.0) as i64)
        .or_else(|| as_epoch_ms(j.get("expires_at")));

    Ok(Refreshed {
        access_token,
        refresh_token: j
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from),
        expires_at,
    })
}

/// Write the refreshed fields into an existing credential document, leaving
/// every other key (mcpOAuth entries, scopes, subscriptionType, ...) intact.
pub fn patch_document(doc: &mut Value, r: &Refreshed) {
    let key = container_key(doc);
    let target = match key {
        Some(k) => doc.get_mut(k).unwrap(),
        None => doc,
    };
    let Some(obj) = target.as_object_mut() else {
        return;
    };
    // Match the field naming already present in the document.
    let snake = obj.contains_key("access_token");
    let set = |obj: &mut serde_json::Map<String, Value>, camel: &str, snake_k: &str, v: Value| {
        obj.insert(if snake { snake_k.into() } else { camel.into() }, v);
    };
    set(
        obj,
        "accessToken",
        "access_token",
        Value::from(r.access_token.clone()),
    );
    if let Some(rt) = &r.refresh_token {
        set(
            obj,
            "refreshToken",
            "refresh_token",
            Value::from(rt.clone()),
        );
    }
    if let Some(exp) = r.expires_at {
        set(obj, "expiresAt", "expires_at", Value::from(exp));
    }
}

fn write_keychain(account: &str, blob: &str) -> Result<()> {
    let mut args = vec![
        "add-generic-password".to_string(),
        "-U".into(),
        "-s".into(),
        KEYCHAIN_SERVICE.into(),
    ];
    if !account.is_empty() {
        args.push("-a".into());
        args.push(account.into());
    }
    // `-w` with no value reads the secret from stdin (twice, for confirmation),
    // which keeps the token out of the process table.
    args.push("-w".into());

    let mut child = Command::new("security")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot run `security`")?;
    {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        writeln!(stdin, "{blob}")?;
        writeln!(stdin, "{blob}")?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "keychain update failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn write_file(path: &Path, blob: &str) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, blob)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Persist a refreshed credential back to the store it came from.
pub fn persist(origin: &Origin, r: &Refreshed) -> Result<()> {
    match origin {
        Origin::Keychain { account } => {
            let out = Command::new("security")
                .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
                .output()
                .context("cannot read the Keychain entry back")?;
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let mut doc: Value = serde_json::from_str(&raw)
                .unwrap_or_else(|_| serde_json::json!({ "claudeAiOauth": {} }));
            patch_document(&mut doc, r);
            write_keychain(account, &doc.to_string())
        }
        Origin::File(path) => {
            let raw = std::fs::read_to_string(path).context("cannot read the credentials file")?;
            let mut doc: Value =
                serde_json::from_str(&raw).context("credentials file is not valid JSON")?;
            patch_document(&mut doc, r);
            write_file(path, &serde_json::to_string_pretty(&doc)?)
        }
        Origin::Arg | Origin::Env => Ok(()),
    }
}

// ---------- quota probe ----------

pub struct Probe {
    pub ok: bool,
    pub status: u16,
    pub reason: &'static str,
    pub note: String,
    pub data: Option<Value>,
}

impl Probe {
    fn failed(reason: &'static str, note: String) -> Self {
        Probe {
            ok: false,
            status: 0,
            reason,
            note,
            data: None,
        }
    }
}

pub fn probe(token: &str) -> Probe {
    let client = match client() {
        Ok(c) => c,
        Err(e) => return Probe::failed("network", e.to_string()),
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
        Err(e) => return Probe::failed("network", e.to_string()),
    };
    let status = res.status().as_u16();
    let body = res.text().unwrap_or_default();

    if !(200..300).contains(&status) {
        return Probe {
            ok: false,
            status,
            reason: classify(status, &body),
            note: one_line(&body, 160),
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

/// Outcome of a successful lookup: the usage document, the credential that
/// worked, and any notes worth showing the user (refreshes, write-back
/// failures).
pub struct Hit {
    pub data: Value,
    pub candidate: Candidate,
    pub notes: Vec<String>,
}

/// A server-side reason that a fresh access token would plausibly fix.
fn refreshable(reason: &str) -> bool {
    matches!(reason, "expired" | "invalid" | "revoked")
}

/// Probe candidates in priority order; first 200 wins. When a candidate's
/// access token is stale and a refresh token is on hand, refresh it first.
pub fn fetch_usage(candidates: &[Candidate], allow_refresh: bool) -> Result<Hit> {
    let mut failures = vec![];
    let mut notes: Vec<String> = vec![];
    // Set once the token endpoint throttles us: further refresh attempts in
    // this run would only make it worse.
    let mut refresh_blocked = false;
    // Plain-text credential files the server has already invalidated. They are
    // pure leftovers — worth telling the user about, never worth deleting for
    // them.
    let mut stale_files: Vec<PathBuf> = vec![];

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

        let mut token = c.token.clone();
        let mut refreshed = false;

        // Locally known to be stale — refresh before spending a round trip on
        // a request that is certain to 401.
        if allow_refresh && !refresh_blocked && c.is_expired() && c.can_refresh() {
            match try_refresh(c, &mut notes) {
                Ok(t) => {
                    token = t;
                    refreshed = true;
                }
                Err(e) => {
                    refresh_blocked |= e.is_rate_limited();
                    failures.push(format!("  ---  [refresh failed] {} — {e}", c.source));
                    continue;
                }
            }
        }

        let mut r = probe(&token);

        // Server disagreed with the local expiry bookkeeping; try once more
        // with a fresh token before giving up on this candidate.
        if !r.ok
            && !refreshed
            && allow_refresh
            && !refresh_blocked
            && c.can_refresh()
            && refreshable(r.reason)
        {
            match try_refresh(c, &mut notes) {
                Ok(t) => {
                    token = t;
                    r = probe(&token);
                }
                Err(e) => refresh_blocked |= e.is_rate_limited(),
            }
        }

        if r.ok {
            return Ok(Hit {
                data: r.data.unwrap(),
                candidate: Candidate {
                    token,
                    source: c.source.clone(),
                    refresh_token: None,
                    expires_at: c.expires_at,
                    origin: c.origin.clone(),
                },
                notes,
            });
        }

        let local = c.expiry_note();
        let local = if local.is_empty() {
            String::new()
        } else {
            format!(" ({local})")
        };
        if matches!(r.reason, "revoked" | "invalid") {
            if let Origin::File(path) = &c.origin {
                stale_files.push(path.clone());
            }
        }
        failures.push(format!(
            "  {:<4} [{}] {}{local} — {}",
            r.status, r.reason, c.source, r.note
        ));
    }

    let hint = if refresh_blocked {
        "  the OAuth token endpoint is rate-limiting this machine right now.\n  Wait a few minutes and retry, or run `claude` once and let it refresh."
    } else if allow_refresh {
        "  revoked → credential was invalidated; run `claude` and log in again\n  expired → the refresh token is stale too; run `claude` to re-authenticate\n  invalid → token format or account problem"
    } else {
        "  --no-refresh is set, so expired access tokens were used as-is.\n  Drop the flag to let quota-check refresh them, or run `claude` first."
    };
    let leftovers = if stale_files.is_empty() {
        String::new()
    } else {
        let list = stale_files
            .iter()
            .map(|p| format!("    {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\n  ⚠ the OAuth entry in these files was invalidated server-side. Logging in\n    again overwrites it (the files may also hold unrelated MCP tokens, so\n    don't delete them outright):\n{list}")
    };
    bail!(
        "all {} credential candidates failed:\n{}\n\n{hint}{leftovers}",
        candidates.len(),
        failures.join("\n")
    )
}

/// Refresh one candidate and write the result back to its store. A write-back
/// failure is reported but does not fail the lookup.
fn try_refresh(
    c: &Candidate,
    notes: &mut Vec<String>,
) -> std::result::Result<String, RefreshError> {
    let rt = c
        .refresh_token
        .as_deref()
        .ok_or_else(|| RefreshError::new(0, "no refresh token stored"))?;
    let fresh = refresh(rt)?;

    if c.origin.is_writable() {
        match persist(&c.origin, &fresh) {
            Ok(()) => notes.push(format!("↻ refreshed access token, saved to {}", c.source)),
            Err(e) => notes.push(format!(
                "↻ refreshed access token, but could not save it back to {} ({e}).\n    Run `claude` to re-authenticate if the next run fails.",
                c.source
            )),
        }
    } else {
        notes.push("↻ refreshed access token (in memory only)".into());
    }
    Ok(fresh.access_token)
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
    fn reads_expiry_and_refresh_token() {
        let j = json!({"claudeAiOauth": {
            "accessToken": "sk-ant-oat01-aaaaaaaaaaaaaaaaaaaa",
            "refreshToken": "sk-ant-ort01-rrrrrrrrrrrrrrrrrrrr",
            "expiresAt": 1_788_383_209_686i64
        }});
        assert_eq!(extract_expires_at(&j), Some(1_788_383_209_686));
        assert!(extract_refresh_token(&j).is_some());
        // seconds are upgraded to milliseconds
        let secs = json!({"oauth": {"expires_at": 1_788_383_209i64}});
        assert_eq!(extract_expires_at(&secs), Some(1_788_383_209_000));
    }

    #[test]
    fn expiry_classification() {
        let past = Candidate {
            expires_at: Some(now_ms() - 3_600_000),
            ..Candidate::bare("t".into(), "s".into(), Origin::Env)
        };
        assert!(past.is_expired());
        assert!(past.expiry_note().starts_with("local token expired"));

        let future = Candidate {
            expires_at: Some(now_ms() + 3_600_000),
            ..Candidate::bare("t".into(), "s".into(), Origin::Env)
        };
        assert!(!future.is_expired());
        assert!(future.expiry_note().starts_with("local token valid"));

        let unknown = Candidate::bare("t".into(), "s".into(), Origin::Env);
        assert!(!unknown.is_expired());
        assert_eq!(unknown.expiry_note(), "");
    }

    #[test]
    fn patch_preserves_unrelated_keys() {
        let mut doc = json!({
            "mcpOAuth": {"some-server": {"accessToken": "keep-me"}},
            "claudeAiOauth": {
                "accessToken": "old",
                "refreshToken": "old-r",
                "expiresAt": 1,
                "subscriptionType": "max"
            }
        });
        patch_document(
            &mut doc,
            &Refreshed {
                access_token: "new".into(),
                refresh_token: Some("new-r".into()),
                expires_at: Some(42),
            },
        );
        assert_eq!(doc["claudeAiOauth"]["accessToken"], "new");
        assert_eq!(doc["claudeAiOauth"]["refreshToken"], "new-r");
        assert_eq!(doc["claudeAiOauth"]["expiresAt"], 42);
        assert_eq!(doc["claudeAiOauth"]["subscriptionType"], "max");
        assert_eq!(doc["mcpOAuth"]["some-server"]["accessToken"], "keep-me");
    }

    #[test]
    fn patch_follows_snake_case_documents() {
        let mut doc = json!({"access_token": "old", "expires_at": 1});
        patch_document(
            &mut doc,
            &Refreshed {
                access_token: "new".into(),
                refresh_token: None,
                expires_at: Some(9),
            },
        );
        assert_eq!(doc["access_token"], "new");
        assert_eq!(doc["expires_at"], 9);
        assert!(doc.get("accessToken").is_none());
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

        // dedupe: the same token from two sources collapses to one entry.
        // (Only the arg/env sources are asserted — the machine running the
        // tests may also have real Keychain/file credentials.)
        let cs2 = collect_candidates(Some("sk-ant-oat01-env-env-env-env-env-env"));
        let injected: Vec<&str> = cs2
            .iter()
            .filter(|c| c.token.contains("-env-env-"))
            .map(|c| c.source.as_str())
            .collect();
        assert_eq!(injected, vec!["--token arg"]);
        unsafe {
            std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        }
    }
}
