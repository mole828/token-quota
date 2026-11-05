//! Codex：优先查询实时用量；失败时退回本机会话记录里最近一次的限额快照。
//!
//! 实时接口未公开承诺兼容性：`GET https://chatgpt.com/backend-api/wham/usage`。
//! 会话记录：`~/.codex/sessions/**/rollout-*.jsonl` 中 `payload.rate_limits`。

use super::Context;
use crate::config::home_dir;
use crate::http;
use crate::snapshot::{Balance, Platform, Snapshot, Source, UsageWindow};
use anyhow::{Context as _, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<LiveRateLimit>,
    credits: Option<Credits>,
}

#[derive(Deserialize)]
struct LiveRateLimit {
    primary_window: Option<LiveWindow>,
    secondary_window: Option<LiveWindow>,
}

#[derive(Deserialize)]
struct LiveWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct Credits {
    has_credits: Option<bool>,
    unlimited: Option<bool>,
    balance: Option<String>,
}

#[derive(Deserialize)]
struct LogLine {
    timestamp: Option<DateTime<Utc>>,
    payload: Option<LogPayload>,
}

#[derive(Deserialize)]
struct LogPayload {
    rate_limits: Option<LogRateLimits>,
}

#[derive(Deserialize)]
struct LogRateLimits {
    primary: Option<LogWindow>,
    secondary: Option<LogWindow>,
    plan_type: Option<String>,
    credits: Option<Credits>,
}

#[derive(Deserialize)]
struct LogWindow {
    used_percent: Option<f64>,
    window_minutes: Option<i64>,
    resets_at: Option<i64>,
}

fn codex_home() -> PathBuf {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home_dir().join(".codex"),
    }
}

pub fn fetch(ctx: &Context) -> Result<Snapshot> {
    let account = ctx.config.account(Platform::Codex);
    let live_error = match fetch_live(ctx, &account) {
        Ok(snapshot) => return Ok(snapshot),
        Err(err) => err,
    };
    match latest_session_snapshot(&codex_home().join("sessions"), &account, &ctx.device) {
        Ok(snapshot) => Ok(snapshot),
        Err(log_error) => Err(anyhow!("实时查询失败：{live_error:#}；会话记录也不可用：{log_error:#}")),
    }
}

fn fetch_live(ctx: &Context, account: &str) -> Result<Snapshot> {
    let path = codex_home().join("auth.json");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("未找到 Codex 登录凭证：{}", path.display()))?;
    let auth: AuthFile = serde_json::from_str(&text).context("Codex 凭证格式无法解析")?;
    let Some(tokens) = auth.tokens else { bail!("Codex 未使用 ChatGPT 账号登录，没有用量额度可查") };

    let bearer = format!("Bearer {}", tokens.access_token);
    let mut headers = vec![("Authorization", bearer.as_str()), ("Accept", "application/json")];
    if let Some(id) = tokens.account_id.as_deref() {
        headers.push(("ChatGPT-Account-Id", id));
    }
    let reply = http::get(ctx.agent, USAGE_URL, &headers)?;
    if reply.status == 401 {
        bail!("Codex 登录已失效（HTTP 401），运行一次 codex 即可刷新");
    }
    let reply = reply.ensure_success("Codex 用量接口")?;
    parse_live(&reply.body, account, &ctx.device, Utc::now())
}

pub fn parse_live(body: &str, account: &str, device: &str, now: DateTime<Utc>) -> Result<Snapshot> {
    let usage: UsageResponse = serde_json::from_str(body).context("Codex 用量响应格式无法解析")?;
    let mut snapshot = Snapshot::new(Platform::Codex, account, device, Source::Live, now);
    snapshot.plan = usage.plan_type;

    let rate_limit = usage.rate_limit.ok_or_else(|| anyhow!("Codex 用量响应中没有 rate_limit"))?;
    for window in [rate_limit.primary_window, rate_limit.secondary_window].into_iter().flatten() {
        snapshot.windows.push(UsageWindow {
            name: window_name(window.limit_window_seconds),
            used_percent: window.used_percent,
            resets_at: window.reset_at.and_then(from_unix),
        });
    }
    snapshot.balances.extend(usage.credits.and_then(credits_balance));
    Ok(snapshot)
}

/// 在会话记录目录里找最近修改的文件，取其中最后一条带限额的记录。
pub fn latest_session_snapshot(sessions_dir: &Path, account: &str, device: &str) -> Result<Snapshot> {
    let mut files = Vec::new();
    collect_rollout_files(sessions_dir, &mut files);
    files.sort_by_key(|(modified, _)| *modified);

    // 最新的文件可能还没有限额记录（会话刚开始），所以依次往前找。
    for (_, path) in files.iter().rev().take(20) {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        if let Some(snapshot) = parse_session_log(&text, account, device) {
            return Ok(snapshot);
        }
    }
    bail!("{} 下没有包含限额信息的会话记录", sessions_dir.display())
}

fn collect_rollout_files(dir: &Path, out: &mut Vec<(SystemTime, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollout_files(&path, out);
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("rollout-") && name.ends_with(".jsonl") {
            let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((modified, path));
        }
    }
}

/// 返回文件中最后一条带限额信息的记录对应的快照。
pub fn parse_session_log(text: &str, account: &str, device: &str) -> Option<Snapshot> {
    for line in text.lines().rev() {
        if !line.contains("\"rate_limits\"") {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<LogLine>(line) else { continue };
        let (Some(timestamp), Some(limits)) = (parsed.timestamp, parsed.payload.and_then(|p| p.rate_limits))
        else {
            continue;
        };

        let mut snapshot = Snapshot::new(Platform::Codex, account, device, Source::SessionLog, timestamp);
        snapshot.plan = limits.plan_type;
        for window in [limits.primary, limits.secondary].into_iter().flatten() {
            snapshot.windows.push(UsageWindow {
                name: window_name(window.window_minutes.map(|m| m * 60)),
                used_percent: window.used_percent,
                resets_at: window.resets_at.and_then(from_unix),
            });
        }
        snapshot.balances.extend(limits.credits.and_then(credits_balance));
        return Some(snapshot);
    }
    None
}

/// 只有账号确实有 credits 时才展示，避免把"没有开通"显示成"余额 0"。
fn credits_balance(credits: Credits) -> Option<Balance> {
    if credits.unlimited == Some(true) {
        return Some(Balance { name: "credits（不限）".into(), total: None, used: None, remaining: None, unit: "credits".into() });
    }
    if credits.has_credits != Some(true) {
        return None;
    }
    Some(Balance {
        name: "credits".into(),
        total: None,
        used: None,
        remaining: credits.balance.and_then(|b| b.parse().ok()),
        unit: "credits".into(),
    })
}

/// 18000 秒 → "5h"，604800 秒 → "7d"。
fn window_name(seconds: Option<i64>) -> String {
    match seconds {
        Some(s) if s > 0 && s % 86_400 == 0 => format!("{}d", s / 86_400),
        Some(s) if s > 0 && s % 3_600 == 0 => format!("{}h", s / 3_600),
        Some(s) if s > 0 => format!("{}m", s / 60),
        _ => "window".to_string(),
    }
}

fn from_unix(seconds: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(seconds, 0)
}
