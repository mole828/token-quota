//! 终端文本输出。时间按本地时区显示。

use crate::snapshot::{Balance, Collected, Snapshot, Source, UsageWindow};
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use std::fmt::Display;
use std::fmt::Write;

/// 超过这个时长的服务器数据标记为"可能过期"。
const STALE_AFTER_MINUTES: i64 = 30;

/// 本机查询结果。`tz` 决定重置时间按哪个时区显示。
pub fn local_report<Tz: TimeZone>(results: &[Collected], now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let mut out = String::new();
    for result in results {
        match (&result.snapshot, &result.error) {
            (Some(snapshot), _) => out.push_str(&snapshot_block(snapshot, now, tz, false)),
            (None, Some(error)) => {
                let _ = writeln!(out, "{} · {}\n  ✗ {}\n", result.platform.display_name(), result.account, error);
            }
            (None, None) => {}
        }
    }
    out
}

/// 服务器汇总，额外显示来源设备和数据新旧。
pub fn remote_report<Tz: TimeZone>(snapshots: &[Snapshot], now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    if snapshots.is_empty() {
        return "服务器上还没有任何数据。\n".to_string();
    }
    snapshots.iter().map(|s| snapshot_block(s, now, tz, true)).collect()
}

fn snapshot_block<Tz: TimeZone>(snapshot: &Snapshot, now: DateTime<Utc>, tz: &Tz, show_device: bool) -> String
where
    Tz::Offset: Display,
{
    let mut out = String::new();
    let mut header = format!("{} · {}", snapshot.platform.display_name(), snapshot.account);
    if let Some(plan) = &snapshot.plan {
        let _ = write!(header, " · {plan}");
    }
    let _ = writeln!(out, "{header}");

    let mut meta = Vec::new();
    if show_device {
        meta.push(format!("来自 {}", snapshot.device));
    }
    meta.push(format!("{}更新", age(snapshot.fetched_at, now)));
    if snapshot.source == Source::SessionLog {
        meta.push("取自本机会话记录".to_string());
    }
    if (now - snapshot.fetched_at).num_minutes() > STALE_AFTER_MINUTES {
        meta.push("⚠ 数据可能已过期".to_string());
    }
    let _ = writeln!(out, "  {}", meta.join(" · "));

    for window in &snapshot.windows {
        let _ = writeln!(out, "  {}", window_line(window, now, tz));
    }
    for balance in &snapshot.balances {
        let _ = writeln!(out, "  {}", balance_line(balance));
    }
    out.push('\n');
    out
}

fn window_line<Tz: TimeZone>(window: &UsageWindow, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let used = match window.used_percent {
        Some(p) => format!("{p:>5.1}% {}", bar(p)),
        None => "  未知".to_string(),
    };
    let reset = match window.resets_at {
        Some(at) => format!("  {}重置（{}）", until(at, now), at.with_timezone(tz).format("%m-%d %H:%M")),
        None => String::new(),
    };
    format!("{:<10} {used}{reset}", window.name)
}

fn balance_line(balance: &Balance) -> String {
    let remaining = balance.remaining.map(number).unwrap_or_else(|| "未知".to_string());
    let detail = match (balance.used, balance.total) {
        (Some(used), Some(total)) => format!("（已用 {} / 共 {}）", number(used), number(total)),
        _ => String::new(),
    };
    format!("{:<10} 剩余 {remaining} {}{detail}", balance.name, balance.unit)
}

fn bar(percent: f64) -> String {
    const WIDTH: usize = 20;
    let filled = ((percent.clamp(0.0, 100.0) / 100.0) * WIDTH as f64).round() as usize;
    format!("[{}{}]", "#".repeat(filled), "-".repeat(WIDTH - filled))
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 { format!("{value:.0}") } else { format!("{value:.2}") }
}

fn age(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (now - at).num_minutes();
    match minutes {
        m if m < 1 => "刚刚".to_string(),
        m if m < 60 => format!("{m} 分钟前"),
        m if m < 60 * 24 => format!("{} 小时前", m / 60),
        m => format!("{} 天前", m / (60 * 24)),
    }
}

fn until(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (at - now).num_minutes();
    match minutes {
        m if m <= 0 => "已到".to_string(),
        m if m < 60 => format!("{m} 分钟后"),
        m if m < 60 * 24 => format!("{} 小时 {} 分钟后", m / 60, m % 60),
        m => format!("{} 天 {} 小时后", m / (60 * 24), (m / 60) % 24),
    }
}

/// 解析 UTC 偏移，接受 `date +%z` 的输出（`+0800`、`-0530`）和 `+08:00`。
///
/// URL 查询参数里的 `+` 会被解码成空格，所以开头的空格按 `+` 处理。
/// 固定偏移不处理夏令时。
pub fn parse_utc_offset(text: &str) -> Option<FixedOffset> {
    let text = text.trim_end();
    let (sign, digits) = match text.chars().next()? {
        '+' | ' ' => (1, &text[1..]),
        '-' => (-1, &text[1..]),
        _ => return None,
    };
    let digits = digits.replace(':', "");
    if digits.len() != 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    if minutes >= 60 {
        return None;
    }
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_date_z_output_and_url_decoded_plus() {
        let east8 = FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(parse_utc_offset("+0800"), Some(east8));
        assert_eq!(parse_utc_offset(" 0800"), Some(east8));
        assert_eq!(parse_utc_offset("+08:00"), Some(east8));
        assert_eq!(parse_utc_offset("-0530"), FixedOffset::west_opt(5 * 3600 + 1800));
        assert_eq!(parse_utc_offset("+0000"), FixedOffset::east_opt(0));
    }

    #[test]
    fn rejects_malformed_offsets() {
        for bad in ["", "8", "0800", "+8", "+08", "+0860", "+2500", "UTC", "+08:00:00"] {
            assert_eq!(parse_utc_offset(bad), None, "{bad}");
        }
    }

    #[test]
    fn reset_time_follows_requested_timezone() {
        let now: DateTime<Utc> = "2026-11-05T08:00:00Z".parse().unwrap();
        let mut snapshot = crate::snapshot::Snapshot::new(
            crate::snapshot::Platform::Codex, "a", "dev", crate::snapshot::Source::Live, now);
        snapshot.windows.push(UsageWindow {
            name: "5h".into(),
            used_percent: Some(10.0),
            resets_at: Some("2026-11-05T12:00:00Z".parse().unwrap()),
        });
        let east8 = parse_utc_offset("+0800").unwrap();
        let west5 = parse_utc_offset("-0500").unwrap();
        assert!(remote_report(std::slice::from_ref(&snapshot), now, &east8).contains("11-05 20:00"));
        assert!(remote_report(std::slice::from_ref(&snapshot), now, &west5).contains("11-05 07:00"));
    }
}
