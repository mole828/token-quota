//! `tq-server`：接收各设备上报的额度快照，提供汇总查询。
//!
//! 只保存额度数字，不接触任何平台凭证。
//!
//! - `POST /v1/snapshots`：请求体为快照数组，需要 `Authorization: Bearer <令牌>`
//! - `GET  /v1/state`：返回每个"平台 + 账号"的最新快照，同样需要令牌
//! - `GET  /v1/text`：同上，渲染成终端文本，方便没有 `tq` 的设备直接用 curl 查看；
//!   时区用 `?tz=+0800` 指定（即 `date +%z` 的输出），默认 `--display-tz`
//! - `GET  /healthz`：健康检查，不需要令牌

use anyhow::{Context, Result, bail};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, FixedOffset, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use token_quota::render;
use token_quota::snapshot::Snapshot;
use token_quota::store::{MergeOutcome, Store};

#[derive(Parser)]
#[command(name = "tq-server", version, about = "Token Quota 上报服务器")]
struct Args {
    /// 监听地址
    #[arg(long, env = "TQ_LISTEN", default_value = "127.0.0.1:8787")]
    listen: String,
    /// 状态文件路径
    #[arg(long, env = "TQ_DATA", default_value = "tq-state.json")]
    data: PathBuf,
    /// 允许访问的令牌，逗号分隔；每台设备可以用不同令牌，便于单独吊销
    #[arg(long, env = "TQ_TOKENS", hide_env_values = true)]
    tokens: String,
    /// /v1/text 默认显示时区，格式同 `date +%z`，例如 +0800
    #[arg(long, env = "TQ_DISPLAY_TZ", default_value = "+0800", allow_hyphen_values = true)]
    display_tz: String,
}

#[derive(Clone)]
struct AppState {
    store: Arc<Mutex<Store>>,
    data_path: Arc<PathBuf>,
    tokens: Arc<Vec<String>>,
    display_tz: FixedOffset,
}

type ApiError = (StatusCode, String);

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let tokens: Vec<String> =
        args.tokens.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        bail!("TQ_TOKENS 不能为空");
    }
    if let Some(short) = tokens.iter().find(|t| t.len() < 16) {
        bail!("令牌太短（{} 个字符），至少需要 16 个字符", short.len());
    }

    let display_tz = render::parse_utc_offset(&args.display_tz).context("--display-tz 格式应为 +0800")?;
    let store = Store::load(&args.data)?;
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        data_path: Arc::new(args.data.clone()),
        tokens: Arc::new(tokens),
        display_tz,
    };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/snapshots", post(post_snapshots))
        .route("/v1/state", get(get_state))
        .route("/v1/text", get(get_text))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("无法监听 {}", args.listen))?;
    println!("tq-server 监听 {}，状态文件 {}", args.listen, args.data.display());
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

#[derive(Serialize)]
struct PushResponse {
    stored: usize,
    outdated: usize,
    rejected: usize,
}

async fn post_snapshots(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(batch): Json<Vec<Snapshot>>,
) -> Result<Json<PushResponse>, ApiError> {
    authorize(&headers, &state.tokens)?;

    let mut store = state.store.lock().expect("store lock poisoned");
    let mut response = PushResponse { stored: 0, outdated: 0, rejected: 0 };
    for snapshot in batch {
        match store.merge(snapshot) {
            MergeOutcome::Stored => response.stored += 1,
            MergeOutcome::Outdated => response.outdated += 1,
            MergeOutcome::UnsupportedSchema => response.rejected += 1,
        }
    }
    if response.stored > 0 {
        store.save(&state.data_path).map_err(|err| {
            eprintln!("保存状态失败：{err:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "保存状态失败".to_string())
        })?;
    }
    Ok(Json(response))
}

#[derive(Serialize)]
struct StateResponse {
    server_time: DateTime<Utc>,
    snapshots: Vec<Snapshot>,
}

async fn get_state(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<StateResponse>, ApiError> {
    authorize(&headers, &state.tokens)?;
    let snapshots = state.store.lock().expect("store lock poisoned").snapshots();
    Ok(Json(StateResponse { server_time: Utc::now(), snapshots }))
}

#[derive(Deserialize)]
struct TextQuery {
    /// UTC 偏移，例如 +0800、-05:30。
    tz: Option<String>,
}

async fn get_text(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TextQuery>,
) -> Result<([(header::HeaderName, &'static str); 1], String), ApiError> {
    authorize(&headers, &state.tokens)?;
    let tz = match query.tz {
        Some(text) => render::parse_utc_offset(&text)
            .ok_or((StatusCode::BAD_REQUEST, "tz 格式应为 +0800（即 date +%z 的输出）\n".to_string()))?,
        None => state.display_tz,
    };
    let snapshots = state.store.lock().expect("store lock poisoned").snapshots();
    let text = render::remote_report(&snapshots, Utc::now(), &tz);
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}

fn authorize(headers: &HeaderMap, tokens: &[String]) -> Result<(), ApiError> {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if tokens.iter().any(|t| constant_time_eq(t.as_bytes(), presented.as_bytes())) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "令牌无效\n".to_string()))
    }
}

/// 比较耗时不随第一个不同字节的位置变化，避免通过响应时间猜令牌。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
