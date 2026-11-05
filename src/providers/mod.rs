//! 各平台的额度采集。每个平台模块把接口响应解析成统一的 [`Snapshot`]。
//!
//! 解析函数（`parse_*`）不做网络请求，便于用固定样例测试。

pub mod claude;
pub mod codex;
pub mod qoder;

use crate::config::Config;
use crate::snapshot::{Collected, Platform, Snapshot};
use anyhow::Result;
use ureq::Agent;

/// 采集时需要的公共信息。
pub struct Context<'a> {
    pub agent: &'a Agent,
    pub config: &'a Config,
    pub device: String,
}

impl<'a> Context<'a> {
    pub fn new(agent: &'a Agent, config: &'a Config) -> Self {
        Context { agent, config, device: config.device() }
    }
}

pub fn fetch(platform: Platform, ctx: &Context) -> Result<Snapshot> {
    match platform {
        Platform::Claude => claude::fetch(ctx),
        Platform::Codex => codex::fetch(ctx),
        Platform::Qoder => qoder::fetch(ctx),
    }
}

pub fn collect(platform: Platform, ctx: &Context) -> Collected {
    let account = ctx.config.account(platform);
    match fetch(platform, ctx) {
        Ok(snapshot) => Collected { platform, account, snapshot: Some(snapshot), error: None },
        Err(err) => Collected { platform, account, snapshot: None, error: Some(format!("{err:#}")) },
    }
}
