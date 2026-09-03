//! Human-readable rendering.
//!
//! No hardcoded field paths: recursively scan the response and treat any
//! object containing a percentage field as a quota window. Providers may pass
//! a pre-normalized document (see `providers::kimi::normalize`) when the raw
//! response carries limit/used numbers instead of percentages.
//!
//! Windows are always rendered shortest-window-first (5h before week), so
//! every provider prints the same way regardless of its JSON key order.

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
    // Codex reports the window length under this name.
    "limit_window_seconds",
    "limitWindowSeconds",
    "window_hours",
    "windowHours",
    "window",
];

const HOUR: f64 = 3600.0;
const DAY: f64 = 86400.0;
const WEEK: f64 = 604800.0;

/// Well-known path segment → (label, canonical window length in seconds).
/// Authoritative window keys win over these; these win over reset-seconds
/// inference (remaining time ≠ window size).
const KNOWN_LABELS: &[(&str, &str, f64)] = &[
    ("primary_window", "5h", 5.0 * HOUR),
    ("secondary_window", "week", WEEK),
    ("five_hour", "5h", 5.0 * HOUR),
    ("seven_day", "week", WEEK),
    ("seven_day_opus", "week · Opus", WEEK),
    ("seven_day_sonnet", "week · Sonnet", WEEK),
    ("seven_day_oauth_apps", "week · OAuth apps", WEEK),
    ("today", "day", DAY),
];

/// Bar width in cells.
const BAR_WIDTH: usize = 24;
/// Separator width, wide enough for the longest window line.
const RULE_WIDTH: usize = 58;

/// One quota window found by scanning.
pub struct Window {
    pub path: String,
    pub percent: f64,
    pub reset_seconds: Option<f64>,
    pub reset_at: Option<Value>,
    pub window: Option<(String, Value)>,
}

impl Window {
    /// Percentage normalized to 0..100 (some providers report 0..1 fractions).
    pub fn pct(&self) -> f64 {
        if self.percent <= 1.0 && self.percent > 0.0 {
            self.percent * 100.0
        } else {
            self.percent
        }
    }
}

fn pick<'a>(obj: &'a Map<String, Value>, keys: &[&'a str]) -> Option<(&'a str, &'a Value)> {
    keys.iter()
        .find_map(|k| obj.get(*k).filter(|v| !v.is_null()).map(|v| (*k, v)))
}

pub fn collect_windows(data: &Value) -> Vec<Window> {
    let mut out = Vec::new();
    walk(data, &mut Vec::new(), &mut out);
    sort_windows(&mut out);
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

/// Shortest window first (hourly before weekly); windows whose length cannot
/// be inferred sink to the bottom. Ties break on the label so the plain
/// `week` row lands before its `week · Opus` variants.
fn sort_windows(windows: &mut [Window]) {
    windows.sort_by(|a, b| {
        let ka = window_seconds(a).unwrap_or(f64::INFINITY);
        let kb = window_seconds(b).unwrap_or(f64::INFINITY);
        ka.partial_cmp(&kb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| label_for(a).cmp(&label_for(b)))
    });
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
    if (sec - WEEK).abs() <= WEEK * 0.1 {
        Some("week".into())
    } else if (sec - DAY).abs() <= DAY * 0.1 {
        Some("day".into())
    } else if sec < DAY {
        Some(format!("{}h", (sec / HOUR).round() as u64))
    } else {
        Some(format!("{}d", (sec / DAY).round() as u64))
    }
}

/// Length of a window in seconds, from the explicit `window_*` field.
fn declared_window_seconds(w: &Option<(String, Value)>) -> Option<f64> {
    let (key, value) = w.as_ref()?;
    let n = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))?;
    if key.contains("minutes") {
        Some(n * 60.0)
    } else if key.contains("hours") {
        Some(n * HOUR)
    } else if key.contains("seconds") {
        Some(n)
    } else {
        None
    }
}

fn known_label(w: &Window) -> Option<(&'static str, f64)> {
    let last = w.path.rsplit('.').next().unwrap_or(&w.path);
    KNOWN_LABELS
        .iter()
        .find(|(k, _, _)| *k == last)
        .map(|(_, label, secs)| (*label, *secs))
}

/// Best guess at how long this window is. Used for ordering only.
fn window_seconds(w: &Window) -> Option<f64> {
    declared_window_seconds(&w.window)
        .or_else(|| known_label(w).map(|(_, s)| s))
        .or(w.reset_seconds)
}

fn label_for(w: &Window) -> String {
    if let Some(l) = declared_window_seconds(&w.window).and_then(duration_label) {
        return l;
    }
    if let Some((l, _)) = known_label(w) {
        return l.to_string();
    }
    if let Some(l) = w.reset_seconds.and_then(duration_label) {
        return l;
    }
    w.path.rsplit('.').next().unwrap_or(&w.path).to_string()
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
    let Some(v) = &w.reset_at else {
        return String::new();
    };
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
        "resets in {:<8} {}",
        fmt_duration(left),
        local.format("%m-%d %H:%M")
    )
}

/// Squash a (possibly pretty-printed) response body onto one line and clip it,
/// so a failed candidate stays one readable row in the failure list.
pub fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(max).collect()
}

/// Mask a token for display: first 6 + … + last 4.
pub fn mask(t: &str) -> String {
    if t.len() <= 12 {
        "***".into()
    } else {
        format!("{}…{}", &t[..6], &t[t.len() - 4..])
    }
}

struct Style {
    dim: &'static str,
    bold: &'static str,
    reset: &'static str,
}

impl Style {
    fn new(color: bool) -> Self {
        if color {
            Style {
                dim: "\x1b[2m",
                bold: "\x1b[1m",
                reset: "\x1b[0m",
            }
        } else {
            Style {
                dim: "",
                bold: "",
                reset: "",
            }
        }
    }
}

/// Render a quota document. `header` lines (account, credential source, ...)
/// are printed between the title and the window bars.
///
/// `color`: emit ANSI colors (usually stdout-is-TTY).
pub fn render(title: &str, data: &Value, header: &[String], color: bool) -> String {
    let s = Style::new(color);
    let mut lines: Vec<String> = Vec::new();

    lines.push(String::new());
    lines.push(format!("  {}{title}{}", s.bold, s.reset));
    lines.push(format!("  {}{}{}", s.dim, "─".repeat(RULE_WIDTH), s.reset));
    for l in header {
        lines.push(format!("  {}{l}{}", s.dim, s.reset));
    }
    if !header.is_empty() {
        lines.push(String::new());
    }

    let windows = collect_windows(data);
    if windows.is_empty() {
        lines.push("  No percentage fields found in the response.".into());
        lines.push(
            "  Re-run with --json to inspect the raw response; the API shape may have changed."
                .into(),
        );
        lines.push(String::new());
        return lines.join("\n");
    }

    let labels: Vec<String> = windows.iter().map(label_for).collect();
    let width = labels
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);

    for (w, label) in windows.iter().zip(&labels) {
        let pct = w.pct();
        // Pad by char count: labels may contain multi-byte characters, which
        // makes `{:<width$}` (byte-based for str) misalign.
        let pad = " ".repeat(width - label.chars().count());
        lines.push(format!(
            "  {label}{pad}  {}  {}  {}{}{}",
            colorize(pct, &bar(pct, BAR_WIDTH), color),
            colorize(pct, &format!("{:>6}", format!("{pct:.1}%")), color),
            s.dim,
            fmt_reset(w),
            s.reset
        ));
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
    fn codex_window_length_comes_from_the_api() {
        // Real /wham/usage shape: the weekly window is listed second, and its
        // length is declared as `limit_window_seconds`.
        let data = json!({
            "rate_limit": {
                "secondary_window": {
                    "used_percent": 8,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 567961
                },
                "primary_window": {
                    "used_percent": 41,
                    "limit_window_seconds": 18000,
                    "reset_after_seconds": 3038
                }
            }
        });
        let ws = collect_windows(&data);
        let labels: Vec<String> = ws.iter().map(label_for).collect();
        assert_eq!(labels, vec!["5h", "week"]);
        assert_eq!(ws[0].pct(), 41.0);
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
    fn hourly_window_sorts_before_weekly() {
        // Weekly appears first in the JSON; the renderer must still put 5h on top.
        let data = json!({
            "seven_day": {"utilization": 60},
            "seven_day_opus": {"utilization": 10},
            "five_hour": {"utilization": 5}
        });
        let labels: Vec<String> = collect_windows(&data).iter().map(label_for).collect();
        assert_eq!(labels, vec!["5h", "week", "week · Opus"]);
    }

    #[test]
    fn unknown_windows_sink_to_the_bottom() {
        let data = json!({
            "mystery": {"used_percent": 1},
            "five_hour": {"utilization": 2},
            "today": {"utilization": 3}
        });
        let labels: Vec<String> = collect_windows(&data).iter().map(label_for).collect();
        assert_eq!(labels, vec!["5h", "day", "mystery"]);
    }

    #[test]
    fn render_puts_hourly_line_first() {
        let data = json!({
            "seven_day": {"utilization": 60},
            "five_hour": {"utilization": 5}
        });
        let out = render("T", &data, &[], false);
        let five = out.find("5h").unwrap();
        let week = out.find("week").unwrap();
        assert!(five < week);
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
    fn labels_are_padded_by_char_count() {
        let data = json!({
            "five_hour": {"utilization": 5},
            "seven_day_opus": {"utilization": 10}
        });
        let out = render("T", &data, &[], false);
        // Both bars must start at the same column despite the multi-byte "·".
        let cols: Vec<usize> = out
            .lines()
            .filter(|l| l.contains('█') || l.contains('░'))
            .map(|l| l.chars().position(|c| c == '█' || c == '░').unwrap())
            .collect();
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0], cols[1]);
    }

    #[test]
    fn bar_clamps() {
        assert_eq!(bar(150.0, 4), "████");
        assert_eq!(bar(0.0, 4), "░░░░");
        assert_eq!(bar(50.0, 4), "██░░");
    }

    #[test]
    fn one_line_flattens_and_clips() {
        let pretty = "{\n  \"error\": {\n    \"type\": \"rate_limit_error\"\n  }\n}";
        assert_eq!(
            one_line(pretty, 160),
            "{ \"error\": { \"type\": \"rate_limit_error\" } }"
        );
        assert_eq!(one_line("abcdefghij", 4), "abcd");
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
