//! Shared services for the `boss` read-only job search CLI and MCP server.

pub mod ai;
pub mod auth;
pub(crate) mod browser_login;
pub mod cache;
pub mod campaign;
pub mod city;
pub mod config;
pub mod data;
pub mod error;
pub mod export;
pub mod history;
pub mod mcp;
pub mod model;
pub mod notify;
pub mod preset;
pub mod provider;
pub mod reply;
pub mod resume;
pub mod schema;
pub mod service;
pub mod shortlist;
pub mod watch;

pub use ai::{AiProfile, AiProfileStore, AiScore};
pub use cache::JobCache;
pub use campaign::{
    ApplicationPlan, BlacklistKind, BlacklistRule, CampaignField, CampaignPolicy, CampaignRule,
    CampaignStats, GreetingTemplate, PlanBuildResult,
};
pub use config::{AppConfig, ConfigEntry, ConfigStore};
pub use data::DataPaths;
pub use error::BossError;
pub use model::{
    Envelope, Job, Platform, PlatformInfo, PlatformSelector, SearchFilters, SearchReport,
    SearchSpec, SearchSpecPatch,
};
pub use notify::{
    NotificationAudit, NotificationPayload, NotificationStatus, NotificationStore,
    NotificationSummary,
};
pub use reply::{ReplyMatch, ReplyRule, ReplyStore};
pub use service::BossService;
pub use shortlist::{ShortlistEntry, ShortlistStore};
