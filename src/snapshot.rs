//! 统一的额度快照格式。客户端输出 JSON、上报服务器、服务器存储都使用它。
//!
//! 约定：未知的数值一律为 `None`，不写成 0。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// 快照格式版本。字段含义变化时递增，服务器拒绝不认识的版本。
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Claude,
    Codex,
    Qoder,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::Claude, Platform::Codex, Platform::Qoder];

    pub fn id(self) -> &'static str {
        match self {
            Platform::Claude => "claude",
            Platform::Codex => "codex",
            Platform::Qoder => "qoder",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Platform::Claude => "Claude Code",
            Platform::Codex => "Codex",
            Platform::Qoder => "Qoder CN",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// 数据来源：实时接口，或本机会话记录里的历史快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Live,
    SessionLog,
}

/// 按时间窗口限流的额度，例如 Claude 的 5 小时 / 7 天窗口。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub name: String,
    pub used_percent: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
}

/// 按数量计的额度，例如 Qoder 的 credits。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Balance {
    pub name: String,
    pub total: Option<f64>,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u32,
    pub platform: Platform,
    /// 用户在配置里给账号起的别名，不是邮箱或平台账号 ID。
    pub account: String,
    pub device: String,
    pub source: Source,
    /// 数据本身的时间。实时接口为请求时间；会话记录为记录写入时间。
    pub fetched_at: DateTime<Utc>,
    pub plan: Option<String>,
    #[serde(default)]
    pub windows: Vec<UsageWindow>,
    #[serde(default)]
    pub balances: Vec<Balance>,
}

impl Snapshot {
    pub fn new(
        platform: Platform,
        account: &str,
        device: &str,
        source: Source,
        fetched_at: DateTime<Utc>,
    ) -> Self {
        Snapshot {
            schema: SCHEMA_VERSION,
            platform,
            account: account.to_string(),
            device: device.to_string(),
            source,
            fetched_at,
            plan: None,
            windows: Vec::new(),
            balances: Vec::new(),
        }
    }
}

/// 一个平台的查询结果：成功得到快照，或失败原因。用于本机展示和 JSON 输出。
#[derive(Debug, Clone, Serialize)]
pub struct Collected {
    pub platform: Platform,
    pub account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
