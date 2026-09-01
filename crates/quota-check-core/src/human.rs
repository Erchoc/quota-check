//! 人类可读渲染。
//!
//! 不硬编码字段名：递归扫描整个响应，凡是「含有百分比字段的对象」都当成一个
//! 额度窗口。私有接口字段变了也能尽量渲染出来。

use serde_json::{Map, Value};

use crate::auth::Identity;

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

/// 扫描出的一个额度窗口。
pub struct Window {
    pub path: String,
    pub percent: f64,
    pub reset_seconds: Option<f64>,
    pub reset_at: Option<Value>,
    pub window: Option<(String, Value)>,
}

fn pick<'a>(obj: &'a Map<String, Value>, keys: &[&'a str]) -> Option<(&'a str, &'a Value)> {
    keys.iter().find_map(|k| {
        obj.get(*k)
            .filter(|v| !v.is_null())
            .map(|v| (*k, v))
    })
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

/// 窗口时长归一化成「周 / 日 / Nh / Nd」这样的人话标签。
fn duration_label(sec: f64) -> Option<String> {
    if (sec - 604800.0).abs() <= 60480.0 {
        Some("周".into())
    } else if (sec - 86400.0).abs() <= 8640.0 {
        Some("日".into())
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

fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round().clamp(0.0, width as f64) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn colorize(pct: f64, text: &str, enabled: bool) -> String {
    if !enabled {
        return text.to_string();
    }
    let code = if pct >= 90.0 {
        31 // 红
    } else if pct >= 75.0 {
        33 // 黄
    } else {
        32 // 绿
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn fmt_reset(w: &Window) -> String {
    if let Some(secs) = w.reset_seconds {
        return format!("重置 {}", fmt_duration(secs));
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
        "重置 {}  ({})",
        fmt_duration(left),
        local.format("%Y-%m-%d %H:%M:%S")
    )
}

/// `color`：是否输出 ANSI 颜色（一般取 stdout 是否 TTY）。
pub fn render(data: &Value, ident: Option<&Identity>, auth_path: &str, color: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(String::new());
    lines.push("  Codex 额度".into());
    lines.push(format!("  {}", "─".repeat(46)));

    if let Some(id) = ident {
        let bits: Vec<&str> = [id.email.as_deref(), id.plan.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !bits.is_empty() {
            lines.push(format!("  {}", bits.join("  ·  ")));
        }
        lines.push(format!("  凭据  {auth_path}"));
        lines.push(String::new());
    }

    let windows = collect_windows(data);
    if windows.is_empty() {
        lines.push("  没在响应里找到百分比字段。".into());
        lines.push("  去掉 --human 看原始 JSON，接口结构可能变了。".into());
        lines.push(String::new());
        return lines.join("\n");
    }

    for w in &windows {
        let pct = if w.percent <= 1.0 && w.percent > 0.0 {
            w.percent * 100.0
        } else {
            w.percent
        };
        let label = window_label(&w.window)
            .or_else(|| w.reset_seconds.and_then(duration_label))
            .unwrap_or_else(|| {
                w.path
                    .rsplit('.')
                    .next()
                    .unwrap_or(&w.path)
                    .to_string()
            });
        let pct_text = format!("{:>6}", format!("{pct:.1}%"));
        lines.push(format!(
            "  {:<6} {} {}   {}",
            label,
            colorize(pct, &bar(pct, 24), color),
            colorize(pct, &pct_text, color),
            fmt_reset(w)
        ));
        let dim = if color { "\x1b[2m" } else { "" };
        let reset = if color { "\x1b[0m" } else { "" };
        lines.push(format!("  {dim}      {}{reset}", w.path));
    }

    lines.push(String::new());
    lines.join("\n")
}
