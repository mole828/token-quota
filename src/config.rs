//! 本机配置：`~/.config/token-quota/config.toml`（可用环境变量 `TQ_CONFIG` 指定其他路径）。
//!
//! 所有字段都可省略，例如：
//!
//! ```toml
//! device = "laptop"            # 默认取主机名
//!
//! [accounts]                   # 账号别名，默认都是 "default"
//! claude = "personal"
//!
//! [qoder]
//! pat_file = "~/.config/token-quota/qodercn_pat"   # 也可用环境变量 QODERCN_PERSONAL_ACCESS_TOKEN
//!
//! [server]
//! url = "https://example.com/token-quota"
//! token_file = "~/.config/token-quota/server_token" # 也可用环境变量 TQ_SERVER_TOKEN
//! ```

use crate::snapshot::Platform;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub device: Option<String>,
    #[serde(default)]
    pub accounts: Accounts,
    #[serde(default)]
    pub qoder: QoderConfig,
    #[serde(default)]
    pub server: ServerConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounts {
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub qoder: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QoderConfig {
    pub pat_file: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub url: Option<String>,
    pub token_file: Option<String>,
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = match std::env::var_os("TQ_CONFIG") {
            Some(p) => PathBuf::from(p),
            None => config_dir().join("config.toml"),
        };
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("读取配置失败：{}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("配置格式错误：{}", path.display()))
    }

    pub fn device(&self) -> String {
        match &self.device {
            Some(d) => d.clone(),
            None => gethostname::gethostname().to_string_lossy().into_owned(),
        }
    }

    pub fn account(&self, platform: Platform) -> String {
        let alias = match platform {
            Platform::Claude => &self.accounts.claude,
            Platform::Codex => &self.accounts.codex,
            Platform::Qoder => &self.accounts.qoder,
        };
        alias.clone().unwrap_or_else(|| "default".to_string())
    }

    /// Qoder CN 个人访问令牌：环境变量优先，其次是配置里的文件。
    pub fn qoder_pat(&self) -> Result<String> {
        secret_from(
            "QODERCN_PERSONAL_ACCESS_TOKEN",
            self.qoder.pat_file.as_deref(),
            "未配置 Qoder CN 个人访问令牌：设置 QODERCN_PERSONAL_ACCESS_TOKEN，或在配置 [qoder] pat_file 指定文件",
        )
    }

    pub fn server_url(&self) -> Result<String> {
        match std::env::var("TQ_SERVER_URL").ok().or_else(|| self.server.url.clone()) {
            Some(url) => Ok(url.trim_end_matches('/').to_string()),
            None => bail!("未配置上报服务器：设置 TQ_SERVER_URL，或在配置 [server] url 填写地址"),
        }
    }

    pub fn server_token(&self) -> Result<String> {
        secret_from(
            "TQ_SERVER_TOKEN",
            self.server.token_file.as_deref(),
            "未配置上报服务器令牌：设置 TQ_SERVER_TOKEN，或在配置 [server] token_file 指定文件",
        )
    }
}

fn secret_from(env_name: &str, file: Option<&str>, missing_message: &str) -> Result<String> {
    if let Ok(value) = std::env::var(env_name) {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Ok(value);
        }
    }
    let Some(file) = file else { bail!("{missing_message}") };
    let path = expand_home(file);
    let value = std::fs::read_to_string(&path)
        .with_context(|| format!("读取令牌文件失败：{}", path.display()))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        bail!("令牌文件为空：{}", path.display());
    }
    Ok(value)
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(path),
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home_dir().join(".config")).join("token-quota")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| home_dir().join(".cache")).join("token-quota")
}

/// 先写临时文件再改名，避免读到写了一半的文件。
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("创建目录失败：{}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).with_context(|| format!("写入失败：{}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("写入失败：{}", path.display()))?;
    Ok(())
}
