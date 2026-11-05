//! 简单的阻塞 HTTP 封装：非 2xx 不当作异常，由调用方按状态码给出明确提示。

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;
use ureq::Agent;

pub fn agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent("token-quota/0.1")
        .build()
        .into()
}

pub struct Reply {
    pub status: u16,
    pub body: String,
}

impl Reply {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 2xx 时原样返回；否则带上状态码和响应开头，便于定位问题。
    pub fn ensure_success(self, what: &str) -> Result<Reply> {
        if self.is_success() {
            return Ok(self);
        }
        let snippet: String = self.body.chars().take(200).collect();
        bail!("{what} 返回 HTTP {}：{}", self.status, snippet.trim())
    }

    pub fn json<T: DeserializeOwned>(&self, what: &str) -> Result<T> {
        serde_json::from_str(&self.body).with_context(|| format!("{what} 的响应格式无法解析"))
    }
}

pub fn get(agent: &Agent, url: &str, headers: &[(&str, &str)]) -> Result<Reply> {
    let mut request = agent.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.call().with_context(|| format!("请求失败：GET {url}"))?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string()?;
    Ok(Reply { status, body })
}

pub fn post_json<T: Serialize>(
    agent: &Agent,
    url: &str,
    headers: &[(&str, &str)],
    body: &T,
) -> Result<Reply> {
    let mut request = agent.post(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.send_json(body).with_context(|| format!("请求失败：POST {url}"))?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string()?;
    Ok(Reply { status, body })
}

pub fn post_empty(agent: &Agent, url: &str, headers: &[(&str, &str)]) -> Result<Reply> {
    let mut request = agent.post(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.send_empty().with_context(|| format!("请求失败：POST {url}"))?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string()?;
    Ok(Reply { status, body })
}
