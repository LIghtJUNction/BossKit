//! Shared services for the BOSS 直聘-focused `boss` CLI and MCP server.

pub mod auth;
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
pub mod preset;
pub mod provider;
pub mod resume;
pub mod schema;
pub mod service;
pub mod shortlist;
pub mod watch;
pub(crate) mod zhipin_browser;
pub(crate) mod zhipin_direct;
pub(crate) mod zhipin_http;

pub use cache::JobCache;
pub use campaign::{
    ApplicationPlan, BlacklistKind, BlacklistRule, CampaignField, CampaignPolicy, CampaignRule,
    CampaignStats, DEFAULT_MINIMUM_RESUME_SCORE, GreetingTemplate, PlanBuildResult,
    PlanGreetingPreview,
};
pub use config::{AppConfig, ConfigEntry, ConfigStore};
pub use data::DataPaths;
pub use error::BossError;
pub use model::{
    Envelope, Job, Platform, SearchFilters, SearchReport, SearchSpec, SearchSpecPatch,
};
pub use service::BossService;
pub use shortlist::{ShortlistEntry, ShortlistStore};
