//! Kimi Code quota query (5h window / weekly quota).
//!
//! Credential strategy: don't guess which file is "the right one" — collect
//! every candidate key, probe them in order, use the first that returns 200.
//!
//! Compatible with the current Kimi Code CLI / Claude Code / Pi / cc-switch
//! setups. The retired ~/.kimi legacy CLI is not supported.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::Value;

pub const DEFAULT_BASE: &str = "https://api.kimi.com/coding/v1";

/// cc-switch local-routing mode writes placeholders into settings.json,
/// not real keys.
const SENTINELS: &[&str] = &["PROXY_MANAGED", "MANAGED", "null", "undefined", "your-api-key"];

pub fn looks_like_key(t: &str) -> bool {
    let t = t.trim();
    t.len() >= 16 && !SENTINELS.contains(&t)
}

pub struct Candidate {
    pub token: String,
    pub source: String,
}

fn read_json(p: &PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

fn claude_dir() -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".claude"))
}

/// apiKeyHelper is a shell command that prints a key.
fn run_helper(cmd: &str) -> Option<String> {
    let out = std::process::Command::new("sh")
        .args(["-c", cmd])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    looks_like_key(&t).then_some(t)
}

/// Collect candidates from every known location, deduplicated by key value.
pub fn collect_candidates(arg_key: Option<&str>) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut push = |token: Option<&str>, source: String| {
        let Some(t) = token else { return };
        if !looks_like_key(t) {
            return;
        }
        let t = t.trim();
        if seen.contains(t) {
            return;
        }
        seen.insert(t.to_string());
        out.push(Candidate {
            token: t.into(),
            source,
        });
    };

    // 1. CLI argument
    push(arg_key, "--key arg".into());

    // 2. Environment variables
    for name in ["KIMI_API_KEY", "MOONSHOT_API_KEY", "KIMI_CODE_API_KEY"] {
        push(std::env::var(name).ok().as_deref(), format!("env {name}"));
    }
    // ANTHROPIC_* only counts when the base URL points at Kimi/Moonshot.
    let base_env = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default();
    if base_env.to_lowercase().contains("kimi") || base_env.to_lowercase().contains("moonshot") {
        push(
            std::env::var("ANTHROPIC_AUTH_TOKEN").ok().as_deref(),
            "env ANTHROPIC_AUTH_TOKEN".into(),
        );
        push(
            std::env::var("ANTHROPIC_API_KEY").ok().as_deref(),
            "env ANTHROPIC_API_KEY".into(),
        );
    }

    // 3. Kimi Code CLI OAuth credentials
    let code_home = std::env::var("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".kimi-code"));
    let cred_path = code_home.join("credentials").join("kimi-code.json");
    if let Some(cred) = read_json(&cred_path) {
        let t = cred
            .get("tokens")
            .or_else(|| cred.get("credential"))
            .unwrap_or(&cred);
        for k in ["access_token", "accessToken", "api_key", "apiKey", "token"] {
            push(t.get(k).and_then(|v| v.as_str()), cred_path.display().to_string());
        }
    }

    // 4. Claude Code settings files — this is where cc-switch lands the active provider
    let settings_files = [
        claude_dir().join("settings.json"),
        claude_dir().join("settings.local.json"),
        PathBuf::from(".claude/settings.json"),
        PathBuf::from(".claude/settings.local.json"),
    ];
    for f in &settings_files {
        let Some(j) = read_json(f) else { continue };
        let env = j.get("env").cloned().unwrap_or(Value::Null);
        let url = env
            .get("ANTHROPIC_BASE_URL")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Collect even when the base URL doesn't look like Kimi — probing will
        // sort it out, and the tag helps debugging.
        let looks_kimi = url.to_lowercase().contains("kimi") || url.to_lowercase().contains("moonshot");
        let tag = if looks_kimi {
            String::new()
        } else {
            format!(" [base={}]", if url.is_empty() { "unset" } else { url })
        };
        let src = format!("{}{tag}", f.display());
        push(
            env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()),
            src.clone(),
        );
        push(env.get("ANTHROPIC_API_KEY").and_then(|v| v.as_str()), src);
        if let Some(helper) = j.get("apiKeyHelper").and_then(|v| v.as_str()) {
            if let Some(t) = run_helper(helper) {
                push(Some(&t), format!("{} (apiKeyHelper)", f.display()));
            }
        }
    }

    // 5. Pi
    let pi_auth = home().join(".pi").join("agent").join("auth.json");
    if let Some(pi) = read_json(&pi_auth) {
        if let Some(obj) = pi.as_object() {
            for (k, v) in obj {
                if !v.is_object() {
                    continue;
                }
                let kl = k.to_lowercase();
                if !kl.contains("kimi") && !kl.contains("moonshot") {
                    continue;
                }
                for kk in ["access_token", "accessToken", "api_key", "apiKey"] {
                    push(
                        v.get(kk).and_then(|x| x.as_str()),
                        format!("{} → {k}", pi_auth.display()),
                    );
                }
            }
        }
        push(
            pi.get("access_token").and_then(|v| v.as_str()),
            pi_auth.display().to_string(),
        );
    }
    let pi_cfg = home().join(".pi").join("providers").join("kimi-coding").join("config.json");
    if let Some(cfg) = read_json(&pi_cfg) {
        push(
            cfg.get("api_key").or_else(|| cfg.get("apiKey")).and_then(|v| v.as_str()),
            pi_cfg.display().to_string(),
        );
    }

    // Note: cc-switch's SQLite store (~/.cc-switch/cc-switch.db) is not scanned
    // yet — tracked on the roadmap.
    out
}

pub struct Probe {
    pub ok: bool,
    pub status: u16,
    pub note: String,
    pub data: Option<Value>,
}

pub fn probe(token: &str, base: &str) -> Probe {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Probe {
                ok: false,
                status: 0,
                note: e.to_string(),
                data: None,
            }
        }
    };

    let res = client
        .get(format!("{base}/usages"))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("User-Agent", concat!("quota-check/", env!("CARGO_PKG_VERSION")))
        .send();

    let res = match res {
        Ok(r) => r,
        Err(e) => {
            return Probe {
                ok: false,
                status: 0,
                note: e.to_string(),
                data: None,
            }
        }
    };
    let status = res.status().as_u16();
    let body = res.text().unwrap_or_default();

    if !(200..300).contains(&status) {
        return Probe {
            ok: false,
            status,
            note: body.chars().take(140).collect(),
            data: None,
        };
    }
    match serde_json::from_str::<Value>(&body) {
        Ok(data) => Probe {
            ok: true,
            status,
            note: String::new(),
            data: Some(data),
        },
        Err(_) => Probe {
            ok: false,
            status,
            note: "response is not JSON".into(),
            data: None,
        },
    }
}

/// Probe candidates in order; first 200 wins.
pub fn fetch_usage(candidates: &[Candidate], base: &str) -> Result<(Value, Candidate)> {
    let mut failures = vec![];
    for c in candidates {
        let r = probe(&c.token, base);
        if r.ok {
            return Ok((
                r.data.unwrap(),
                Candidate {
                    token: c.token.clone(),
                    source: c.source.clone(),
                },
            ));
        }
        failures.push(format!("  {:<4} {} — {}", r.status, c.source, r.note));
    }
    bail!(
        "all {} credential candidates failed:\n{}\n\n  common causes: token expired (re-run `kimi /login`);\n  wrong region (for CN subscriptions try --base pointing at moonshot.cn);\n  these keys are not Kimi's at all (cc-switch may have another provider active).",
        candidates.len(),
        failures.join("\n")
    )
}

// ---------- normalization for the human renderer ----------
// Kimi's response carries limit/used/remaining numbers, not percentages, so
// reshape it into the generic window shape the renderer understands.

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// Window spec {duration, timeUnit} → seconds.
pub fn window_seconds(w: &Value) -> Option<f64> {
    let d = num(w.get("duration"))?;
    let unit = w
        .get("timeUnit")
        .or_else(|| w.get("time_unit"))
        .and_then(|v| v.as_str())?;
    let secs = match unit {
        "TIME_UNIT_SECOND" | "SECOND" => 1.0,
        "TIME_UNIT_MINUTE" | "MINUTE" => 60.0,
        "TIME_UNIT_HOUR" | "HOUR" => 3600.0,
        "TIME_UNIT_DAY" | "DAY" => 86400.0,
        "TIME_UNIT_WEEK" | "WEEK" => 604800.0,
        _ => return None,
    };
    Some(d * secs)
}

/// Reshape the raw /usages response into { label: {used_percent,
/// window_size_seconds?, resets_in_seconds?} } rows for the generic renderer.
pub fn normalize(data: &Value) -> Value {
    let mut map = serde_json::Map::new();
    let mut idx = 0;

    let mut add_row = |detail: &Value, window: Option<&Value>, idx: &mut usize| {
        let limit = num(detail.get("limit"));
        let remaining = num(detail.get("remaining"));
        let used = num(detail.get("used")).or_else(|| match (limit, remaining) {
            (Some(l), Some(r)) => Some(l - r),
            _ => None,
        });
        if limit.is_none() && used.is_none() {
            return;
        }
        let pct = match (limit, used) {
            (Some(l), Some(u)) if l > 0.0 => (u / l) * 100.0,
            _ => 0.0,
        };
        let mut row = serde_json::Map::new();
        row.insert("used_percent".into(), Value::from(pct));
        if let Some(sec) = window.and_then(window_seconds) {
            row.insert("window_size_seconds".into(), Value::from(sec));
        }
        if let Some(rt) = detail
            .get("resetTime")
            .or_else(|| detail.get("reset_time"))
            .and_then(|v| v.as_str())
        {
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(rt) {
                let left = (t.timestamp_millis() - chrono::Utc::now().timestamp_millis()) as f64 / 1000.0;
                row.insert("resets_in_seconds".into(), Value::from(left.max(0.0)));
            }
        }
        map.insert(format!("window_{idx}"), Value::Object(row));
        *idx += 1;
    };

    // Top-level weekly quota. The API gives no window spec here; it is weekly.
    if let Some(d) = data.get("usage").or_else(|| data.get("membership")) {
        add_row(d, Some(&serde_json::json!({"duration": 1, "timeUnit": "TIME_UNIT_WEEK"})), &mut idx);
    }
    if let Some(limits) = data.get("limits").and_then(|v| v.as_array()) {
        for item in limits {
            let detail = item.get("detail").or_else(|| item.get("usage")).unwrap_or(item);
            add_row(detail, item.get("window"), &mut idx);
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_shape_filter() {
        assert!(looks_like_key("sk-1234567890abcdef"));
        assert!(!looks_like_key("short"));
        assert!(!looks_like_key("PROXY_MANAGED"));
        assert!(!looks_like_key("your-api-key"));
        assert!(looks_like_key("  sk-1234567890abcdef  "));
    }

    #[test]
    fn window_seconds_units() {
        assert_eq!(
            window_seconds(&json!({"duration": 5, "timeUnit": "TIME_UNIT_HOUR"})),
            Some(18000.0)
        );
        assert_eq!(
            window_seconds(&json!({"duration": 1, "time_unit": "WEEK"})),
            Some(604800.0)
        );
        assert_eq!(window_seconds(&json!({"duration": 5})), None);
        assert_eq!(
            window_seconds(&json!({"duration": 5, "timeUnit": "PARSEC"})),
            None
        );
    }

    #[test]
    fn normalize_computes_percentages() {
        let data = json!({
            "usage": {"limit": 100, "used": 25, "resetTime": "2099-01-01T00:00:00Z"},
            "limits": [
                {"window": {"duration": 5, "timeUnit": "TIME_UNIT_HOUR"},
                 "detail": {"limit": 50, "remaining": 10}}
            ]
        });
        let n = normalize(&data);
        let rows = n.as_object().unwrap();
        assert_eq!(rows.len(), 2);
        let weekly = &rows["window_0"];
        assert_eq!(weekly["used_percent"].as_f64().unwrap(), 25.0);
        assert_eq!(weekly["window_size_seconds"].as_f64().unwrap(), 604800.0);
        assert!(weekly["resets_in_seconds"].as_f64().unwrap() > 0.0);
        let five_h = &rows["window_1"];
        // used derived from limit - remaining = 40 → 80%
        assert_eq!(five_h["used_percent"].as_f64().unwrap(), 80.0);
        assert_eq!(five_h["window_size_seconds"].as_f64().unwrap(), 18000.0);
    }

    #[test]
    fn normalize_skips_empty_details() {
        let data = json!({"limits": [{"detail": {"unrelated": 1}}]});
        assert_eq!(normalize(&data).as_object().unwrap().len(), 0);
    }
}
