//! 用真实响应结构（已替换个人信息）验证各平台解析。

use chrono::{DateTime, Utc};
use token_quota::providers::{claude, codex, qoder};
use token_quota::snapshot::{Platform, Source};

fn now() -> DateTime<Utc> {
    "2026-11-05T08:00:00Z".parse().unwrap()
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn claude_usage_keeps_only_present_windows() {
    let s = claude::parse_usage(&fixture("claude_usage.json"), "me", "dev", now()).unwrap();
    assert_eq!(s.platform, Platform::Claude);
    let names: Vec<_> = s.windows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["5h", "7d"]);
    assert_eq!(s.windows[0].used_percent, Some(6.0));
    assert_eq!(s.windows[1].resets_at.unwrap().to_rfc3339(), "2026-11-07T02:00:00.415835+00:00");
}

#[test]
fn codex_live_usage_names_windows_by_length() {
    let s = codex::parse_live(&fixture("codex_usage.json"), "me", "dev", now()).unwrap();
    assert_eq!(s.source, Source::Live);
    assert_eq!(s.plan.as_deref(), Some("plus"));
    assert_eq!(s.windows[0].name, "5h");
    assert_eq!(s.windows[1].name, "7d");
    assert_eq!(s.windows[1].used_percent, Some(58.0));
    assert_eq!(s.windows[0].resets_at.unwrap().timestamp(), 1793881116);
    // 没有开通 credits 时不显示余额，而不是显示 0。
    assert!(s.balances.is_empty());
}

#[test]
fn codex_session_log_uses_last_rate_limit_entry() {
    let s = codex::parse_session_log(&fixture("codex_rollout.jsonl"), "me", "dev").unwrap();
    assert_eq!(s.source, Source::SessionLog);
    assert_eq!(s.fetched_at.to_rfc3339(), "2026-11-05T07:19:22.893+00:00");
    assert_eq!(s.windows[0].used_percent, Some(9.0));
}

#[test]
fn codex_session_log_without_rate_limits_is_none() {
    let text = r#"{"timestamp":"2026-11-05T07:00:00Z","type":"session_meta","payload":{}}"#;
    assert!(codex::parse_session_log(text, "me", "dev").is_none());
}

#[test]
fn qoder_quota_maps_buckets() {
    let s = qoder::parse_quota(&fixture("qoder_quota.json"), "me", "dev", now()).unwrap();
    assert_eq!(s.plan.as_deref(), Some("personal_standard"));
    let add_on = s.balances.iter().find(|b| b.name == "附加").unwrap();
    assert_eq!((add_on.total, add_on.used, add_on.remaining), (Some(1000.0), Some(13.0), Some(987.0)));
}

#[test]
fn qoder_quota_accepts_snake_case() {
    let body = r#"{"user_type":"x","add_on_quota":{"total":10,"used":1,"remaining":9}}"#;
    let s = qoder::parse_quota(body, "me", "dev", now()).unwrap();
    assert_eq!(s.balances[0].remaining, Some(9.0));
    assert_eq!(s.balances[0].unit, "credits");
}

#[test]
fn qoder_exchange_token_at_top_level_or_in_data() {
    assert_eq!(qoder::parse_exchange(r#"{"token":"abc"}"#).unwrap(), "abc");
    assert_eq!(qoder::parse_exchange(r#"{"data":{"token":"abc"}}"#).unwrap(), "abc");
    assert!(qoder::parse_exchange(r#"{"data":{}}"#).is_err());
}
