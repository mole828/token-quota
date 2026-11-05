//! Token Quota：集中查看 Claude Code、Codex、Qoder CN 的剩余额度。
//!
//! - `tq` 二进制：在本机查询、领取 Qoder 每日额度、上报到服务器、查看服务器汇总。
//! - `tq-server` 二进制：接收各设备上报的快照，按"平台 + 账号"保留最新一条。

pub mod config;
pub mod http;
pub mod providers;
pub mod render;
pub mod snapshot;
pub mod store;
