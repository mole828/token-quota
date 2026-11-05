//! Claude Code：读取本机登录凭证，查询 OAuth 用量接口。
//!
//! 接口未公开承诺兼容性：`GET https://api.anthropic.com/api/oauth/usage`，
//! 需要请求头 `anthropic-beta: oauth-2025-04-20`。
//! 不刷新 token，避免和 Claude Code 同时改写凭证文件。

use super::Context;
use crate::config::home_dir;
use crate::http;
use crate::snapshot::{Platform, Snapshot, Source, UsageWindow};
use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OAuth>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuth {
    access_token: String,
    /// 毫秒时间戳。
    expires_at: Option<i64>,
    subscription_type: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    seven_day_opus: Option<Window>,
    seven_day_sonnet: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    utilization: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
}

fn credentials_path() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir).join(".credentials.json"),
        None => home_dir().join(".claude").join(".credentials.json"),
    }
}

pub fn fetch(ctx: &Context) -> Result<Snapshot> {
    let path = credentials_path();
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("未找到 Claude Code 登录凭证：{}", path.display()))?;
    let file: CredentialsFile = serde_json::from_str(&text).context("Claude Code 凭证格式无法解析")?;
    let Some(oauth) = file.oauth else { bail!("Claude Code 未使用账号登录（凭证中没有 claudeAiOauth）") };

    let now = Utc::now();
    if let Some(expires_ms) = oauth.expires_at
        && expires_ms <= now.timestamp_millis()
    {
        bail!("Claude Code 登录已过期，运行一次 claude 即可刷新");
    }

    let auth = format!("Bearer {}", oauth.access_token);
    let reply = http::get(
        ctx.agent,
        USAGE_URL,
        &[
            ("Authorization", &auth),
            ("anthropic-beta", "oauth-2025-04-20"),
            ("Accept", "application/json"),
        ],
    )?;
    if reply.status == 401 {
        bail!("Claude Code 登录已失效（HTTP 401），运行一次 claude 即可刷新");
    }
    let reply = reply.ensure_success("Claude 用量接口")?;

    let mut snapshot = parse_usage(
        &reply.body,
        &ctx.config.account(Platform::Claude),
        &ctx.device,
        now,
    )?;
    snapshot.plan = oauth.subscription_type;
    Ok(snapshot)
}

pub fn parse_usage(body: &str, account: &str, device: &str, now: DateTime<Utc>) -> Result<Snapshot> {
    let usage: UsageResponse = serde_json::from_str(body).context("Claude 用量响应格式无法解析")?;
    let mut snapshot = Snapshot::new(Platform::Claude, account, device, Source::Live, now);

    let named = [
        ("5h", usage.five_hour),
        ("7d", usage.seven_day),
        ("7d opus", usage.seven_day_opus),
        ("7d sonnet", usage.seven_day_sonnet),
    ];
    for (name, window) in named {
        if let Some(window) = window {
            snapshot.windows.push(UsageWindow {
                name: name.to_string(),
                used_percent: window.utilization,
                resets_at: window.resets_at,
            });
        }
    }
    if snapshot.windows.is_empty() {
        bail!("Claude 用量响应中没有任何额度窗口");
    }
    Ok(snapshot)
}
