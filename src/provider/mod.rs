//! Read-only provider adapters.

mod qiancheng;
mod zhilian;
mod zhipin;

use async_trait::async_trait;
use reqwest::{Client, Url, header::CONTENT_TYPE};
use sha2::{Digest, Sha256};

use crate::{BossError, Job, Platform};

pub use qiancheng::QianchengProvider;
pub use zhilian::ZhilianProvider;
pub use zhipin::ZhipinProvider;

/// Search parameters shared by every provider.
#[derive(Clone, Debug)]
pub struct SearchRequest<'a> {
    /// Search phrase.
    pub query: &'a str,
    /// Provider-native city value.
    pub city: Option<&'a str>,
    /// One-based page.
    pub page: u32,
    /// Maximum result count.
    pub limit: u32,
}

/// Shared interface implemented by every provider.
#[async_trait]
pub trait JobProvider: Send + Sync {
    /// Adapter platform.
    fn platform(&self) -> Platform;
    /// Executes a read-only public search.
    async fn search(&self, request: &SearchRequest<'_>) -> Result<Vec<Job>, BossError>;
    /// Fetches and overlays one read-only job detail.
    async fn detail(&self, _job: &Job) -> Result<Job, BossError> {
        Err(BossError::InvalidArgument(
            "provider detail is not implemented".to_owned(),
        ))
    }
}

/// Builds the restrained HTTP client used by provider adapters.
pub fn http_client(timeout_secs: u64) -> Result<Client, BossError> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 BossKit/0.1")
        .build()
        .map_err(|error| BossError::Network(error.to_string()))
}

pub(crate) fn stable_id(platform: Platform, remote_id: &str, url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(platform.as_str());
    hasher.update([0]);
    hasher.update(remote_id);
    hasher.update([0]);
    hasher.update(url);
    format!("{}-{:x}", platform.as_str(), hasher.finalize())
}

pub(crate) async fn send_json(
    request: reqwest::RequestBuilder,
) -> Result<serde_json::Value, BossError> {
    let response = request
        .send()
        .await
        .map_err(|error| BossError::Network(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(BossError::Http {
            status: status.as_u16(),
            message: status
                .canonical_reason()
                .unwrap_or("request rejected")
                .to_owned(),
        });
    }
    if !json_content_type(response.headers().get(CONTENT_TYPE)) {
        return Err(BossError::Http {
            status: 403,
            message: "provider returned non-JSON challenge content".to_owned(),
        });
    }
    response
        .json()
        .await
        .map_err(|error| BossError::Parse(error.to_string()))
}

fn json_content_type(value: Option<&reqwest::header::HeaderValue>) -> bool {
    value.is_none_or(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                let media_type = media_type.trim();
                media_type.ends_with("/json") || media_type.ends_with("+json")
            })
        })
    })
}

pub(crate) async fn send_text(request: reqwest::RequestBuilder) -> Result<String, BossError> {
    let response = request
        .send()
        .await
        .map_err(|error| BossError::Network(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(BossError::Http {
            status: status.as_u16(),
            message: status
                .canonical_reason()
                .unwrap_or("request rejected")
                .to_owned(),
        });
    }
    response
        .text()
        .await
        .map_err(|error| BossError::Parse(error.to_string()))
}

pub(crate) fn required_url(value: Option<&str>, field: &str) -> Result<String, BossError> {
    value
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BossError::Parse(format!("missing {field}")))
}

pub(crate) fn parse_url(base: &str) -> Result<Url, BossError> {
    Url::parse(base).map_err(|error| BossError::InvalidArgument(error.to_string()))
}

pub(crate) fn first_text(value: &serde_json::Value, paths: &[&str]) -> String {
    paths
        .iter()
        .find_map(|path| match value.pointer(path) {
            Some(serde_json::Value::String(text)) => Some(text.trim().to_owned()),
            Some(serde_json::Value::Array(items)) => {
                let joined = items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                (!joined.is_empty()).then_some(joined)
            }
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) fn text_list(value: &serde_json::Value, paths: &[&str]) -> Vec<String> {
    paths
        .iter()
        .find_map(|path| match value.pointer(path) {
            Some(serde_json::Value::Array(items)) => Some(
                items
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .or_else(|| item.get("name").and_then(serde_json::Value::as_str))
                    })
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            Some(serde_json::Value::String(text)) => Some(
                text.split([',', '，', '、'])
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) fn overlay_text(target: &mut String, value: String) {
    if !value.is_empty() {
        *target = value;
    }
}

pub(crate) fn overlay_list(target: &mut Vec<String>, value: Vec<String>) {
    if !value.is_empty() {
        *target = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_content_type_accepts_json_media_types_and_missing_headers() {
        assert!(json_content_type(None));
        assert!(json_content_type(Some(
            &reqwest::header::HeaderValue::from_static("application/json; charset=utf-8")
        )));
        assert!(json_content_type(Some(
            &reqwest::header::HeaderValue::from_static("application/problem+json")
        )));
    }

    #[test]
    fn json_content_type_rejects_html_challenge_content() {
        assert!(!json_content_type(Some(
            &reqwest::header::HeaderValue::from_static("text/html")
        )));
    }
}
