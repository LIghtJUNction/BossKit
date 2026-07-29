//! Error types used by the library.

use thiserror::Error;

/// Errors returned by configuration, providers, and the local cache.
#[derive(Debug, Error)]
pub enum BossError {
    /// A request argument is invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// A provider URL failed its outbound request trust policy.
    #[error("unsafe provider URL: {0}")]
    UnsafeProviderUrl(String),
    /// An HTTP request failed.
    #[error("network request failed: {0}")]
    Network(String),
    /// A platform rejected the request.
    #[error("platform returned HTTP {status}: {message}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Safe, redacted explanation.
        message: String,
    },
    /// A provider response could not be normalized.
    #[error("response parse failed: {0}")]
    Parse(String),
    /// Local cache I/O failed.
    #[error("cache I/O failed: {0}")]
    CacheIo(#[from] std::io::Error),
    /// Local cache JSON was invalid.
    #[error("cache JSON failed: {0}")]
    CacheJson(#[from] serde_json::Error),
    /// Configuration I/O failed.
    #[error("config I/O failed: {0}")]
    ConfigIo(String),
    /// Configuration JSON was invalid.
    #[error("config JSON failed: {0}")]
    ConfigJson(String),
    /// Shortlist I/O failed.
    #[error("shortlist I/O failed: {0}")]
    ShortlistIo(String),
    /// Shortlist JSON was invalid.
    #[error("shortlist JSON failed: {0}")]
    ShortlistJson(String),
    /// Data directory probing failed.
    #[error("data directory failed: {0}")]
    DataIo(String),
    /// Search history I/O failed.
    #[error("history I/O failed: {0}")]
    HistoryIo(String),
    /// Search history JSON was invalid.
    #[error("history JSON failed: {0}")]
    HistoryJson(String),
    /// Export file I/O failed.
    #[error("export I/O failed: {0}")]
    ExportIo(String),
    /// Export encoding failed.
    #[error("export encoding failed: {0}")]
    ExportEncoding(String),
    /// Export refused to overwrite an existing path.
    #[error("export target already exists: {0}")]
    ExportExists(String),
    /// Preset persistence or lookup failed.
    #[error("preset failed: {0}")]
    Preset(String),
    /// Watch persistence or lookup failed.
    #[error("watch failed: {0}")]
    Watch(String),
    /// Resume persistence or validation failed.
    #[error("resume failed: {0}")]
    Resume(String),
    /// Local keyword-reply rule persistence or lookup failed.
    #[error("reply rule failed: {0}")]
    Reply(String),
    /// Local campaign policy, template, blacklist, or review-plan operation failed.
    #[error("campaign failed: {0}")]
    Campaign(String),
    /// Local AI profile metadata operation failed without exposing credentials.
    #[error("AI profile failed: {0}")]
    Ai(String),
    /// The runtime-only model API key was not supplied.
    #[error("AI model API key is not configured")]
    AiApiKeyMissing,
    /// The confirmed model request could not reach its HTTPS endpoint.
    #[error("AI model request failed")]
    AiNetwork,
    /// The model endpoint returned a non-success response without exposing its body.
    #[error("AI model returned HTTP {status}")]
    AiHttp { status: u16 },
    /// The model response did not match the bounded expected shape.
    #[error("AI model response failed: {0}")]
    AiResponse(&'static str),
    /// Notification configuration, audit, or confirmed delivery failed without revealing a URL.
    #[error("notification failed: {0}")]
    Notification(&'static str),
    /// Guarded cleanup failed.
    #[error("cleanup failed: {0}")]
    Cleanup(String),
    /// Local authentication setup or private credential storage failed.
    #[error("authentication setup failed: {0}")]
    Authentication(String),
}

impl BossError {
    /// Returns a stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "invalid_argument",
            Self::UnsafeProviderUrl(_) => "unsafe_provider_url",
            Self::Network(_) => "network_error",
            Self::Http {
                status: 401 | 403, ..
            } => "authentication_or_risk_control",
            Self::Http { status: 429, .. } => "rate_limited",
            Self::Http { .. } => "http_error",
            Self::Parse(_) => "parse_error",
            Self::CacheIo(_) => "cache_io_error",
            Self::CacheJson(_) => "cache_json_error",
            Self::ConfigIo(_) => "config_io",
            Self::ConfigJson(_) => "config_json",
            Self::ShortlistIo(_) => "shortlist_io",
            Self::ShortlistJson(_) => "shortlist_json",
            Self::DataIo(_) => "data_io",
            Self::HistoryIo(_) => "history_io",
            Self::HistoryJson(_) => "history_json",
            Self::ExportIo(_) => "export_io",
            Self::ExportEncoding(_) => "export_encoding",
            Self::ExportExists(_) => "export_exists",
            Self::Preset(_) => "preset_error",
            Self::Watch(_) => "watch_error",
            Self::Resume(_) => "resume_error",
            Self::Reply(_) => "reply_error",
            Self::Campaign(_) => "campaign_error",
            Self::Ai(_) => "ai_error",
            Self::AiApiKeyMissing => "ai_api_key_missing",
            Self::AiNetwork => "ai_network_error",
            Self::AiHttp { .. } => "ai_http_error",
            Self::AiResponse(_) => "ai_response_error",
            Self::Notification(_) => "notification_error",
            Self::Cleanup(_) => "cleanup_error",
            Self::Authentication(_) => "authentication_error",
        }
    }

    /// Indicates whether retrying or adjusting authentication may help.
    #[must_use]
    pub const fn recoverable(&self) -> bool {
        !matches!(
            self,
            Self::InvalidArgument(_)
                | Self::UnsafeProviderUrl(_)
                | Self::Preset(_)
                | Self::Watch(_)
                | Self::Resume(_)
                | Self::Reply(_)
                | Self::Campaign(_)
                | Self::Ai(_)
                | Self::AiApiKeyMissing
                | Self::AiResponse(_)
                | Self::Notification(_)
                | Self::Cleanup(_)
        )
    }
}
