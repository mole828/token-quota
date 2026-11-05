//! `tq`：本机额度查询、Qoder 每日领取、上报服务器、查看服务器汇总。

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::ExitCode;
use token_quota::config::{Config, cache_dir, config_dir, write_atomic};
use token_quota::http;
use token_quota::providers::{self, Context, qoder};
use token_quota::render;
use token_quota::snapshot::{Collected, Platform, Snapshot};

/// 本机查询结果的缓存时长，避免频繁请求各平台接口。
const CACHE_TTL_SECONDS: i64 = 60;

#[derive(Parser)]
#[command(name = "tq", version, about = "查看 Claude Code、Codex、Qoder CN 的剩余额度")]
struct Cli {
    /// 不带子命令时：以 JSON 输出服务器汇总
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// 查询本机各平台额度
    #[command(visible_alias = "show")]
    Local(ShowArgs),
    /// 查询本机额度并上报到服务器
    Push {
        /// 忽略本机缓存，重新查询
        #[arg(long)]
        refresh: bool,
    },
    /// 查看服务器上汇总的各设备额度（默认命令）
    Remote {
        #[arg(long)]
        json: bool,
    },
    /// 配置上报服务器：从标准输入读取令牌，验证后写入本机配置
    Setup {
        /// 服务器地址，例如 https://tq.molec.cc
        url: String,
    },
    /// Qoder CN 每日活动
    Qoder {
        #[command(subcommand)]
        command: QoderCommand,
    },
}

#[derive(clap::Args, Default)]
struct ShowArgs {
    /// 输出 JSON
    #[arg(long)]
    json: bool,
    /// 忽略本机缓存，重新查询
    #[arg(long)]
    refresh: bool,
    /// 只查询指定平台
    #[arg(long, value_enum)]
    only: Option<PlatformArg>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum PlatformArg {
    Claude,
    Codex,
    Qoder,
}

impl From<PlatformArg> for Platform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Claude => Platform::Claude,
            PlatformArg::Codex => Platform::Codex,
            PlatformArg::Qoder => Platform::Qoder,
        }
    }
}

#[derive(Subcommand)]
enum QoderCommand {
    /// 查看当前领取活动和领取状态
    Status {
        #[arg(long)]
        json: bool,
    },
    /// 领取当前可领的每日额度；已领取时跳过
    Claim {
        /// 只检查，不发出领取请求
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("错误：{err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let config = Config::load()?;
    let agent = http::agent();
    let ctx = Context::new(&agent, &config);

    let command = match cli.command {
        Some(command) => command,
        None => default_command(&config, cli.json),
    };
    match command {
        Command::Local(args) => show(&ctx, args),
        Command::Push { refresh } => push(&ctx, refresh),
        Command::Remote { json } => remote(&ctx, json),
        Command::Setup { url } => setup(&agent, &url),
        Command::Qoder { command: QoderCommand::Status { json } } => qoder_claim(&ctx, true, json),
        Command::Qoder { command: QoderCommand::Claim { dry_run, json } } => qoder_claim(&ctx, dry_run, json),
    }
}

/// 默认查看服务器汇总；还没配置服务器时退回本机查询。
fn default_command(config: &Config, json: bool) -> Command {
    if config.server_url().is_ok() {
        return Command::Remote { json };
    }
    eprintln!("提示：未配置服务器，显示本机额度。运行 tq setup <服务器地址> 后，tq 默认显示各设备汇总。\n");
    Command::Local(ShowArgs { json, ..ShowArgs::default() })
}

fn show(ctx: &Context, args: ShowArgs) -> Result<ExitCode> {
    let platforms: Vec<Platform> = match args.only {
        Some(p) => vec![p.into()],
        None => Platform::ALL.to_vec(),
    };
    let results = collect_with_cache(ctx, &platforms, args.refresh);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print!("{}", render::local_report(&results, Utc::now(), &chrono::Local));
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Deserialize)]
struct PushResponse {
    stored: usize,
    outdated: usize,
    rejected: usize,
}

fn push(ctx: &Context, refresh: bool) -> Result<ExitCode> {
    let url = ctx.config.server_url()?;
    let token = ctx.config.server_token()?;

    let results = collect_with_cache(ctx, &Platform::ALL, refresh);
    // 只上报查询成功的快照；失败原因只在本机显示。
    for failed in results.iter().filter(|r| r.snapshot.is_none()) {
        eprintln!("跳过 {}：{}", failed.platform.display_name(), failed.error.as_deref().unwrap_or("未知错误"));
    }
    let snapshots: Vec<Snapshot> = results.into_iter().filter_map(|r| r.snapshot).collect();
    if snapshots.is_empty() {
        bail!("没有可上报的数据");
    }

    let bearer = format!("Bearer {token}");
    let reply = http::post_json(ctx.agent, &format!("{url}/v1/snapshots"), &[("Authorization", &bearer)], &snapshots)?
        .ensure_success("上报服务器")?;
    let summary: PushResponse = reply.json("上报服务器")?;
    println!("已上报 {} 条：更新 {}，已有更新数据 {}，被拒绝 {}", snapshots.len(), summary.stored, summary.outdated, summary.rejected);
    Ok(ExitCode::SUCCESS)
}

#[derive(Deserialize)]
struct StateResponse {
    server_time: DateTime<Utc>,
    snapshots: Vec<Snapshot>,
}

fn remote(ctx: &Context, json: bool) -> Result<ExitCode> {
    let url = ctx.config.server_url()?;
    let token = ctx.config.server_token()?;
    let bearer = format!("Bearer {token}");
    let reply = http::get(ctx.agent, &format!("{url}/v1/state"), &[("Authorization", &bearer)])?
        .ensure_success("读取服务器状态")?;
    if json {
        println!("{}", reply.body);
        return Ok(ExitCode::SUCCESS);
    }
    let state: StateResponse = reply.json("读取服务器状态")?;
    // 用服务器时间计算"多久前"，避免本机时钟偏差。
    print!("{}", render::remote_report(&state.snapshots, state.server_time, &chrono::Local));
    Ok(ExitCode::SUCCESS)
}

/// 令牌从标准输入读取，避免留在 shell 历史里。
fn setup(agent: &ureq::Agent, url: &str) -> Result<ExitCode> {
    let url = url.trim_end_matches('/');
    let config_path = config_dir().join("config.toml");
    let token_path = config_dir().join("server_token");

    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == "[server]") {
        bail!("{} 中已有 [server] 配置，请直接编辑该文件", config_path.display());
    }

    eprint!("粘贴服务器令牌后回车：");
    let mut token = String::new();
    std::io::stdin().read_line(&mut token)?;
    let token = token.trim();
    if token.is_empty() {
        bail!("令牌为空");
    }

    let bearer = format!("Bearer {token}");
    http::get(agent, &format!("{url}/v1/state"), &[("Authorization", &bearer)])?.ensure_success("验证服务器令牌")?;

    write_private(&token_path, token)?;
    let section = format!("\n[server]\nurl = \"{url}\"\ntoken_file = \"{}\"\n", token_path.display());
    write_atomic(&config_path, format!("{existing}{section}").as_bytes())?;
    println!("已写入 {}，现在可以运行 tq remote 查看各设备额度。", config_path.display());
    Ok(ExitCode::SUCCESS)
}

/// 写入只有当前用户可读的文件。
fn write_private(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("写入失败：{}", path.display()))?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

fn qoder_claim(ctx: &Context, dry_run: bool, json: bool) -> Result<ExitCode> {
    let session = qoder::Session::login(ctx)?;
    let reports = session.claim_daily(dry_run)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(ExitCode::SUCCESS);
    }
    if reports.is_empty() {
        println!("当前没有进行中的领取活动。");
    }
    for report in &reports {
        let amount = |a: &Option<f64>| a.map(|v| format!(" {v} credits")).unwrap_or_default();
        let line = match &report.result {
            qoder::ClaimResult::Claimed { amount: a } => format!("已领取{}", amount(a)),
            qoder::ClaimResult::AlreadyClaimed => "本轮已领取，跳过".to_string(),
            qoder::ClaimResult::WouldClaim { amount: a } => format!("可以领取{}（未领取）", amount(a)),
            qoder::ClaimResult::NotClaimable { status } => format!("暂不可领取（状态：{status}）"),
        };
        let ends = report
            .ends_at
            .map(|t| format!("，本轮截止 {}", t.with_timezone(&chrono::Local).format("%m-%d %H:%M")))
            .unwrap_or_default();
        println!("{}：{line}{ends}", report.campaign_key);
    }
    Ok(ExitCode::SUCCESS)
}

// ---- 本机短时缓存 ----

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    cached_at: DateTime<Utc>,
    snapshot: Snapshot,
}

fn cache_path() -> PathBuf {
    cache_dir().join("local.json")
}

fn read_cache() -> Vec<CacheEntry> {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_cache(entries: &[CacheEntry]) -> Result<()> {
    write_atomic(&cache_path(), &serde_json::to_vec(entries)?).context("写入缓存失败")
}

/// 缓存未过期的平台直接用缓存，其余重新查询。查询失败不写缓存。
fn collect_with_cache(ctx: &Context, platforms: &[Platform], refresh: bool) -> Vec<Collected> {
    let now = Utc::now();
    let mut cache = read_cache();
    cache.retain(|e| now - e.cached_at < Duration::seconds(CACHE_TTL_SECONDS));

    let mut results = Vec::new();
    for &platform in platforms {
        let account = ctx.config.account(platform);
        let cached = cache.iter().find(|e| e.snapshot.platform == platform && e.snapshot.account == account);
        match cached {
            Some(entry) if !refresh => results.push(Collected {
                platform,
                account,
                snapshot: Some(entry.snapshot.clone()),
                error: None,
            }),
            _ => results.push(providers::collect(platform, ctx)),
        }
    }

    for result in &results {
        if let Some(snapshot) = &result.snapshot {
            cache.retain(|e| !(e.snapshot.platform == snapshot.platform && e.snapshot.account == snapshot.account));
            cache.push(CacheEntry { cached_at: now, snapshot: snapshot.clone() });
        }
    }
    if let Err(err) = write_cache(&cache) {
        eprintln!("警告：{err:#}");
    }
    results
}
