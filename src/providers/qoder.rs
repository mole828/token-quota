//! Qoder CN：用个人访问令牌（PAT）换取短期 token，再查询额度和每日领取活动。
//!
//! 这些接口来自 qoderclicn 的实现和社区脚本，未公开承诺兼容性：
//! - `POST /api/v1/jobToken/exchange`，请求体 `{"personal_token": PAT}`，返回 24 小时有效的 token
//! - `GET  /api/v2/quota/usage`
//! - `GET  /sash/api/v1/me/campaigns`（必须带 `Cosy-ClientType: 10`，否则返回空列表）
//! - `POST /sash/api/v1/me/campaigns/{id}/claim`（服务端幂等）

use super::Context;
use crate::http;
use crate::snapshot::{Balance, Platform, Snapshot, Source};
use anyhow::{Context as _, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BASE_URL: &str = "https://openapi.qoder.com.cn";
const CLIENT_TYPE: (&str, &str) = ("Cosy-ClientType", "10");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaUsage {
    #[serde(alias = "user_type")]
    user_type: Option<String>,
    #[serde(alias = "user_quota")]
    user_quota: Option<Bucket>,
    #[serde(alias = "add_on_quota")]
    add_on_quota: Option<Bucket>,
    #[serde(alias = "org_resource_package")]
    org_resource_package: Option<Bucket>,
}

#[derive(Deserialize)]
struct Bucket {
    total: Option<f64>,
    /// 组织资源包用 cap 表示总量。
    cap: Option<f64>,
    used: Option<f64>,
    remaining: Option<f64>,
    unit: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampaignList {
    #[serde(default)]
    pub campaigns: Vec<Campaign>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Campaign {
    pub campaign_id: String,
    pub campaign_key: Option<String>,
    pub action_type: Option<String>,
    pub claim_status: Option<String>,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub benefit: Option<Benefit>,
}

#[derive(Debug, Deserialize)]
pub struct Benefit {
    pub amount: Option<f64>,
}

impl Campaign {
    fn is_claim_benefit(&self) -> bool {
        self.action_type.as_deref() == Some("CLAIM_BENEFIT")
    }

    fn is_active(&self, now: DateTime<Utc>) -> bool {
        let now = now.timestamp();
        match (self.start_at, self.end_at) {
            (Some(start), Some(end)) => start <= now && now < end,
            _ => false,
        }
    }
}

/// 一个领取活动在本次运行中的结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum ClaimResult {
    /// 本次领取成功。
    Claimed { amount: Option<f64> },
    /// 本轮已经领过。
    AlreadyClaimed,
    /// 活动进行中，但状态既不是可领取也不是已领取。
    NotClaimable { status: String },
    /// 可以领取，但这是 dry run，没有发出领取请求。
    WouldClaim { amount: Option<f64> },
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaimReport {
    pub campaign_key: String,
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub result: ClaimResult,
}

/// 已换取的短期 token。只保存在内存中。
pub struct Session<'a> {
    agent: &'a ureq::Agent,
    bearer: String,
}

impl<'a> Session<'a> {
    pub fn login(ctx: &Context<'a>) -> Result<Session<'a>> {
        let pat = ctx.config.qoder_pat()?;
        let reply = http::post_json(
            ctx.agent,
            &format!("{BASE_URL}/api/v1/jobToken/exchange"),
            &[("Accept", "application/json")],
            &serde_json::json!({ "personal_token": pat }),
        )?;
        if matches!(reply.status, 400 | 401 | 403) {
            bail!("Qoder CN 个人访问令牌无效或已过期（HTTP {}），请在 https://qoder.cn/account/integrations 重新创建", reply.status);
        }
        let reply = reply.ensure_success("Qoder 令牌兑换")?;
        let token = parse_exchange(&reply.body)?;
        Ok(Session { agent: ctx.agent, bearer: format!("Bearer {token}") })
    }

    fn get(&self, path: &str, what: &str) -> Result<http::Reply> {
        let headers = [("Authorization", self.bearer.as_str()), ("Accept", "application/json"), CLIENT_TYPE];
        let reply = http::get(self.agent, &format!("{BASE_URL}{path}"), &headers)?;
        check_auth(reply, what)
    }

    pub fn quota(&self, account: &str, device: &str) -> Result<Snapshot> {
        let reply = self.get("/api/v2/quota/usage", "Qoder 额度接口")?;
        parse_quota(&reply.body, account, device, Utc::now())
    }

    pub fn campaigns(&self) -> Result<CampaignList> {
        let reply = self.get("/sash/api/v1/me/campaigns", "Qoder 活动接口")?;
        reply.json("Qoder 活动接口")
    }

    /// 查询当前活动，按需领取。`dry_run` 时只报告，不发领取请求。
    pub fn claim_daily(&self, dry_run: bool) -> Result<Vec<ClaimReport>> {
        let now = Utc::now();
        let list = self.campaigns()?;
        let mut reports = Vec::new();
        for campaign in list.campaigns.iter().filter(|c| c.is_claim_benefit() && c.is_active(now)) {
            let amount = campaign.benefit.as_ref().and_then(|b| b.amount);
            let result = match campaign.claim_status.as_deref() {
                Some("CLAIMED") => ClaimResult::AlreadyClaimed,
                Some("CLAIMABLE") if dry_run => ClaimResult::WouldClaim { amount },
                Some("CLAIMABLE") => self.claim(&campaign.campaign_id, amount)?,
                other => ClaimResult::NotClaimable { status: other.unwrap_or("未知").to_string() },
            };
            reports.push(ClaimReport {
                campaign_key: campaign.campaign_key.clone().unwrap_or_else(|| campaign.campaign_id.clone()),
                ends_at: campaign.end_at.and_then(|t| DateTime::from_timestamp(t, 0)),
                result,
            });
        }
        Ok(reports)
    }

    fn claim(&self, campaign_id: &str, fallback_amount: Option<f64>) -> Result<ClaimResult> {
        let headers = [("Authorization", self.bearer.as_str()), ("Accept", "application/json"), CLIENT_TYPE];
        let url = format!("{BASE_URL}/sash/api/v1/me/campaigns/{campaign_id}/claim");
        let reply = check_auth(http::post_empty(self.agent, &url, &headers)?, "Qoder 领取接口")?;
        let body: Value = reply.json("Qoder 领取接口")?;
        // 响应可能包在 data 里。
        let data = body.get("data").unwrap_or(&body);
        match data.get("status").and_then(Value::as_str) {
            Some("CLAIMED") => {
                let amount = data
                    .pointer("/benefit/amount")
                    .or_else(|| data.get("amount"))
                    .and_then(Value::as_f64)
                    .or(fallback_amount);
                Ok(ClaimResult::Claimed { amount })
            }
            other => bail!("Qoder 领取未成功，返回状态：{}", other.unwrap_or("未知")),
        }
    }
}

fn check_auth(reply: http::Reply, what: &str) -> Result<http::Reply> {
    match reply.status {
        401 => bail!("{what}：认证失败（HTTP 401），个人访问令牌可能已失效"),
        403 => bail!("{what}：没有权限（HTTP 403），可能是账号类型不支持或触发了风控"),
        _ => reply.ensure_success(what),
    }
}

pub fn fetch(ctx: &Context) -> Result<Snapshot> {
    let session = Session::login(ctx)?;
    session.quota(&ctx.config.account(Platform::Qoder), &ctx.device)
}

/// 兑换接口的 token 可能在顶层，也可能在 `data` 里。
pub fn parse_exchange(body: &str) -> Result<String> {
    let value: Value = serde_json::from_str(body).context("Qoder 令牌兑换响应格式无法解析")?;
    let data = value.get("data").unwrap_or(&value);
    ["token", "access_token", "device_token"]
        .iter()
        .find_map(|key| data.get(*key).and_then(Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Qoder 令牌兑换响应中没有 token"))
}

pub fn parse_quota(body: &str, account: &str, device: &str, now: DateTime<Utc>) -> Result<Snapshot> {
    let usage: QuotaUsage = serde_json::from_str(body).context("Qoder 额度响应格式无法解析")?;
    let mut snapshot = Snapshot::new(Platform::Qoder, account, device, Source::Live, now);
    snapshot.plan = usage.user_type;

    let buckets = [("套餐", usage.user_quota), ("附加", usage.add_on_quota), ("组织", usage.org_resource_package)];
    for (name, bucket) in buckets {
        if let Some(bucket) = bucket {
            snapshot.balances.push(Balance {
                name: name.to_string(),
                total: bucket.total.or(bucket.cap),
                used: bucket.used,
                remaining: bucket.remaining,
                unit: bucket.unit.unwrap_or_else(|| "credits".to_string()),
            });
        }
    }
    if snapshot.balances.is_empty() {
        bail!("Qoder 额度响应中没有任何额度信息");
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campaign_activity_window_is_half_open() {
        let campaign = Campaign {
            campaign_id: "c".into(),
            campaign_key: None,
            action_type: Some("CLAIM_BENEFIT".into()),
            claim_status: Some("CLAIMABLE".into()),
            start_at: Some(100),
            end_at: Some(200),
            benefit: None,
        };
        let at = |t| DateTime::from_timestamp(t, 0).unwrap();
        assert!(!campaign.is_active(at(99)));
        assert!(campaign.is_active(at(100)));
        assert!(campaign.is_active(at(199)));
        assert!(!campaign.is_active(at(200)));
    }
}
