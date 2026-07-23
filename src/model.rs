//! Serializable domain models and output envelopes.

use serde::{Deserialize, Serialize};

use crate::BossError;

/// Supported job platform selector.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// BOSS 直聘.
    Zhipin,
    /// 智联招聘.
    Zhilian,
    /// 前程无忧 / 51job.
    Qiancheng,
}

/// Owned platform selector used by saved local search specifications.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformSelector {
    /// Search all registered providers.
    All,
    /// Search BOSS 直聘.
    Zhipin,
    /// Search 智联招聘.
    Zhilian,
    /// Search 前程无忧.
    Qiancheng,
}

impl PlatformSelector {
    /// Converts the owned selector to the service's optional platform.
    #[must_use]
    pub const fn selected(self) -> Option<Platform> {
        match self {
            Self::All => None,
            Self::Zhipin => Some(Platform::Zhipin),
            Self::Zhilian => Some(Platform::Zhilian),
            Self::Qiancheng => Some(Platform::Qiancheng),
        }
    }
}

impl From<Option<Platform>> for PlatformSelector {
    fn from(value: Option<Platform>) -> Self {
        match value {
            None => Self::All,
            Some(Platform::Zhipin) => Self::Zhipin,
            Some(Platform::Zhilian) => Self::Zhilian,
            Some(Platform::Qiancheng) => Self::Qiancheng,
        }
    }
}

impl Platform {
    /// Returns the stable CLI value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zhipin => "zhipin",
            Self::Zhilian => "zhilian",
            Self::Qiancheng => "qiancheng",
        }
    }

    /// Returns the platform display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Zhipin => "BOSS 直聘",
            Self::Zhilian => "智联招聘",
            Self::Qiancheng => "前程无忧 / 51job",
        }
    }
}

/// A normalized cached job.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Job {
    /// Deterministic local identifier.
    pub id: String,
    /// Source platform.
    pub platform: Platform,
    /// Platform-native identifier when available.
    pub remote_id: String,
    /// Job title.
    pub title: String,
    /// Company name.
    pub company: String,
    /// City or location.
    pub city: String,
    /// Human-readable salary.
    pub salary: String,
    /// Public job URL.
    pub url: String,
    /// District or sub-city location.
    #[serde(default)]
    pub district: String,
    /// Required experience.
    #[serde(default)]
    pub experience: String,
    /// Required education.
    #[serde(default)]
    pub education: String,
    /// Employment or job type.
    #[serde(default)]
    pub employment_type: String,
    /// Normalized skill labels.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Welfare and benefit labels.
    #[serde(default)]
    pub welfare: Vec<String>,
    /// Job description text.
    #[serde(default)]
    pub description: String,
    /// Detailed workplace address.
    #[serde(default)]
    pub address: String,
}

impl Job {
    /// Creates a normalized job with optional discovery fields empty.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        platform: Platform,
        remote_id: impl Into<String>,
        title: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            platform,
            remote_id: remote_id.into(),
            title: title.into(),
            company: String::new(),
            city: String::new(),
            salary: String::new(),
            url: url.into(),
            district: String::new(),
            experience: String::new(),
            education: String::new(),
            employment_type: String::new(),
            skills: Vec::new(),
            welfare: Vec::new(),
            description: String::new(),
            address: String::new(),
        }
    }
}

/// Local filters applied to each provider's returned list fields.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchFilters {
    /// Company-name substring.
    pub company: Option<String>,
    /// Salary-text substring.
    pub salary: Option<String>,
    /// Experience-text substring.
    pub experience: Option<String>,
    /// Education-text substring.
    pub education: Option<String>,
    /// Employment-type substring.
    pub employment_type: Option<String>,
    /// Required welfare tokens, matched with AND semantics.
    pub welfare: Vec<String>,
}

/// Complete owned search input used by presets and watches.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSpec {
    /// Non-empty search query.
    pub query: String,
    /// All providers or one selected provider.
    pub platform: PlatformSelector,
    /// Optional shared or provider-native city.
    pub city: Option<String>,
    /// One-based result page.
    pub page: u32,
    /// Positive page size.
    pub limit: u32,
    /// Normalized local result filters.
    pub filters: SearchFilters,
}

/// Explicit overrides applied to a preset or configuration-backed search.
#[derive(Clone, Debug, Default)]
pub struct SearchSpecPatch {
    /// Explicit query.
    pub query: Option<String>,
    /// Explicit platform selector.
    pub platform: Option<PlatformSelector>,
    /// Explicit city.
    pub city: Option<String>,
    /// Explicit page.
    pub page: Option<u32>,
    /// Explicit limit.
    pub limit: Option<u32>,
    /// Explicit company filter.
    pub company: Option<String>,
    /// Explicit salary filter.
    pub salary: Option<String>,
    /// Explicit experience filter.
    pub experience: Option<String>,
    /// Explicit education filter.
    pub education: Option<String>,
    /// Explicit employment-type filter.
    pub employment_type: Option<String>,
    /// Explicit welfare replacement.
    pub welfare: Option<Vec<String>>,
}

impl SearchFilters {
    /// Normalizes, validates, and deduplicates local filter values.
    pub fn new(
        company: Option<String>,
        salary: Option<String>,
        experience: Option<String>,
        education: Option<String>,
        employment_type: Option<String>,
        welfare: Vec<String>,
    ) -> Result<Self, BossError> {
        Ok(Self {
            company: normalize_optional(company, "company")?,
            salary: normalize_optional(salary, "salary")?,
            experience: normalize_optional(experience, "experience")?,
            education: normalize_optional(education, "education")?,
            employment_type: normalize_optional(employment_type, "employment_type")?,
            welfare: normalize_tokens(welfare, "welfare")?,
        })
    }

    /// Returns true when a normalized job satisfies every requested filter.
    #[must_use]
    pub fn matches(&self, job: &Job) -> bool {
        matches_text(self.company.as_deref(), &job.company)
            && matches_text(self.salary.as_deref(), &job.salary)
            && matches_text(self.experience.as_deref(), &job.experience)
            && matches_text(self.education.as_deref(), &job.education)
            && matches_text(self.employment_type.as_deref(), &job.employment_type)
            && {
                let searchable = format!(
                    "{} {} {}",
                    job.welfare.join(" "),
                    job.skills.join(" "),
                    job.description
                );
                self.welfare
                    .iter()
                    .all(|token| contains_normalized(&searchable, token))
            }
    }
}

/// Platform registration and capability information.
#[derive(Debug, Serialize)]
pub struct PlatformInfo {
    /// Stable platform name.
    pub platform: &'static str,
    /// Human-readable name.
    pub display_name: &'static str,
    /// Whether read-only search is implemented.
    pub search: &'static str,
    /// Supported operations.
    pub capabilities: [&'static str; 2],
}

/// Safe provider failure details.
#[derive(Clone, Debug, Serialize)]
pub struct ProviderFailure {
    /// Machine-readable error code.
    pub code: String,
    /// Redacted message.
    pub message: String,
    /// Whether retrying may succeed.
    pub recoverable: bool,
}

/// One provider's search outcome.
#[derive(Debug, Serialize)]
pub struct ProviderResult {
    /// Source platform.
    pub platform: Platform,
    /// Normalized jobs, empty on failure.
    pub jobs: Vec<Job>,
    /// Failure details, absent on success.
    pub error: Option<ProviderFailure>,
}

/// Aggregated search report preserving partial failures.
#[derive(Debug, Serialize)]
pub struct SearchReport {
    /// Original query.
    pub query: String,
    /// Normalized local-only filters.
    pub filters: SearchFilters,
    /// Per-provider outcomes.
    pub providers: Vec<ProviderResult>,
}

fn normalize_optional(value: Option<String>, field: &str) -> Result<Option<String>, BossError> {
    value
        .map(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Err(BossError::InvalidArgument(format!(
                    "{field} must not be empty"
                )))
            } else {
                Ok(trimmed.to_owned())
            }
        })
        .transpose()
}

fn normalize_tokens(tokens: Vec<String>, field: &str) -> Result<Vec<String>, BossError> {
    let mut normalized: Vec<String> = Vec::new();
    for token in tokens {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err(BossError::InvalidArgument(format!(
                "{field} tokens must not be empty"
            )));
        }
        if !normalized
            .iter()
            .any(|existing| contains_equal(existing, trimmed))
        {
            normalized.push(trimmed.to_owned());
        }
    }
    Ok(normalized)
}

fn matches_text(filter: Option<&str>, value: &str) -> bool {
    filter.is_none_or(|filter| contains_normalized(value, filter))
}

fn contains_normalized(value: &str, needle: &str) -> bool {
    value
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn contains_equal(value: &str, other: &str) -> bool {
    value.eq_ignore_ascii_case(other)
}

impl SearchReport {
    /// Returns true when at least one provider succeeded.
    #[must_use]
    pub fn has_success(&self) -> bool {
        self.providers.iter().any(|result| result.error.is_none())
    }
}

/// Stable JSON error representation.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    /// Machine-readable error code.
    pub code: String,
    /// Human-readable safe message.
    pub message: String,
    /// Whether the operation may succeed after retry or adjustment.
    pub recoverable: bool,
}

/// JSON-only CLI output envelope.
#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    /// Whether the operation succeeded.
    pub ok: bool,
    /// Operation data, including partial search data on total failure.
    pub data: Option<T>,
    /// Error details.
    pub error: Option<ErrorBody>,
    /// Safe operator hints.
    pub hints: Vec<String>,
}

impl<T: Serialize> Envelope<T> {
    /// Constructs a success envelope.
    #[must_use]
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            hints: Vec::new(),
        }
    }

    /// Constructs a failure envelope with optional partial data.
    #[must_use]
    pub fn failure(error: &BossError, data: Option<T>, hints: Vec<String>) -> Self {
        Self {
            ok: false,
            data,
            error: Some(ErrorBody {
                code: error.code().to_owned(),
                message: redact_secrets(&error.to_string()),
                recoverable: error.recoverable(),
            }),
            hints,
        }
    }
}

/// Removes configured provider cookies from text before it reaches output.
#[must_use]
pub fn redact_secrets(message: &str) -> String {
    [
        "BOSS_ZHIPIN_COOKIE",
        "BOSS_ZHILIAN_COOKIE",
        "BOSS_QIANCHENG_COOKIE",
    ]
    .iter()
    .filter_map(|name| std::env::var(name).ok())
    .filter(|secret| !secret.is_empty())
    .fold(message.to_owned(), |safe, secret| {
        safe.replace(&secret, "[REDACTED]")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_cookie_is_redacted() {
        // SAFETY: This test is the only writer of this dedicated environment variable.
        unsafe { std::env::set_var("BOSS_ZHIPIN_COOKIE", "secret-cookie-value") };
        let output = redact_secrets("request failed with secret-cookie-value");
        // SAFETY: Restore process state before returning from the test.
        unsafe { std::env::remove_var("BOSS_ZHIPIN_COOKIE") };
        assert_eq!(output, "request failed with [REDACTED]");
    }

    #[test]
    fn filters_normalize_dedupe_and_match_welfare_with_and_semantics() {
        let filters = SearchFilters::new(
            Some(" example ".to_owned()),
            None,
            None,
            None,
            Some("FULL".to_owned()),
            vec![
                " remote ".to_owned(),
                "remote".to_owned(),
                "Rust".to_owned(),
            ],
        )
        .expect("filters");
        let mut job = Job::new("id", Platform::Zhipin, "remote", "Rust", "https://job");
        job.company = "Example Corp".to_owned();
        job.employment_type = "Full-time".to_owned();
        job.skills = vec!["Rust".to_owned()];
        job.description = "Remote friendly".to_owned();
        assert!(filters.matches(&job) && filters.welfare.len() == 2);
    }

    #[test]
    fn filters_reject_empty_tokens() {
        let filters = SearchFilters::new(None, None, None, None, None, vec![" ".to_owned()]);
        assert!(filters.is_err());
    }
}
