//! Confirmed webhook notifications with a redacted local audit trail.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::data::atomic_write;
use crate::{BossError, DataPaths};

/// Maximum length of a stable notification event type.
pub const MAX_NOTIFICATION_EVENT_CHARS: usize = 64;
/// Maximum length accepted for the runtime-only webhook URL.
pub const MAX_NOTIFICATION_WEBHOOK_URL_CHARS: usize = 2048;
const MAX_AUDIT_EVENTS: usize = 200;

/// The bounded aggregate counts that may leave the local data root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NotificationSummary {
    /// Cached job count.
    pub cached_jobs: u64,
    /// Local shortlist entry count.
    pub shortlist: u64,
    /// Local preset count.
    pub presets: u64,
    /// Local watch count.
    pub watches: u64,
    /// Local typed resume count.
    pub resumes: u64,
    /// Local campaign plan count.
    pub campaign_plans: u64,
}

impl NotificationSummary {
    /// Builds a bounded summary from the service's local statistics envelope.
    pub fn from_stats(stats: &Value) -> Result<Self, BossError> {
        Ok(Self {
            cached_jobs: stat_number(stats, &["jobs", "total"])?,
            shortlist: stat_number(stats, &["shortlist"])?,
            presets: stat_number(stats, &["presets"])?,
            watches: stat_number(stats, &["watches"])?,
            resumes: stat_number(stats, &["resumes"])?,
            campaign_plans: stat_number(stats, &["campaign", "plans", "total"])?,
        })
    }
}

/// The minimal JSON payload sent only after explicit confirmation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NotificationPayload {
    /// A stable, bounded local event type.
    pub event: String,
    /// Aggregate counts only; it never contains jobs, resumes, or credentials.
    pub summary: NotificationSummary,
}

impl NotificationPayload {
    /// Validates an event and constructs a safe local or remote payload.
    pub fn new(event: &str, summary: NotificationSummary) -> Result<Self, BossError> {
        Ok(Self {
            event: normalize_event(event)?,
            summary,
        })
    }
}

/// Redacted outcome persisted for each attempted confirmed notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationStatus {
    /// The webhook accepted a successful HTTP response.
    Success,
    /// Configuration, transport, or HTTP failure prevented delivery.
    Failure,
}

/// A bounded local audit record with no endpoint, payload, or response data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationAudit {
    /// Stable event type.
    pub event: String,
    /// Delivery outcome only.
    pub status: NotificationStatus,
    /// Seconds since the Unix epoch when the attempt ended.
    pub timestamp: u64,
}

/// Atomic local store for redacted notification audit records.
#[derive(Clone, Debug)]
pub struct NotificationStore {
    path: PathBuf,
}

impl NotificationStore {
    /// Uses the notification audit file under the shared local data root.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.notification_audit(),
        }
    }

    /// Lists validated audit records.
    pub fn list(&self) -> Result<Vec<NotificationAudit>, BossError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(BossError::Notification("notification audit cannot be read")),
        };
        let values: Vec<NotificationAudit> = serde_json::from_slice(&bytes)
            .map_err(|_| BossError::Notification("notification audit is invalid"))?;
        if values.len() > MAX_AUDIT_EVENTS
            || values.iter().any(|value| {
                normalize_event(&value.event)
                    .map(|normalized| normalized != value.event)
                    .unwrap_or(true)
                    || value.timestamp == 0
            })
        {
            return Err(BossError::Notification("notification audit is invalid"));
        }
        Ok(values)
    }

    /// Appends one redacted record and retains only the newest bounded history.
    pub fn record(
        &self,
        event: &str,
        status: NotificationStatus,
    ) -> Result<NotificationAudit, BossError> {
        let audit = NotificationAudit {
            event: normalize_event(event)?,
            status,
            // Keep this immediately beside record construction so every branch in
            // `notification_send` records the time of its own outcome.
            timestamp: audit_timestamp()?,
        };
        let mut values = self.list()?;
        values.push(audit.clone());
        let retained_from = values.len().saturating_sub(MAX_AUDIT_EVENTS);
        if retained_from > 0 {
            values.drain(..retained_from);
        }
        let bytes = serde_json::to_vec(&values)
            .map_err(|_| BossError::Notification("notification audit cannot be encoded"))?;
        atomic_write(&self.path, &bytes, |_| {
            BossError::Notification("notification audit cannot be written")
        })?;
        Ok(audit)
    }
}

fn audit_timestamp() -> Result<u64, BossError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| BossError::Notification("notification timestamp is unavailable"))
}

/// Validates a runtime-only HTTPS webhook URL without exposing it in errors.
pub fn webhook_url_from_environment() -> Result<Url, BossError> {
    let value = std::env::var("BOSS_NOTIFY_WEBHOOK_URL")
        .map_err(|_| BossError::Notification("notification webhook is not configured"))?;
    validate_webhook_url(&value)
}

/// Validates an event type shared by CLI, MCP, and audit persistence.
pub fn normalize_event(event: &str) -> Result<String, BossError> {
    if event.is_empty()
        || event.len() > MAX_NOTIFICATION_EVENT_CHARS
        || !event.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' | b'0'..=b'9' => true,
            b'.' | b'_' | b'-' => index > 0,
            _ => false,
        })
    {
        return Err(BossError::InvalidArgument(
            "notification event must use 1..=64 lowercase letters, digits, '.', '_' or '-'"
                .to_owned(),
        ));
    }
    Ok(event.to_owned())
}

/// Validates an HTTPS URL while deliberately omitting the value from errors.
pub fn validate_webhook_url(value: &str) -> Result<Url, BossError> {
    if value.is_empty()
        || value.len() > MAX_NOTIFICATION_WEBHOOK_URL_CHARS
        || value.trim() != value
        || value.contains('%')
    {
        return Err(BossError::Notification(
            "notification webhook URL is invalid",
        ));
    }
    let url = Url::parse(value)
        .map_err(|_| BossError::Notification("notification webhook URL is invalid"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(BossError::Notification(
            "notification webhook URL is invalid",
        ));
    }
    Ok(url)
}

fn stat_number(stats: &Value, path: &[&str]) -> Result<u64, BossError> {
    let mut value = stats;
    for segment in path {
        value = value.get(*segment).ok_or(BossError::Notification(
            "local notification statistics are unavailable",
        ))?;
    }
    value.as_u64().ok_or(BossError::Notification(
        "local notification statistics are unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn event_and_webhook_validation_are_bounded_and_secret_safe() {
        assert_eq!(
            normalize_event("campaign.ready").expect("valid event"),
            "campaign.ready"
        );
        for event in ["", "Upper", " starts", "a/../b", &"a".repeat(65)] {
            assert!(normalize_event(event).is_err(), "{event:?}");
        }
        assert!(validate_webhook_url("https://notify.example.test/hooks/boss").is_ok());
        for value in [
            "http://notify.example.test/hooks",
            "https://user:pass@notify.example.test/hooks",
            "https://notify.example.test/hooks?secret=value",
            "https://notify.example.test/hooks#fragment",
            "https://notify.example.test/%68ooks",
        ] {
            let error = validate_webhook_url(value).expect_err("unsafe URL");
            assert!(!error.to_string().contains(value));
        }
    }

    #[test]
    fn audit_store_retains_only_redacted_bounded_records() {
        let directory = tempdir().expect("directory");
        let store = NotificationStore::from_paths(&DataPaths::new(directory.path()));
        for _ in 1..=201 {
            store
                .record("campaign.ready", NotificationStatus::Success)
                .expect("record");
        }
        let records = store.list().expect("list");
        assert_eq!(records.len(), 200);
        assert!(records.iter().all(|record| record.timestamp > 0));
        let encoded = std::fs::read_to_string(directory.path().join("notification_audit.json"))
            .expect("audit");
        assert!(encoded.contains("campaign.ready") && encoded.contains("success"));
        for forbidden in ["url", "body", "response", "header", "resume", "job"] {
            assert!(!encoded.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn audit_store_rejects_persisted_sensitive_or_unknown_fields() {
        let directory = tempdir().expect("directory");
        let path = directory.path().join("notification_audit.json");
        for field in ["url", "body", "header", "response"] {
            std::fs::write(
                &path,
                serde_json::json!([{
                    "event":"watch.complete", "status":"failure", "timestamp":1,
                    field:"must not persist"
                }])
                .to_string(),
            )
            .expect("write audit");
            let store = NotificationStore::from_paths(&DataPaths::new(directory.path()));
            assert!(store.list().is_err(), "{field}");
        }
    }

    #[test]
    fn payload_extracts_counts_without_local_content() {
        let payload = NotificationPayload::new(
            "watch.complete",
            NotificationSummary::from_stats(&serde_json::json!({
                "jobs":{"total":3}, "shortlist":2, "presets":1, "watches":4,
                "resumes":5, "campaign":{"plans":{"total":6}}
            }))
            .expect("summary"),
        )
        .expect("payload");
        assert_eq!(payload.summary.campaign_plans, 6);
        assert_eq!(
            serde_json::to_value(payload).expect("json")["event"],
            "watch.complete"
        );
    }
}
