//! Human-readable rendering.
//!
//! No hardcoded field paths: recursively scan the response and treat any
//! object containing a percentage field as a quota window. Providers may pass
//! a pre-normalized document (see `providers::kimi::normalize`) when the raw
//! response carries limit/used numbers instead of percentages.

use serde_json::{Map, Value};

const PCT_KEYS: &[&str] = &[
    "used_percent",
    "usedPercent",
    "used_percentage",
    "usedPercentage",
    "utilization",
    "percent_used",
    "percentUsed",
];
const RESET_SEC_KEYS: &[&str] = &[
    "resets_in_seconds",
    "resetsInSeconds",
    "reset_after_seconds",
    "resetAfterSeconds",
    "seconds_until_reset",
    "secondsUntilReset",
    "reset_in_seconds",
    "resetInSeconds",
];
const RESET_AT_KEYS: &[&str] = &[
    "resets_at",
    "resetsAt",
    "reset_at",
    "resetAt",
    "reset_time",
    "resetTime",
    "resets_at_utc",
];
const WINDOW_KEYS: &[&str] = &[
    "window_minutes",
    "windowMinutes",
    "window_size_seconds",
    "windowSizeSeconds",
    "window_hours",
    "windowHours",
    "window",
];

/// Well-known path segment → label. Authoritative window keys win over these;
/// these win over reset-seconds inference (remaining time ≠ window size).
const KNOWN_LABELS: &[(&str, &str)] = &[
    ("primary_window", "5h"),
    ("secondary_window", "week"),
    ("five_hour", "5h"),
    ("seven_day", "week"),
    ("seven_day_opus", "week · Opus"),
    ("seven_day_sonnet", "week · Sonnet"),
    ("seven_day_oauth_apps", "week · OAuth apps"),
    ("today", "day"),
];

/// One quota window found by scanning.
pub struct Window {
    pub path: String,
    pub percent: f64,
    pub reset_seconds: Option<f64>,
    pub reset_at: Option<Value>,
    pub window: Option<(String, Value)>,
}

fn pick<'a>(obj: &'a Map<String, Value>, keys: &[&'a str]) -> Option<(&'a str, &'a Value)> {
    keys.iter()
        .find_map(|k| obj.get(*k).filter(|v| !v.is_null()).map(|v| (*k, v)))
}

pub fn collect_windows(data: &Value) -> Vec<Window> {
    let mut out = Vec::new();
    walk(data, &mut Vec::new(), &mut out);
    out
}

fn walk(node: &Value, path: &mut Vec<String>, out: &mut Vec<Window>) {
    match node {
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                path.push(i.to_string());
                walk(v, path, out);
                path.pop();
            }
        }
        Value::Object(obj) => {
            if let Some((_, v)) = pick(obj, PCT_KEYS) {
                if let Some(p) = v.as_f64() {
                    out.push(Window {
                        path: if path.is_empty() {
                            "(root)".into()
                        } else {
                            path.join(".")
                        },
                        percent: p,
                        reset_seconds: pick(obj, RESET_SEC_KEYS).and_then(|(_, v)| v.as_f64()),
                        reset_at: pick(obj, RESET_AT_KEYS).map(|(_, v)| v.clone()),
                        window: pick(obj, WINDOW_KEYS).map(|(k, v)| (k.to_string(), v.clone())),
                    });
                }
            }
            for (k, v) in obj {
                path.push(k.clone());
                walk(v, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn fmt_duration(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Normalize a duration in seconds to a human label: week / day / Nh / Nd.
fn duration_label(sec: f64) -> Option<String> {
    if (sec - 604800.0).abs() <= 60480.0 {
        Some("week".into())
    } else if (sec - 86400.0).abs() <= 8640.0 {
        Some("day".into())
    } else if sec < 86400.0 {
        Some(format!("{}h", (sec / 3600.0).round() as u64))
    } else {
        Some(format!("{}d", (sec / 86400.0).round() as u64))
    }
}

fn window_label(w: &Option<(String, Value)>) -> Option<String> {
    let (key, value) = w.as_ref()?;
    let n = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))?;
    let sec = if key.contains("minutes") {
        n * 60.0
    } else if key.contains("hours") {
        n * 3600.0
    } else if key.contains("seconds") {
        n
    } else {
        return None;
    };
    duration_label(sec)
}

fn label_for(w: &Window) -> String {
    let last = w.path.rsplit('.').next().unwrap_or(&w.path);
    if let Some(l) = window_label(&w.window) {
        return l;
    }
    if let Some((_, l)) = KNOWN_LABELS.iter().find(|(k, _)| *k == last) {
        return l.to_string();
    }
    if let Some(l) = w.reset_seconds.and_then(duration_label) {
        return l;
    }
    last.to_string()
}

fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn colorize(pct: f64, text: &str, enabled: bool) -> String {
    if !enabled {
        return text.to_string();
    }
    let code = if pct >= 90.0 {
        31 // red
    } else if pct >= 75.0 {
        33 // yellow
    } else {
        32 // green
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn fmt_reset(w: &Window) -> String {
    if let Some(secs) = w.reset_seconds {
        return format!("resets in {}", fmt_duration(secs));
    }
    let Some(v) = &w.reset_at else { return String::new() };
    let t = if let Some(n) = v.as_f64() {
        let ms = if n > 1e11 { n } else { n * 1000.0 };
        chrono::DateTime::from_timestamp_millis(ms as i64)
    } else if let Some(s) = v.as_str() {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc))
    } else {
        None
    };
    let Some(t) = t else { return String::new() };
    let left = (t.timestamp_millis() - chrono::Utc::now().timestamp_millis()) as f64 / 1000.0;
    let local = t.with_timezone(&chrono::Local);
    format!(
        "resets in {}  ({})",
        fmt_duration(left),
        local.format("%Y-%m-%d %H:%M:%S")
    )
}

/// Mask a token for display: first 6 + … + last 4.
pub fn mask(t: &str) -> String {
    if t.len() <= 12 {
        "***".into()
    } else {
        format!("{}…{}", &t[..6], &t[t.len() - 4..])
    }
}

/// Render a quota document. `header` lines (account, credential source, ...)
/// are printed between the title and the window bars.
///
/// `color`: emit ANSI colors (usually stdout-is-TTY).
pub fn render(title: &str, data: &Value, header: &[String], color: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(String::new());
    lines.push(format!("  {title}"));
    lines.push(format!("  {}", "─".repeat(46)));
    lines.extend(header.iter().map(|l| format!("  {l}")));
    if !header.is_empty() {
        lines.push(String::new());
    }

    let windows = collect_windows(data);
    if windows.is_empty() {
        lines.push("  No percentage fields found in the response.".into());
        lines.push("  Drop --human to inspect the raw JSON; the API shape may have changed.".into());
        lines.push(String::new());
        return lines.join("\n");
    }

    for w in &windows {
        let pct = if w.percent <= 1.0 && w.percent > 0.0 {
            w.percent * 100.0
        } else {
            w.percent
        };
        let label = label_for(w);
        let pct_text = format!("{:>6}", format!("{pct:.1}%"));
        lines.push(format!(
            "  {:<16} {} {}   {}",
            label,
            colorize(pct, &bar(pct, 24), color),
            colorize(pct, &pct_text, color),
            fmt_reset(w)
        ));
        let dim = if color { "\x1b[2m" } else { "" };
        let reset = if color { "\x1b[0m" } else { "" };
        lines.push(format!("  {dim}  {}{reset}", w.path));
    }

    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_codex_style_windows() {
        let data = json!({
            "rate_limit": {
                "primary_window": {"used_percent": 12.5, "resets_in_seconds": 18000},
                "secondary_window": {"used_percent": 80.0, "resets_in_seconds": 604800}
            }
        });
        let ws = collect_windows(&data);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].percent, 12.5);
        assert_eq!(label_for(&ws[0]), "5h");
        assert_eq!(label_for(&ws[1]), "week");
    }

    #[test]
    fn collects_claude_style_utilization() {
        let data = json!({
            "five_hour": {"utilization": 3, "resets_at": "2099-01-01T00:00:00Z"},
            "seven_day_opus": {"utilization": 0}
        });
        let ws = collect_windows(&data);
        assert_eq!(ws.len(), 2);
        assert_eq!(label_for(&ws[1]), "week · Opus");
        assert!(fmt_reset(&ws[0]).contains("resets in"));
    }

    #[test]
    fn fraction_percent_is_scaled() {
        let data = json!({"w": {"used_percent": 0.42}});
        let out = render("T", &data, &[], false);
        assert!(out.contains("42.0%"));
    }

    #[test]
    fn window_size_seconds_beats_known_label() {
        let data = json!({"mystery": {"used_percent": 1, "window_size_seconds": 604800}});
        let ws = collect_windows(&data);
        assert_eq!(label_for(&ws[0]), "week");
    }

    #[test]
    fn duration_label_buckets() {
        assert_eq!(duration_label(604800.0).unwrap(), "week");
        assert_eq!(duration_label(86400.0).unwrap(), "day");
        assert_eq!(duration_label(18000.0).unwrap(), "5h");
        assert_eq!(duration_label(172800.0).unwrap(), "2d");
    }

    #[test]
    fn bar_clamps() {
        assert_eq!(bar(150.0, 4), "████");
        assert_eq!(bar(0.0, 4), "░░░░");
        assert_eq!(bar(50.0, 4), "██░░");
    }

    #[test]
    fn masks_tokens() {
        assert_eq!(mask("sk-ant-oat01-abcdefgh"), "sk-ant…efgh");
        assert_eq!(mask("short"), "***");
    }

    #[test]
    fn empty_response_is_explained() {
        let out = render("T", &json!({"foo": 1}), &[], false);
        assert!(out.contains("No percentage fields"));
    }
}
