//! Operations shared by CLI and MCP transports.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::redirect::Policy as RedirectPolicy;
use serde_json::{Value, json};

use crate::ai::{AiProfile, AiProfileStore, AiScore};
use crate::auth::{AuthStore, read_manual_cookie};
use crate::campaign::{
    ApplicationPlan, ApplicationPlanState, BlacklistKind, BlacklistRule, CampaignPolicy,
    CampaignStats, CampaignStore, GreetingTemplate, PlanBuildResult, ScreenPlanOptions,
};
use crate::config::{AppConfig, ConfigChange, ConfigEntry, ConfigStore};
use crate::export::{ExportOptions, ExportResult, ExportSource, structured_jobs, write_export};
use crate::history::{HistoryProviderSummary, SearchHistoryEntry, SearchHistoryStore};
use crate::model::{ProviderFailure, ProviderResult, SearchSpecPatch};
use crate::notify::{
    NotificationPayload, NotificationStatus, NotificationStore, NotificationSummary,
    webhook_url_from_environment,
};
use crate::preset::{Preset, PresetStore};
use crate::provider::{
    JobProvider, QianchengProvider, SearchRequest, ZhilianProvider, ZhipinProvider, http_client,
};
use crate::reply::{ReplyMatch, ReplyRule, ReplyStore};
use crate::resume::{ResumeDiff, ResumeDocument, ResumeStore, export_document};
use crate::schema::{SchemaFormat, render};
use crate::shortlist::{ShortlistComparison, ShortlistEntry, ShortlistStore};
use crate::watch::{Watch, WatchStore};
use crate::{
    BossError, DataPaths, Job, JobCache, Platform, PlatformInfo, PlatformSelector, SearchFilters,
    SearchReport, SearchSpec,
};

const MAX_RESUME_IMPORT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    len: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(not(unix))]
    modified: Option<std::time::Duration>,
}

struct CleanInspection {
    logical: &'static str,
    path: PathBuf,
    identity: Option<FileIdentity>,
}

#[cfg(target_os = "linux")]
struct ArchivedTarget {
    inspection_index: usize,
    name: String,
    recovery_path: PathBuf,
}

#[cfg(target_os = "linux")]
struct ArchiveTransaction {
    path: PathBuf,
    root: OwnedFd,
    archive: OwnedFd,
    directory: OwnedFd,
    transaction_name: String,
}

#[cfg(target_os = "linux")]
struct RescueDirectory {
    path: PathBuf,
    directory: OwnedFd,
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct RecoveryReport {
    verified_paths: Vec<PathBuf>,
    unverified_targets: Vec<String>,
    issues: Vec<String>,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CleanStage {
    AfterArchiveRootChecked,
    AfterTransactionCreated,
    BeforeArchiveMove,
    AfterArchiveMoveVerified,
}

#[cfg(target_os = "linux")]
static CLEAN_NONCE: AtomicU64 = AtomicU64::new(0);

/// High-level application service.
pub struct BossService {
    paths: DataPaths,
    cache: JobCache,
    config: ConfigStore,
    shortlist: ShortlistStore,
    history: SearchHistoryStore,
    presets: PresetStore,
    reply_rules: ReplyStore,
    watches: WatchStore,
    resumes: ResumeStore,
    campaigns: CampaignStore,
    ai_profiles: AiProfileStore,
    auth: AuthStore,
    providers: Vec<Box<dyn JobProvider>>,
}

impl BossService {
    /// Constructs the service with all three real read-only adapters.
    pub fn discover() -> Result<Self, BossError> {
        Self::from_paths(DataPaths::discover())
    }

    pub(crate) fn from_paths(paths: DataPaths) -> Result<Self, BossError> {
        let config = ConfigStore::from_paths(&paths)?;
        let client = http_client(config.effective().request_timeout_secs)?;
        let auth = AuthStore::from_paths(&paths);
        let zhipin_cookie = auth.runtime_cookie(Platform::Zhipin);
        let zhilian_cookie = auth.runtime_cookie(Platform::Zhilian);
        let qiancheng_cookie = auth.runtime_cookie(Platform::Qiancheng);
        Ok(Self {
            cache: JobCache::from_paths(&paths),
            shortlist: ShortlistStore::from_paths(&paths),
            history: SearchHistoryStore::from_paths(&paths),
            presets: PresetStore::from_paths(&paths),
            reply_rules: ReplyStore::from_paths(&paths),
            watches: WatchStore::from_paths(&paths),
            resumes: ResumeStore::from_paths(&paths),
            campaigns: CampaignStore::from_paths(&paths),
            ai_profiles: AiProfileStore::from_paths(&paths),
            paths,
            config,
            auth,
            providers: vec![
                Box::new(ZhipinProvider::new(client.clone(), zhipin_cookie)),
                Box::new(ZhilianProvider::new(client.clone(), zhilian_cookie)),
                Box::new(QianchengProvider::new(client, qiancheng_cookie)),
            ],
        })
    }

    /// Returns the effective typed configuration.
    #[must_use]
    pub fn effective_config(&self) -> AppConfig {
        self.config.effective()
    }

    /// Lists safe configuration values and their source.
    pub fn config_list(&self) -> Result<Vec<ConfigEntry>, BossError> {
        self.config.list()
    }

    /// Reads one safe configuration value.
    pub fn config_get(&self, key: &str) -> Result<ConfigEntry, BossError> {
        self.config.get(key)
    }

    /// Sets one validated user override.
    pub fn config_set(&mut self, key: &str, value: &str) -> Result<ConfigChange, BossError> {
        self.config.set(key, value)
    }

    /// Resets one or all user overrides.
    pub fn config_reset(&mut self, key: Option<&str>) -> Result<ConfigChange, BossError> {
        self.config.reset(key)
    }

    /// Returns all registered platforms.
    #[must_use]
    pub fn platforms(&self) -> Vec<PlatformInfo> {
        [Platform::Zhipin, Platform::Zhilian, Platform::Qiancheng]
            .into_iter()
            .map(|platform| PlatformInfo {
                platform: platform.as_str(),
                display_name: platform.display_name(),
                search: "read_only_registered",
                capabilities: ["search", "detail"],
            })
            .collect()
    }

    /// Lists exact common logical cities.
    #[must_use]
    pub fn cities(&self) -> Value {
        let cities = crate::city::names();
        json!({"count":cities.len(),"cities":cities})
    }

    /// Reports configured authentication state without exposing values or paths.
    #[must_use]
    pub fn status(&self, platform: Option<Platform>) -> Value {
        let providers: Vec<Value> = selected_platforms(platform)
            .into_iter()
            .map(|selected| {
                let variable = AuthStore::cookie_env(selected);
                let env_present = AuthStore::environment_cookie(selected).is_some();
                let stored_session_present = self.auth.has_session(selected);
                json!({
                    "platform":selected,
                    "cookie_env":variable,
                    "present":env_present,
                    "stored_session_present":stored_session_present,
                    "auth_state":if env_present {
                        "env_cookie_present"
                    } else if stored_session_present {
                        "stored_session_present"
                    } else {
                        "missing"
                    }
                })
            })
            .collect();
        json!({
            "network_checked":false,
            "configured_default":self.config.effective().platform,
            "auth_store":self.auth.health().as_str(),
            "providers":providers
        })
    }

    /// Runs strictly local diagnostics and returns errors as structured checks.
    #[must_use]
    pub fn doctor(&self, platform: Option<Platform>) -> Value {
        diagnose_local(
            &self.paths,
            &self.cache,
            &self.shortlist,
            &self.reply_rules,
            &self.auth,
            self.providers.len(),
            platform,
        )
    }

    /// Runs local diagnostics even when the persisted configuration is invalid.
    #[must_use]
    pub fn doctor_local(platform: Option<Platform>) -> Value {
        let paths = DataPaths::discover();
        let cache = JobCache::from_paths(&paths);
        let shortlist = ShortlistStore::from_paths(&paths);
        let reply_rules = ReplyStore::from_paths(&paths);
        let auth = AuthStore::from_paths(&paths);
        diagnose_local(&paths, &cache, &shortlist, &reply_rules, &auth, 3, platform)
    }

    /// Resolves a local Cookie source and verifies Zhipin directly.
    pub async fn login(
        &mut self,
        platform: Option<Platform>,
        manual: bool,
    ) -> Result<Value, BossError> {
        let mut results = Vec::new();
        for selected in selected_platforms(platform) {
            results.push(self.login_platform(selected, manual).await?);
        }
        let direct_verified = results
            .iter()
            .any(|result| result.get("state").and_then(Value::as_str) == Some("direct_verified"));
        Ok(json!({
            "network_checked":direct_verified,
            "verification":if direct_verified {
                "zhipin_authenticated_api_code_0"
            } else {
                "local_unverified"
            },
            "results":results
        }))
    }

    /// Removes local saved sessions.
    pub fn logout(&mut self, platform: Option<Platform>, yes: bool) -> Result<Value, BossError> {
        if !yes {
            return Err(BossError::InvalidArgument(
                "logout requires --yes".to_owned(),
            ));
        }
        let results = selected_platforms(platform)
            .into_iter()
            .map(|selected| {
                Ok(json!({
                    "platform":selected,
                    "revoked":self.auth.revoke(selected)?
                }))
            })
            .collect::<Result<Vec<_>, BossError>>()?;
        Ok(json!({"network_checked":false,"results":results}))
    }

    async fn login_platform(
        &mut self,
        platform: Platform,
        manual: bool,
    ) -> Result<Value, BossError> {
        if manual {
            return self.manual_login_result(platform);
        }
        if let Some(cookie) = AuthStore::environment_cookie(platform) {
            return self.store_login_cookie(platform, cookie, "environment");
        }
        if let Some(cookie) = self.auth.runtime_cookie(platform) {
            return self.store_login_cookie(platform, cookie, "stored_session");
        }
        self.manual_login_result(platform)
    }

    fn manual_login_result(&mut self, platform: Platform) -> Result<Value, BossError> {
        match read_manual_cookie(platform)? {
            Some(cookie) => self.store_login_cookie(platform, cookie, "manual"),
            None => Ok(login_outcome(platform, "manual_login_required", "none")),
        }
    }

    fn store_login_cookie(
        &mut self,
        platform: Platform,
        cookie: String,
        source: &'static str,
    ) -> Result<Value, BossError> {
        if platform == Platform::Zhipin {
            let refreshed = crate::zhipin_direct::refresh_session(&cookie)?;
            self.auth.store_session(platform, refreshed.cookie)?;
            return Ok(json!({
                "platform":platform,
                "state":"direct_verified",
                "source":source,
                "verification":refreshed.verification
            }));
        }
        self.auth.store_session(platform, cookie)?;
        Ok(login_outcome(platform, "stored_unverified", source))
    }

    /// Establishes one explicitly confirmed default Zhipin greeting.
    pub fn chat_greet(&mut self, job_id: &str, yes: bool) -> Result<Value, BossError> {
        if !yes {
            return Err(BossError::InvalidArgument(
                "chat greet requires --yes".to_owned(),
            ));
        }
        let job = self
            .cache
            .show(job_id)?
            .ok_or_else(|| BossError::InvalidArgument(format!("job not found: {job_id}")))?;
        if job.platform != Platform::Zhipin {
            return Err(BossError::InvalidArgument(
                "chat greet supports only cached Zhipin jobs".to_owned(),
            ));
        }
        if job.remote_id.trim().is_empty() || job.title.trim().is_empty() {
            return Err(BossError::InvalidArgument(
                "cached Zhipin job is missing direct greeting metadata".to_owned(),
            ));
        }
        let cookie = self.auth.runtime_cookie(Platform::Zhipin).ok_or_else(|| {
            BossError::Authentication(
                "chat greet requires a saved or environment Zhipin session".to_owned(),
            )
        })?;
        let greeting = crate::zhipin_direct::greet(&cookie, &job.title, &job.remote_id)?;
        self.auth.store_session(Platform::Zhipin, greeting.cookie)?;
        Ok(json!({
            "action":"chat_greet",
            "job_id":job.id,
            "state":greeting.state,
            "verification":greeting.verification,
            "network_checked":true,
            "custom_message_sent":false,
            "resume_submitted":false
        }))
    }

    /// Renders the shared capability registry.
    pub fn schema(&self, format: SchemaFormat) -> Result<Value, BossError> {
        render(format)
    }

    /// Applies explicit fields over a preset or configuration-backed search.
    pub fn resolve_search_spec(
        &self,
        preset: Option<&str>,
        patch: SearchSpecPatch,
    ) -> Result<SearchSpec, BossError> {
        let defaults = self.effective_config();
        let mut spec = match preset {
            Some(name) => self.presets.show(name)?.spec,
            None => SearchSpec {
                query: String::new(),
                platform: PlatformSelector::from(defaults.platform.selected()),
                city: None,
                page: 1,
                limit: u32::try_from(defaults.page_size).unwrap_or(u32::MAX),
                filters: SearchFilters::default(),
            },
        };
        if let Some(value) = patch.query {
            spec.query = value;
        }
        if let Some(value) = patch.platform {
            spec.platform = value;
        }
        if let Some(value) = patch.city {
            spec.city = Some(value);
        }
        if let Some(value) = patch.page {
            spec.page = value;
        }
        if let Some(value) = patch.limit {
            spec.limit = value;
        }
        spec.filters = SearchFilters::new(
            patch.company.or(spec.filters.company),
            patch.salary.or(spec.filters.salary),
            patch.experience.or(spec.filters.experience),
            patch.education.or(spec.filters.education),
            patch.employment_type.or(spec.filters.employment_type),
            patch.welfare.unwrap_or(spec.filters.welfare),
        )?;
        self.validate_search_spec(spec)
    }

    /// Validates and normalizes a complete search specification.
    pub fn validate_search_spec(&self, mut spec: SearchSpec) -> Result<SearchSpec, BossError> {
        spec.query = spec.query.trim().to_owned();
        if spec.query.is_empty() {
            return Err(BossError::InvalidArgument(
                "query or preset is required".to_owned(),
            ));
        }
        if spec.page == 0 || spec.limit == 0 {
            return Err(BossError::InvalidArgument(
                "page and limit must be positive".to_owned(),
            ));
        }
        spec.city = spec
            .city
            .map(|city| city.trim().to_owned())
            .filter(|city| !city.is_empty());
        if let Some(city) = spec.city.as_deref() {
            crate::city::validate_selection(spec.platform.selected(), city)?;
        }
        spec.filters = SearchFilters::new(
            spec.filters.company,
            spec.filters.salary,
            spec.filters.experience,
            spec.filters.education,
            spec.filters.employment_type,
            spec.filters.welfare,
        )?;
        Ok(spec)
    }

    /// Executes one complete validated search specification.
    pub async fn search_spec(&self, spec: SearchSpec) -> Result<SearchReport, BossError> {
        let spec = self.validate_search_spec(spec)?;
        self.search(
            &spec.query,
            spec.platform.selected(),
            spec.city.as_deref(),
            spec.page,
            spec.limit,
            spec.filters,
        )
        .await
    }

    /// Searches selected providers and caches every normalized success.
    pub async fn search(
        &self,
        query: &str,
        platform: Option<Platform>,
        city: Option<&str>,
        page: u32,
        limit: u32,
        filters: SearchFilters,
    ) -> Result<SearchReport, BossError> {
        if let Some(city) = city {
            crate::city::validate_selection(platform, city)?;
        }
        let request = SearchRequest {
            query,
            city,
            page,
            limit,
        };
        let mut results = Vec::new();
        let mut successful_jobs = Vec::new();
        for provider in &self.providers {
            if platform.is_some_and(|selected| selected != provider.platform()) {
                continue;
            }
            match provider.search(&request).await {
                Ok(jobs) => {
                    let jobs: Vec<Job> = jobs
                        .into_iter()
                        .filter(|job| filters.matches(job))
                        .collect();
                    successful_jobs.extend(jobs.iter().cloned());
                    results.push(ProviderResult {
                        platform: provider.platform(),
                        jobs,
                        error: None,
                    });
                }
                Err(error) => results.push(ProviderResult {
                    platform: provider.platform(),
                    jobs: Vec::new(),
                    error: Some(ProviderFailure {
                        code: error.code().to_owned(),
                        message: crate::model::redact_secrets(&error.to_string()),
                        recoverable: error.recoverable(),
                    }),
                }),
            }
        }
        if !successful_jobs.is_empty() {
            self.cache.save(&successful_jobs)?;
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| BossError::HistoryIo(error.to_string()))?
            .as_secs();
        self.history.record(SearchHistoryEntry {
            timestamp,
            query: query.to_owned(),
            platform: platform.map_or_else(|| "all".to_owned(), |value| value.as_str().to_owned()),
            city: city.map(ToOwned::to_owned),
            page,
            limit,
            filters: filters.clone(),
            providers: results
                .iter()
                .map(|result| HistoryProviderSummary {
                    platform: result.platform,
                    count: result.jobs.len(),
                    error_code: result.error.as_ref().map(|error| error.code.clone()),
                })
                .collect(),
        })?;
        Ok(SearchReport {
            query: query.to_owned(),
            filters,
            providers: results,
        })
    }

    /// Lists cached jobs.
    pub fn list(&self, platform: Option<Platform>, limit: usize) -> Result<Vec<Job>, BossError> {
        self.cache.list(platform, limit)
    }

    /// Shows one cached job.
    pub fn show(&self, id: &str) -> Result<Option<Job>, BossError> {
        self.cache.show(id)
    }

    /// Returns cached detail or refreshes it through the matching read-only provider.
    pub async fn detail(&self, id: &str, refresh: bool) -> Result<Job, BossError> {
        let cached = self
            .cache
            .show(id)?
            .ok_or_else(|| BossError::InvalidArgument(format!("job not found: {id}")))?;
        if !refresh && !cached.description.is_empty() {
            return Ok(cached);
        }
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.platform() == cached.platform)
            .ok_or_else(|| {
                BossError::InvalidArgument(format!(
                    "provider not registered: {}",
                    cached.platform.as_str()
                ))
            })?;
        let detailed = provider.detail(&cached).await?;
        self.cache.upsert(detailed.clone())?;
        Ok(detailed)
    }

    /// Lists local BossKit search attempts.
    pub fn history(
        &self,
        platform: Option<Platform>,
        limit: usize,
    ) -> Result<Vec<SearchHistoryEntry>, BossError> {
        self.history.list(platform, limit)
    }

    /// Produces a structured response and optionally writes a local export file.
    pub fn export(&self, options: ExportOptions) -> Result<ExportResult, BossError> {
        let jobs: Vec<Job> = match options.source {
            ExportSource::Jobs => self.cache.all()?,
            ExportSource::Shortlist => self
                .shortlist
                .list(None)?
                .into_iter()
                .map(|entry| entry.job)
                .collect(),
        }
        .into_iter()
        .filter(|job| {
            options
                .platform
                .is_none_or(|selected| selected == job.platform)
        })
        .take(options.limit)
        .collect();
        if let Some(path) = options.output.as_deref() {
            write_export(
                path,
                options.format,
                &jobs,
                options.include_ids,
                options.force,
            )?;
        }
        Ok(ExportResult {
            source: options.source,
            format: options.format,
            count: jobs.len(),
            output: options
                .output
                .as_deref()
                .map(|path| path.display().to_string()),
            jobs: options
                .output
                .is_none()
                .then(|| structured_jobs(&jobs, options.include_ids)),
        })
    }

    /// Adds a cached job to the local shortlist.
    pub fn shortlist_add(
        &self,
        job_id: &str,
        tags: Vec<String>,
        note: Option<String>,
    ) -> Result<ShortlistEntry, BossError> {
        let job = self
            .cache
            .show(job_id)?
            .ok_or_else(|| BossError::InvalidArgument(format!("job not found: {job_id}")))?;
        self.shortlist.add(job, tags, note)
    }

    /// Lists local shortlist entries.
    pub fn shortlist_list(&self, tag: Option<&str>) -> Result<Vec<ShortlistEntry>, BossError> {
        self.shortlist.list(tag)
    }

    /// Annotates a local shortlist entry.
    pub fn shortlist_annotate(
        &self,
        job_id: &str,
        add_tags: Vec<String>,
        remove_tags: Vec<String>,
        note: Option<String>,
    ) -> Result<ShortlistEntry, BossError> {
        self.shortlist.annotate(job_id, add_tags, remove_tags, note)
    }

    /// Removes a local shortlist entry.
    pub fn shortlist_remove(&self, job_id: &str) -> Result<ShortlistEntry, BossError> {
        self.shortlist.remove(job_id)
    }

    /// Compares local shortlist entries.
    pub fn shortlist_compare(&self, tag: Option<&str>) -> Result<ShortlistComparison, BossError> {
        self.shortlist.compare(tag)
    }

    /// Adds or updates one local search preset.
    pub fn preset_add(&self, name: &str, spec: SearchSpec) -> Result<Preset, BossError> {
        let spec = self.validate_search_spec(spec)?;
        self.presets.add(name, spec, now_seconds()?)
    }

    /// Lists local presets.
    pub fn preset_list(&self) -> Result<Vec<Preset>, BossError> {
        self.presets.list()
    }

    /// Shows one local preset.
    pub fn preset_show(&self, name: &str) -> Result<Preset, BossError> {
        self.presets.show(name)
    }

    /// Removes one local preset.
    pub fn preset_remove(&self, name: &str) -> Result<Preset, BossError> {
        self.presets.remove(name)
    }

    /// Adds or updates one strictly local keyword-reply rule.
    pub fn reply_add(&self, keyword: &str, reply: &str) -> Result<ReplyRule, BossError> {
        self.reply_rules.add(keyword, reply, now_seconds()?)
    }

    /// Lists strictly local keyword-reply rules in stored order.
    pub fn reply_list(&self) -> Result<Vec<ReplyRule>, BossError> {
        self.reply_rules.list()
    }

    /// Removes one strictly local keyword-reply rule.
    pub fn reply_remove(&self, keyword: &str) -> Result<ReplyRule, BossError> {
        self.reply_rules.remove(keyword)
    }

    /// Matches local text and returns a suggestion without contacting any platform.
    pub fn reply_match(&self, message: &str) -> Result<ReplyMatch, BossError> {
        self.reply_rules.match_message(message)
    }

    /// Adds or updates a reusable local-only campaign policy.
    pub fn campaign_policy_add(&self, policy: CampaignPolicy) -> Result<CampaignPolicy, BossError> {
        self.campaigns.add_policy(policy)
    }

    /// Lists reusable local-only campaign policies.
    pub fn campaign_policy_list(&self) -> Result<Vec<CampaignPolicy>, BossError> {
        self.campaigns.list_policies()
    }

    /// Shows one local-only campaign policy.
    pub fn campaign_policy_show(&self, name: &str) -> Result<CampaignPolicy, BossError> {
        self.campaigns.show_policy(name)
    }

    /// Removes one local-only campaign policy.
    pub fn campaign_policy_remove(&self, name: &str) -> Result<CampaignPolicy, BossError> {
        self.campaigns.remove_policy(name)
    }

    /// Adds one local campaign blacklist rule.
    pub fn campaign_blacklist_add(
        &self,
        kind: BlacklistKind,
        value: &str,
    ) -> Result<BlacklistRule, BossError> {
        self.campaigns.add_blacklist(kind, value, now_seconds()?)
    }

    /// Lists local campaign blacklist rules.
    pub fn campaign_blacklist_list(&self) -> Result<Vec<BlacklistRule>, BossError> {
        self.campaigns.list_blacklist()
    }

    /// Removes one local campaign blacklist rule.
    pub fn campaign_blacklist_remove(
        &self,
        kind: BlacklistKind,
        value: &str,
    ) -> Result<BlacklistRule, BossError> {
        self.campaigns.remove_blacklist(kind, value)
    }

    /// Adds or updates an allow-listed local greeting template.
    pub fn campaign_template_add(
        &self,
        name: &str,
        body: &str,
    ) -> Result<GreetingTemplate, BossError> {
        self.campaigns.add_template(name, body, now_seconds()?)
    }

    /// Lists local greeting templates.
    pub fn campaign_template_list(&self) -> Result<Vec<GreetingTemplate>, BossError> {
        self.campaigns.list_templates()
    }

    /// Shows one local greeting template.
    pub fn campaign_template_show(&self, name: &str) -> Result<GreetingTemplate, BossError> {
        self.campaigns.show_template(name)
    }

    /// Removes one local greeting template.
    pub fn campaign_template_remove(&self, name: &str) -> Result<GreetingTemplate, BossError> {
        self.campaigns.remove_template(name)
    }

    /// Renders a local template against one cached job without sending it.
    pub fn campaign_template_render(&self, name: &str, job_id: &str) -> Result<String, BossError> {
        let job = self
            .cache
            .show(job_id)?
            .ok_or_else(|| BossError::InvalidArgument(format!("job not found: {job_id}")))?;
        self.campaigns.render_template(name, &job)
    }

    /// Builds local manual-review dry-run plans from cached jobs only.
    pub fn campaign_plan_create(
        &self,
        policy_name: &str,
        template_name: Option<&str>,
        resume_name: Option<&str>,
        limit: usize,
    ) -> Result<PlanBuildResult, BossError> {
        let policy = self.campaigns.show_policy(policy_name)?;
        let template = template_name
            .map(|name| self.campaigns.show_template(name))
            .transpose()?;
        let resume = resume_name
            .map(|name| self.resumes.show(name))
            .transpose()?;
        self.campaigns.build_plans(
            &self.cache.all()?,
            &policy,
            template.as_ref(),
            resume.as_ref(),
            limit,
            now_seconds()?,
        )
    }

    /// Screens cached jobs with explicit resume fields and creates local review plans only.
    pub fn campaign_screen(
        &self,
        resume_name: &str,
        policy_name: &str,
        template_name: Option<&str>,
        limit: usize,
        minimum_resume_score: u8,
    ) -> Result<PlanBuildResult, BossError> {
        let resume = self.resumes.show(resume_name)?;
        let policy = self.campaigns.show_policy(policy_name)?;
        let template = template_name
            .map(|name| self.campaigns.show_template(name))
            .transpose()?;
        self.campaigns.screen_plans(
            &self.cache.all()?,
            &policy,
            &resume,
            ScreenPlanOptions {
                template: template.as_ref(),
                limit,
                minimum_resume_score,
                now: now_seconds()?,
            },
        )
    }

    /// Lists immutable local manual-review dry-run plans.
    pub fn campaign_plan_list(&self) -> Result<Vec<ApplicationPlan>, BossError> {
        self.campaigns.list_plans()
    }

    /// Records a confirmed human plan-state transition without making a remote request.
    pub fn campaign_plan_transition(
        &self,
        job_id: &str,
        state: ApplicationPlanState,
        note: Option<String>,
    ) -> Result<ApplicationPlan, BossError> {
        self.campaigns
            .transition_plan(job_id, state, note, now_seconds()?)
    }

    /// Returns exact counts for local campaign data only.
    pub fn campaign_stats(&self) -> Result<CampaignStats, BossError> {
        self.campaigns.stats()
    }

    /// Adds a foreground watch with a copied search snapshot.
    pub fn watch_add(&self, name: &str, spec: SearchSpec) -> Result<Watch, BossError> {
        let spec = self.validate_search_spec(spec)?;
        self.watches.add(name, spec, now_seconds()?)
    }

    /// Lists foreground watches.
    pub fn watch_list(&self) -> Result<Vec<Watch>, BossError> {
        self.watches.list()
    }

    /// Shows one foreground watch.
    pub fn watch_show(&self, name: &str) -> Result<Watch, BossError> {
        self.watches.show(name)
    }

    /// Removes one foreground watch.
    pub fn watch_remove(&self, name: &str) -> Result<Watch, BossError> {
        self.watches.remove(name)
    }

    /// Runs one watch as an explicit foreground read-only provider search.
    pub async fn watch_run(&self, name: &str) -> Result<Value, BossError> {
        let watch = self.watches.show(name)?;
        let report = self.search_spec(watch.spec).await?;
        if !report.has_success() {
            return Ok(json!({
                "name":name,"ok":false,"new_jobs":[],"new_count":0,
                "report":report,"mutated":false
            }));
        }
        let jobs: Vec<Job> = report
            .providers
            .iter()
            .flat_map(|provider| provider.jobs.iter().cloned())
            .collect();
        let ids: Vec<String> = jobs.iter().map(|job| job.id.clone()).collect();
        let new_ids = self.watches.record_success(name, &ids, now_seconds()?)?;
        let new_jobs: Vec<Job> = jobs
            .into_iter()
            .filter(|job| new_ids.contains(&job.id))
            .collect();
        Ok(json!({
            "name":name,"ok":true,"new_count":new_jobs.len(),
            "new_jobs":new_jobs,"report":report,"mutated":true
        }))
    }

    /// Runs all watches sequentially and preserves every outcome.
    pub async fn watch_run_all(&self) -> Result<Vec<Value>, BossError> {
        let names: Vec<String> = self
            .watches
            .list()?
            .into_iter()
            .map(|watch| watch.name)
            .collect();
        let mut outcomes = Vec::with_capacity(names.len());
        for name in names {
            match self.watch_run(&name).await {
                Ok(value) => outcomes.push(value),
                Err(error) => outcomes.push(json!({
                    "name":name,"ok":false,"mutated":false,
                    "error":{"code":error.code(),"message":error.to_string()}
                })),
            }
        }
        Ok(outcomes)
    }

    /// Initializes one local typed resume.
    pub fn resume_init(
        &self,
        name: &str,
        title: Option<String>,
    ) -> Result<ResumeDocument, BossError> {
        self.resumes.init(name, title, now_seconds()?)
    }

    /// Lists local typed resumes.
    pub fn resume_list(&self) -> Result<Vec<ResumeDocument>, BossError> {
        self.resumes.list()
    }

    /// Shows one local typed resume.
    pub fn resume_show(&self, name: &str) -> Result<ResumeDocument, BossError> {
        self.resumes.show(name)
    }

    /// Sets one allow-listed resume field.
    pub fn resume_set(
        &self,
        name: &str,
        field: &str,
        value: String,
    ) -> Result<ResumeDocument, BossError> {
        self.resumes.set(name, field, value, now_seconds()?)
    }

    /// Mutates normalized local resume skills.
    pub fn resume_skills(
        &self,
        name: &str,
        add: Vec<String>,
        remove: Vec<String>,
    ) -> Result<ResumeDocument, BossError> {
        self.resumes.skills(name, add, remove, now_seconds()?)
    }

    /// Clones one local resume.
    pub fn resume_clone(&self, name: &str, new_name: &str) -> Result<ResumeDocument, BossError> {
        self.resumes.clone_document(name, new_name, now_seconds()?)
    }

    /// Diffs two local resumes.
    pub fn resume_diff(&self, left: &str, right: &str) -> Result<ResumeDiff, BossError> {
        self.resumes.diff(left, right)
    }

    /// Imports one strict JSON resume, capped at 2 MiB.
    pub fn resume_import(&self, path: &Path, force: bool) -> Result<ResumeDocument, BossError> {
        self.resume_import_with_hook(path, force, || {})
    }

    fn resume_import_with_hook(
        &self,
        path: &Path,
        force: bool,
        after_open: impl FnOnce(),
    ) -> Result<ResumeDocument, BossError> {
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            return Err(BossError::InvalidArgument(
                "resume import accepts JSON files only".to_owned(),
            ));
        }
        let path_metadata =
            fs::symlink_metadata(path).map_err(|error| BossError::Resume(error.to_string()))?;
        if path_metadata.file_type().is_symlink() {
            return Err(BossError::Resume(
                "resume import refuses symlinks".to_owned(),
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options
            .open(path)
            .map_err(|error| BossError::Resume(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| BossError::Resume(error.to_string()))?;
        if !metadata.is_file() {
            return Err(BossError::Resume(
                "resume import requires a regular file".to_owned(),
            ));
        }
        after_open();
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len())
                .unwrap_or(MAX_RESUME_IMPORT_BYTES)
                .min(MAX_RESUME_IMPORT_BYTES + 1),
        );
        file.take((MAX_RESUME_IMPORT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| BossError::Resume(error.to_string()))?;
        if bytes.len() > MAX_RESUME_IMPORT_BYTES {
            return Err(BossError::Resume("import exceeds 2 MiB".to_owned()));
        }
        let document: ResumeDocument =
            serde_json::from_slice(&bytes).map_err(|error| BossError::Resume(error.to_string()))?;
        self.resumes.import(document, force)
    }

    /// Returns or writes one strict local resume.
    pub fn resume_export(
        &self,
        name: &str,
        output: Option<&Path>,
        force: bool,
    ) -> Result<Value, BossError> {
        let document = self.resumes.show(name)?;
        if let Some(path) = output {
            export_document(path, &document, force)?;
            Ok(json!({"name":name,"output":path.display().to_string()}))
        } else {
            serde_json::to_value(document).map_err(|error| BossError::Resume(error.to_string()))
        }
    }

    /// Removes one confirmed local resume.
    pub fn resume_remove(&self, name: &str, confirm: bool) -> Result<ResumeDocument, BossError> {
        if !confirm {
            return Err(BossError::InvalidArgument(
                "resume removal requires confirmation".to_owned(),
            ));
        }
        self.resumes.remove(name)
    }

    /// Adds or updates a credential-free local AI model profile.
    pub fn ai_profile_add(
        &self,
        name: &str,
        base_url: &str,
        model: &str,
    ) -> Result<AiProfile, BossError> {
        self.ai_profiles.add(name, base_url, model, now_seconds()?)
    }

    /// Lists credential-free local AI model profiles.
    pub fn ai_profile_list(&self) -> Result<Vec<AiProfile>, BossError> {
        self.ai_profiles.list()
    }

    /// Shows one credential-free local AI model profile.
    pub fn ai_profile_show(&self, name: &str) -> Result<AiProfile, BossError> {
        self.ai_profiles.show(name)
    }

    /// Removes one credential-free local AI model profile.
    pub fn ai_profile_remove(&self, name: &str) -> Result<AiProfile, BossError> {
        self.ai_profiles.remove(name)
    }

    /// Generates one AI draft only after explicit confirmation.
    pub async fn ai_draft(
        &self,
        profile_name: &str,
        job_id: &str,
        resume_name: &str,
        confirm: bool,
    ) -> Result<String, BossError> {
        if !confirm {
            return Err(BossError::InvalidArgument(
                "AI draft requires explicit confirmation".to_owned(),
            ));
        }
        let profile = self.ai_profiles.show(profile_name)?;
        let job = self
            .cache
            .show(job_id)?
            .ok_or_else(|| BossError::InvalidArgument("cached job not found".to_owned()))?;
        let resume = self.resumes.show(resume_name)?;
        crate::ai::draft(&profile, &job, &resume).await
    }

    /// Scores one cached job against one local resume only after confirmation.
    pub async fn ai_score(
        &self,
        profile_name: &str,
        job_id: &str,
        resume_name: &str,
        confirm: bool,
    ) -> Result<AiScore, BossError> {
        if !confirm {
            return Err(BossError::InvalidArgument(
                "AI score requires explicit confirmation".to_owned(),
            ));
        }
        let profile = self.ai_profiles.show(profile_name)?;
        let job = self
            .cache
            .show(job_id)?
            .ok_or_else(|| BossError::InvalidArgument("cached job not found".to_owned()))?;
        let resume = self.resumes.show(resume_name)?;
        crate::ai::score(&profile, &job, &resume).await
    }

    /// Renders a bounded local notification payload without reading environment or using network.
    pub fn notification_preview(&self, event: &str) -> Result<NotificationPayload, BossError> {
        NotificationPayload::new(event, NotificationSummary::from_stats(&self.stats(30)?)?)
    }

    /// Sends one minimal webhook payload only after explicit confirmation.
    ///
    /// The webhook endpoint is runtime-only. Every attempted confirmed delivery records only
    /// event, success/failure status, and timestamp locally; response bodies are never read.
    pub async fn notification_send(
        &self,
        event: &str,
        confirmed: bool,
    ) -> Result<Value, BossError> {
        if !confirmed {
            return Err(BossError::InvalidArgument(
                "notification send requires explicit confirmation".to_owned(),
            ));
        }
        let payload = self.notification_preview(event)?;
        let audit_store = NotificationStore::from_paths(&self.paths);
        let endpoint = match webhook_url_from_environment() {
            Ok(endpoint) => endpoint,
            Err(error) => {
                audit_store.record(&payload.event, NotificationStatus::Failure)?;
                return Err(error);
            }
        };
        let client = match reqwest::Client::builder()
            .https_only(true)
            .redirect(RedirectPolicy::none())
            .timeout(Duration::from_secs(
                self.effective_config().request_timeout_secs,
            ))
            .build()
        {
            Ok(client) => client,
            Err(_) => {
                audit_store.record(&payload.event, NotificationStatus::Failure)?;
                return Err(BossError::Notification(
                    "notification transport is unavailable",
                ));
            }
        };
        let response = client.post(endpoint).json(&payload).send().await;
        let success = response
            .as_ref()
            .is_ok_and(|response| response.status().is_success());
        let status = if success {
            NotificationStatus::Success
        } else {
            NotificationStatus::Failure
        };
        let audit = audit_store.record(&payload.event, status)?;
        if !success {
            return Err(BossError::Notification(
                "notification delivery was not accepted",
            ));
        }
        Ok(json!({
            "mode":"confirmed_remote_notification",
            "sent":true,
            "payload":payload,
            "audit":audit
        }))
    }

    /// Returns exact local workflow statistics.
    pub fn stats(&self, days: u64) -> Result<Value, BossError> {
        if days == 0 {
            return Err(BossError::InvalidArgument(
                "days must be positive".to_owned(),
            ));
        }
        let jobs = self.cache.all()?;
        let mut by_platform = serde_json::Map::new();
        for platform in [Platform::Zhipin, Platform::Zhilian, Platform::Qiancheng] {
            by_platform.insert(
                platform.as_str().to_owned(),
                json!(jobs.iter().filter(|job| job.platform == platform).count()),
            );
        }
        let cutoff = now_seconds()?.saturating_sub(days.saturating_mul(86_400));
        let history: Vec<SearchHistoryEntry> = self
            .history
            .read_all()?
            .into_iter()
            .filter(|entry| entry.timestamp >= cutoff)
            .collect();
        let successful = history
            .iter()
            .filter(|entry| {
                entry
                    .providers
                    .iter()
                    .any(|provider| provider.error_code.is_none())
            })
            .count();
        let sizes = known_file_sizes(&self.paths)?;
        let campaign = self.campaign_stats()?;
        let notification_audit = NotificationStore::from_paths(&self.paths).list()?.len();
        Ok(json!({
            "days":days,
            "jobs":{"total":jobs.len(),"by_platform":by_platform,
                "enriched":jobs.iter().filter(|job| !job.description.is_empty()).count()},
            "history":{"attempts":history.len(),"success":successful,
                "total_failure":history.len().saturating_sub(successful)},
            "shortlist":self.shortlist.list(None)?.len(),
            "presets":self.presets.list()?.len(),
            "reply_rules":self.reply_rules.list()?.len(),
            "watches":self.watches.list()?.len(),
            "resumes":self.resumes.list()?.len(),
            "ai_profiles":self.ai_profiles.list()?.len(),
            "campaign":campaign,
            "notification_audit":notification_audit,
            "file_bytes":sizes
        }))
    }

    /// Previews or recoverably archives exact known mutable JSON files.
    pub fn clean(&self, target: &str, confirmed: bool) -> Result<Value, BossError> {
        #[cfg(target_os = "linux")]
        {
            self.clean_with_hook(target, confirmed, |_, _, _, _| {})
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.clean_portable_preview(target, confirmed)
        }
    }

    #[cfg(target_os = "linux")]
    fn clean_with_hook(
        &self,
        target: &str,
        confirmed: bool,
        mut hook: impl FnMut(CleanStage, usize, &Path, &Path),
    ) -> Result<Value, BossError> {
        let inspected = inspect_clean_targets(&self.paths, target)?;
        if !confirmed {
            return Ok(clean_result(&inspected, false, None, &[]));
        }
        if inspected
            .iter()
            .all(|inspection| inspection.identity.is_none())
        {
            return Ok(clean_result(&inspected, true, None, &[]));
        }
        let transaction = create_archive_transaction(&self.paths, &mut hook)?;
        let archived = archive_clean_targets(&inspected, &transaction, &self.paths, &mut hook)?;
        for target in &archived {
            validate_archived_identity(target, &transaction, &inspected).map_err(|cause| {
                cleanup_rollback_error(
                    cause,
                    restore_archived_files(&archived, &transaction, &inspected, &self.paths),
                )
            })?;
        }
        validate_archive_namespace(&transaction, &self.paths).map_err(|cause| {
            cleanup_rollback_error(
                cause,
                restore_archived_files(&archived, &transaction, &inspected, &self.paths),
            )
        })?;
        let recovery_paths: Vec<(usize, PathBuf)> = archived
            .iter()
            .map(|target| (target.inspection_index, target.recovery_path.clone()))
            .collect();
        Ok(clean_result(
            &inspected,
            true,
            Some(&transaction.path),
            &recovery_paths,
        ))
    }

    #[cfg(not(target_os = "linux"))]
    fn clean_portable_preview(&self, target: &str, confirmed: bool) -> Result<Value, BossError> {
        let inspected = inspect_clean_targets(&self.paths, target)?;
        if confirmed {
            return Err(BossError::Cleanup(
                "confirmed cleanup is supported only on Linux".to_owned(),
            ));
        }
        Ok(clean_result(&inspected, false, None, &[]))
    }
}

fn inspect_clean_targets(
    paths: &DataPaths,
    target: &str,
) -> Result<Vec<CleanInspection>, BossError> {
    clean_targets(paths, target)?
        .into_iter()
        .map(|(logical, path)| {
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => Some(metadata),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(BossError::Cleanup(error.to_string())),
            };
            if let Some(metadata) = metadata.as_ref() {
                if metadata.file_type().is_symlink() {
                    return Err(BossError::Cleanup(format!("refusing symlink: {logical}")));
                }
                if !metadata.is_file() {
                    return Err(BossError::Cleanup(format!("refusing non-file: {logical}")));
                }
            }
            Ok(CleanInspection {
                logical,
                path,
                identity: metadata.as_ref().map(file_identity),
            })
        })
        .collect()
}

fn clean_result(
    inspected: &[CleanInspection],
    confirmed: bool,
    archive_transaction: Option<&Path>,
    archived: &[(usize, PathBuf)],
) -> Value {
    let files: Vec<Value> = inspected
        .iter()
        .enumerate()
        .map(|(index, inspection)| {
            let existed = inspection.identity.is_some();
            let recovery_path = archived
                .iter()
                .find(|(inspection_index, _)| *inspection_index == index)
                .map(|(_, path)| path.display().to_string());
            json!({
                "target":inspection.logical,
                "path":inspection.path.file_name().and_then(|name| name.to_str()),
                "existed":existed,
                "bytes":inspection.identity.as_ref().map_or(0, |identity| identity.len),
                "archived":recovery_path.is_some(),
                "recovery_path":recovery_path
            })
        })
        .collect();
    json!({
        "preview":!confirmed,
        "confirmed":confirmed,
        "action":if confirmed {"archive"} else {"preview"},
        "recoverable":confirmed,
        "archive_transaction":archive_transaction.map(|path| path.display().to_string()),
        "files":files
    })
}

#[cfg(target_os = "linux")]
fn create_archive_transaction(
    paths: &DataPaths,
    hook: &mut impl FnMut(CleanStage, usize, &Path, &Path),
) -> Result<ArchiveTransaction, BossError> {
    create_archive_transaction_with_name(paths, archive_transaction_name()?, hook)
}

#[cfg(target_os = "linux")]
fn create_archive_transaction_with_name(
    paths: &DataPaths,
    transaction_name: String,
    hook: &mut impl FnMut(CleanStage, usize, &Path, &Path),
) -> Result<ArchiveTransaction, BossError> {
    use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, fstat, mkdirat, openat, statat};
    use rustix::io::Errno;

    const ARCHIVE_DIRECTORY: &str = ".bosskit-clean-archive";
    let root_metadata = fs::symlink_metadata(paths.root())
        .map_err(|error| BossError::Cleanup(error.to_string()))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(BossError::Cleanup(
            "data root must be a real directory for confirmed archive".to_owned(),
        ));
    }
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root = openat(CWD, paths.root(), directory_flags, Mode::empty())
        .map_err(|error| BossError::Cleanup(format!("cannot open data root safely: {error}")))?;
    let opened_root = fstat(&root)
        .map_err(|error| BossError::Cleanup(format!("cannot inspect opened data root: {error}")))?;
    if !FileType::from_raw_mode(opened_root.st_mode).is_dir()
        || (opened_root.st_dev, opened_root.st_ino) != (root_metadata.dev(), root_metadata.ino())
    {
        return Err(BossError::Cleanup(
            "opened data root does not match the checked directory".to_owned(),
        ));
    }
    let archive_stat = match statat(&root, ARCHIVE_DIRECTORY, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => {
            mkdirat(&root, ARCHIVE_DIRECTORY, Mode::from_bits_truncate(0o700)).map_err(
                |error| {
                    BossError::Cleanup(format!(
                        "cannot create archive directory without clobbering: {error}"
                    ))
                },
            )?;
            statat(&root, ARCHIVE_DIRECTORY, AtFlags::SYMLINK_NOFOLLOW).map_err(|error| {
                BossError::Cleanup(format!(
                    "cannot verify newly created archive directory: {error}"
                ))
            })?
        }
        Err(error) => {
            return Err(BossError::Cleanup(format!(
                "cannot inspect archive directory safely: {error}"
            )));
        }
    };
    validate_private_directory(
        &archive_stat,
        &paths.root().join(ARCHIVE_DIRECTORY),
        "archive root",
    )?;
    let archive_path = paths.root().join(ARCHIVE_DIRECTORY);
    hook(
        CleanStage::AfterArchiveRootChecked,
        usize::MAX,
        &archive_path,
        &archive_path,
    );
    let archive =
        openat(&root, ARCHIVE_DIRECTORY, directory_flags, Mode::empty()).map_err(|error| {
            BossError::Cleanup(format!("cannot open archive directory safely: {error}"))
        })?;
    let opened_archive = fstat(&archive).map_err(|error| {
        BossError::Cleanup(format!("cannot inspect opened archive directory: {error}"))
    })?;
    validate_private_directory(&opened_archive, &archive_path, "opened archive root")?;
    if directory_identity(&opened_archive) != directory_identity(&archive_stat) {
        return Err(BossError::Cleanup(format!(
            "opened archive root does not match checked inode: {}",
            archive_path.display()
        )));
    }
    mkdirat(
        &archive,
        transaction_name.as_str(),
        Mode::from_bits_truncate(0o700),
    )
    .map_err(|error| {
        BossError::Cleanup(format!(
            "cannot create unique archive transaction without clobbering: {error}"
        ))
    })?;
    let transaction_stat = statat(
        &archive,
        transaction_name.as_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| BossError::Cleanup(format!("cannot inspect archive transaction: {error}")))?;
    let transaction_path = archive_path.join(&transaction_name);
    validate_private_directory(&transaction_stat, &transaction_path, "archive transaction")?;
    hook(
        CleanStage::AfterTransactionCreated,
        usize::MAX,
        &transaction_path,
        &transaction_path,
    );
    let directory = openat(
        &archive,
        transaction_name.as_str(),
        directory_flags,
        Mode::empty(),
    )
    .map_err(|error| BossError::Cleanup(format!("cannot open archive transaction: {error}")))?;
    let opened_transaction = fstat(&directory).map_err(|error| {
        BossError::Cleanup(format!(
            "cannot inspect opened archive transaction: {error}"
        ))
    })?;
    validate_private_directory(
        &opened_transaction,
        &transaction_path,
        "opened archive transaction",
    )?;
    if directory_identity(&opened_transaction) != directory_identity(&transaction_stat) {
        return Err(BossError::Cleanup(format!(
            "opened archive transaction does not match created inode: {}",
            transaction_path.display()
        )));
    }
    Ok(ArchiveTransaction {
        path: transaction_path,
        root,
        archive,
        directory,
        transaction_name,
    })
}

#[cfg(target_os = "linux")]
fn archive_transaction_name() -> Result<String, BossError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| BossError::Cleanup(error.to_string()))?
        .as_nanos();
    let nonce = CLEAN_NONCE.fetch_add(1, Ordering::Relaxed);
    Ok(format!("{}-{timestamp}-{nonce}", std::process::id()))
}

#[cfg(target_os = "linux")]
fn directory_identity(stat: &rustix::fs::Stat) -> (u64, u64) {
    (stat.st_dev, stat.st_ino)
}

#[cfg(target_os = "linux")]
fn validate_private_directory(
    stat: &rustix::fs::Stat,
    path: &Path,
    label: &str,
) -> Result<(), BossError> {
    use rustix::fs::FileType;

    if !FileType::from_raw_mode(stat.st_mode).is_dir()
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o777 != 0o700
    {
        return Err(BossError::Cleanup(format!(
            "refusing unsafe {label}: {}; expected owner-only mode 0700",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn visible_root_matches(transaction: &ArchiveTransaction, paths: &DataPaths) -> bool {
    use rustix::fs::{AtFlags, CWD, fstat, statat};

    statat(CWD, paths.root(), AtFlags::SYMLINK_NOFOLLOW)
        .ok()
        .zip(fstat(&transaction.root).ok())
        .is_some_and(|(visible, opened)| {
            directory_identity(&visible) == directory_identity(&opened)
        })
}

#[cfg(target_os = "linux")]
fn validate_archive_namespace(
    transaction: &ArchiveTransaction,
    paths: &DataPaths,
) -> Result<(), String> {
    use rustix::fs::{AtFlags, CWD, fstat, statat};

    let visible_root = statat(CWD, paths.root(), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| format!("visible data root is unavailable: {error}"))?;
    let opened_root =
        fstat(&transaction.root).map_err(|error| format!("opened data root failed: {error}"))?;
    if directory_identity(&visible_root) != directory_identity(&opened_root) {
        return Err("visible data root no longer identifies opened directory".to_owned());
    }
    let visible_archive = statat(
        &transaction.root,
        ".bosskit-clean-archive",
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| format!("visible archive root is unavailable: {error}"))?;
    let opened_archive = fstat(&transaction.archive)
        .map_err(|error| format!("opened archive root failed: {error}"))?;
    validate_private_directory(
        &opened_archive,
        &paths.root().join(".bosskit-clean-archive"),
        "opened archive root",
    )
    .map_err(|error| error.to_string())?;
    if directory_identity(&visible_archive) != directory_identity(&opened_archive) {
        return Err("visible archive root no longer identifies opened directory".to_owned());
    }
    let visible_transaction = statat(
        &transaction.archive,
        transaction.transaction_name.as_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| format!("visible archive transaction is unavailable: {error}"))?;
    let opened_transaction = fstat(&transaction.directory)
        .map_err(|error| format!("opened archive transaction failed: {error}"))?;
    validate_private_directory(
        &opened_transaction,
        &transaction.path,
        "opened archive transaction",
    )
    .map_err(|error| error.to_string())?;
    if directory_identity(&visible_transaction) != directory_identity(&opened_transaction) {
        return Err("visible archive transaction no longer identifies opened directory".to_owned());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn archive_clean_targets(
    inspected: &[CleanInspection],
    transaction: &ArchiveTransaction,
    paths: &DataPaths,
    hook: &mut impl FnMut(CleanStage, usize, &Path, &Path),
) -> Result<Vec<ArchivedTarget>, BossError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    let mut archived = Vec::new();
    for (index, inspection) in inspected.iter().enumerate() {
        if inspection.identity.is_none() {
            continue;
        }
        let name = inspection
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| BossError::Cleanup("cleanup target has no UTF-8 file name".to_owned()))?
            .to_owned();
        let recovery_path = transaction.path.join(&name);
        hook(
            CleanStage::BeforeArchiveMove,
            index,
            &inspection.path,
            &recovery_path,
        );
        if let Err(error) = renameat_with(
            CWD,
            &inspection.path,
            &transaction.directory,
            name.as_str(),
            RenameFlags::NOREPLACE,
        ) {
            return Err(cleanup_rollback_error(
                format!(
                    "failed to archive {} into the trusted transaction without clobbering: {error}",
                    inspection.logical
                ),
                restore_archived_files(&archived, transaction, inspected, paths),
            ));
        }
        archived.push(ArchivedTarget {
            inspection_index: index,
            name,
            recovery_path,
        });
        let target = archived
            .last()
            .ok_or_else(|| BossError::Cleanup("missing archive state".to_owned()))?;
        if let Err(cause) = validate_archived_identity(target, transaction, inspected) {
            return Err(cleanup_rollback_error(
                cause,
                restore_archived_files(&archived, transaction, inspected, paths),
            ));
        }
        hook(
            CleanStage::AfterArchiveMoveVerified,
            index,
            &inspection.path,
            &target.recovery_path,
        );
    }
    Ok(archived)
}

#[cfg(target_os = "linux")]
fn validate_archived_identity(
    target: &ArchivedTarget,
    transaction: &ArchiveTransaction,
    inspected: &[CleanInspection],
) -> Result<(), String> {
    use rustix::fs::{AtFlags, FileType, statat};

    let inspection = &inspected[target.inspection_index];
    let stat = statat(
        &transaction.directory,
        target.name.as_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| format!("cannot inspect archived file {}: {error}", target.name))?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(format!(
            "archived file type mismatch for {}",
            inspection.logical
        ));
    }
    let identity = file_identity_from_stat(&stat)?;
    if inspection.identity.as_ref() != Some(&identity) {
        return Err(format!(
            "archived file identity mismatch for {}",
            inspection.logical
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn restore_archived_files(
    archived: &[ArchivedTarget],
    transaction: &ArchiveTransaction,
    inspected: &[CleanInspection],
    paths: &DataPaths,
) -> RecoveryReport {
    use rustix::fs::{RenameFlags, renameat_with};

    let mut report = RecoveryReport::default();
    let mut recovered = Vec::new();
    let mut stranded = Vec::new();
    for target in archived.iter().rev() {
        let original = &inspected[target.inspection_index].path;
        match renameat_with(
            &transaction.directory,
            target.name.as_str(),
            &transaction.root,
            target.name.as_str(),
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => recovered.push((target, original.clone(), false)),
            Err(_) => stranded.push(target),
        }
    }
    let rescue = if stranded.is_empty() {
        None
    } else {
        match create_rescue_directory(transaction, paths) {
            Ok(rescue) => {
                for target in stranded {
                    let rescue_path = rescue.path.join(&target.name);
                    match renameat_with(
                        &transaction.directory,
                        target.name.as_str(),
                        &rescue.directory,
                        target.name.as_str(),
                        RenameFlags::NOREPLACE,
                    ) {
                        Ok(()) => recovered.push((target, rescue_path, true)),
                        Err(error) => {
                            report
                                .unverified_targets
                                .push(inspected[target.inspection_index].logical.to_owned());
                            report.issues.push(format!(
                                "could not move {} into trusted rescue directory: {error}",
                                inspected[target.inspection_index].logical
                            ));
                        }
                    }
                }
                Some(rescue)
            }
            Err(error) => {
                for target in stranded {
                    report
                        .unverified_targets
                        .push(inspected[target.inspection_index].logical.to_owned());
                }
                report.issues.push(error);
                None
            }
        }
    };

    if !visible_root_matches(transaction, paths) {
        report.unverified_targets.extend(
            recovered
                .iter()
                .map(|(target, _, _)| inspected[target.inspection_index].logical.to_owned()),
        );
        report
            .issues
            .push("visible data root no longer identifies trusted recovery root".to_owned());
        return report;
    }
    if let Some(rescue) = rescue.as_ref()
        && let Err(error) = validate_rescue_namespace(rescue)
    {
        report.unverified_targets.extend(
            recovered
                .iter()
                .filter(|(_, _, rescued)| *rescued)
                .map(|(target, _, _)| inspected[target.inspection_index].logical.to_owned()),
        );
        recovered.retain(|(_, _, rescued)| !*rescued);
        report.issues.push(error);
    }
    for (target, path, rescued) in recovered {
        let directory = match (rescued, rescue.as_ref()) {
            (true, Some(rescue)) => &rescue.directory,
            (false, _) => &transaction.root,
            (true, None) => {
                report
                    .unverified_targets
                    .push(inspected[target.inspection_index].logical.to_owned());
                report
                    .issues
                    .push("trusted rescue directory closed before validation".to_owned());
                continue;
            }
        };
        match validate_recovered_identity(directory, target, inspected) {
            Ok(()) => report.verified_paths.push(path),
            Err(error) => {
                report
                    .unverified_targets
                    .push(inspected[target.inspection_index].logical.to_owned());
                report.issues.push(error);
            }
        }
    }
    if !visible_root_matches(transaction, paths) {
        report.verified_paths.clear();
        report.unverified_targets.extend(
            archived
                .iter()
                .map(|target| inspected[target.inspection_index].logical.to_owned()),
        );
        report
            .issues
            .push("visible data root changed during recovery validation".to_owned());
    }
    report.unverified_targets.sort();
    report.unverified_targets.dedup();
    report
}

#[cfg(target_os = "linux")]
fn create_rescue_directory(
    transaction: &ArchiveTransaction,
    paths: &DataPaths,
) -> Result<RescueDirectory, String> {
    use rustix::fs::{AtFlags, Mode, OFlags, fstat, mkdirat, openat, statat};

    let name = format!(
        ".bosskit-clean-recovery-{}",
        archive_transaction_name().map_err(|error| error.to_string())?
    );
    let path = paths.root().join(&name);
    mkdirat(
        &transaction.root,
        name.as_str(),
        Mode::from_bits_truncate(0o700),
    )
    .map_err(|error| format!("cannot create private rescue directory: {error}"))?;
    let expected = statat(&transaction.root, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| format!("cannot inspect private rescue directory: {error}"))?;
    validate_private_directory(&expected, &path, "rescue directory")
        .map_err(|error| error.to_string())?;
    let directory = openat(
        &transaction.root,
        name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| format!("cannot open private rescue directory: {error}"))?;
    let opened = fstat(&directory)
        .map_err(|error| format!("cannot inspect opened rescue directory: {error}"))?;
    validate_private_directory(&opened, &path, "opened rescue directory")
        .map_err(|error| error.to_string())?;
    if directory_identity(&opened) != directory_identity(&expected) {
        return Err("opened rescue directory does not match created inode".to_owned());
    }
    Ok(RescueDirectory { path, directory })
}

#[cfg(target_os = "linux")]
fn validate_rescue_namespace(rescue: &RescueDirectory) -> Result<(), String> {
    use rustix::fs::{AtFlags, CWD, fstat, statat};

    let visible = statat(CWD, &rescue.path, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| format!("visible rescue directory is unavailable: {error}"))?;
    let opened = fstat(&rescue.directory)
        .map_err(|error| format!("opened rescue directory failed: {error}"))?;
    validate_private_directory(&opened, &rescue.path, "opened rescue directory")
        .map_err(|error| error.to_string())?;
    if directory_identity(&visible) != directory_identity(&opened) {
        return Err("visible rescue directory no longer identifies opened directory".to_owned());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_recovered_identity(
    directory: &OwnedFd,
    target: &ArchivedTarget,
    inspected: &[CleanInspection],
) -> Result<(), String> {
    use rustix::fs::{AtFlags, FileType, statat};

    let stat = statat(directory, target.name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| format!("cannot inspect recovered {}: {error}", target.name))?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(format!("recovered file type mismatch for {}", target.name));
    }
    let identity = file_identity_from_stat(&stat)?;
    if inspected[target.inspection_index].identity.as_ref() != Some(&identity) {
        return Err(format!(
            "recovered file identity mismatch for {}",
            inspected[target.inspection_index].logical
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn cleanup_rollback_error(cause: impl AsRef<str>, report: RecoveryReport) -> BossError {
    let paths = report
        .verified_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut message = format!("{}; no archived file was deleted", cause.as_ref());
    if !paths.is_empty() {
        let _ = write!(message, "; verified preserved at: {paths}");
    }
    if !report.unverified_targets.is_empty() {
        let _ = write!(
            message,
            "; no stable recovery path could be verified for: {}",
            report.unverified_targets.join(", ")
        );
    }
    if !report.issues.is_empty() {
        let _ = write!(message, "; recovery issues: {}", report.issues.join(" | "));
    }
    BossError::Cleanup(message)
}

#[cfg(target_os = "linux")]
fn file_identity_from_stat(stat: &rustix::fs::Stat) -> Result<FileIdentity, String> {
    Ok(FileIdentity {
        len: u64::try_from(stat.st_size)
            .map_err(|_| "archived file reported a negative size".to_owned())?,
        device: stat.st_dev,
        inode: stat.st_ino,
        modified_seconds: stat.st_mtime,
        modified_nanoseconds: i64::try_from(stat.st_mtime_nsec)
            .map_err(|_| "archived file reported invalid mtime nanoseconds".to_owned())?,
    })
}

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        FileIdentity {
            len: metadata.len(),
            device: metadata.dev(),
            inode: metadata.ino(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }
    #[cfg(not(unix))]
    {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
        FileIdentity {
            len: metadata.len(),
            modified,
        }
    }
}

fn now_seconds() -> Result<u64, BossError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| BossError::DataIo(error.to_string()))
}

fn known_file_sizes(paths: &DataPaths) -> Result<Value, BossError> {
    let mut sizes = serde_json::Map::new();
    for (name, path) in [
        ("jobs", paths.jobs()),
        ("history", paths.history()),
        ("shortlist", paths.shortlist()),
        ("presets", paths.presets()),
        ("reply_rules", paths.reply_rules()),
        ("watches", paths.watches()),
        ("resumes", paths.resumes()),
        ("campaign_policies", paths.campaign_policies()),
        ("campaign_blacklist", paths.campaign_blacklist()),
        ("greeting_templates", paths.greeting_templates()),
        ("application_plans", paths.application_plans()),
        ("ai_profiles", paths.ai_profiles()),
        ("notification_audit", paths.notification_audit()),
        ("config", paths.config()),
    ] {
        let bytes = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                metadata.len()
            }
            Ok(_) => 0,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(BossError::DataIo(error.to_string())),
        };
        sizes.insert(name.to_owned(), json!(bytes));
    }
    Ok(Value::Object(sizes))
}

fn clean_targets(
    paths: &DataPaths,
    target: &str,
) -> Result<Vec<(&'static str, PathBuf)>, BossError> {
    let all = || {
        vec![
            ("jobs", paths.jobs()),
            ("history", paths.history()),
            ("shortlist", paths.shortlist()),
            ("presets", paths.presets()),
            ("reply_rules", paths.reply_rules()),
            ("watches", paths.watches()),
            ("resumes", paths.resumes()),
            ("campaign_policies", paths.campaign_policies()),
            ("campaign_blacklist", paths.campaign_blacklist()),
            ("greeting_templates", paths.greeting_templates()),
            ("application_plans", paths.application_plans()),
            ("ai_profiles", paths.ai_profiles()),
            ("notification_audit", paths.notification_audit()),
        ]
    };
    if target == "all" {
        return Ok(all());
    }
    all()
        .into_iter()
        .find(|(name, _)| *name == target)
        .map(|entry| vec![entry])
        .ok_or_else(|| BossError::InvalidArgument(format!("unknown clean target: {target}")))
}

fn diagnose_local(
    paths: &DataPaths,
    cache: &JobCache,
    shortlist: &ShortlistStore,
    reply_rules: &ReplyStore,
    auth: &AuthStore,
    provider_count: usize,
    platform: Option<Platform>,
) -> Value {
    let mut checks = Vec::new();
    let mut has_error = false;
    let mut has_warn = false;

    let data_result = probe_data_root(paths.root());
    has_error |= data_result.is_err();
    checks.push(check(
        "data_root",
        if data_result.is_ok() { "ok" } else { "error" },
        data_result.err(),
    ));

    let config_result = ConfigStore::from_paths(paths).map(|_| ());
    has_error |= config_result.is_err();
    checks.push(check(
        "config",
        if config_result.is_ok() { "ok" } else { "error" },
        config_result.err().map(|error| error.to_string()),
    ));

    let jobs_result = cache.read_all().map(|_| ());
    has_error |= jobs_result.is_err();
    checks.push(check(
        "jobs_cache",
        if jobs_result.is_ok() { "ok" } else { "error" },
        jobs_result.err().map(|error| error.to_string()),
    ));

    let shortlist_result = shortlist.check_readable();
    has_error |= shortlist_result.is_err();
    checks.push(check(
        "shortlist",
        if shortlist_result.is_ok() {
            "ok"
        } else {
            "error"
        },
        shortlist_result.err().map(|error| error.to_string()),
    ));

    let reply_rules_result = reply_rules.check_readable();
    has_error |= reply_rules_result.is_err();
    checks.push(check(
        "reply_rules",
        if reply_rules_result.is_ok() {
            "ok"
        } else {
            "error"
        },
        reply_rules_result.err().map(|error| error.to_string()),
    ));

    let auth_store_status = auth.health().as_str();
    let auth_store_warn = auth_store_status == "unavailable";
    has_warn |= auth_store_warn;
    checks.push(check(
        "auth_store",
        if auth_store_warn { "warn" } else { "ok" },
        auth_store_warn.then(|| "private credential store was ignored".to_owned()),
    ));

    let registered = provider_count == 3;
    has_error |= !registered;
    checks.push(check(
        "provider_registration",
        if registered { "ok" } else { "error" },
        (!registered).then(|| format!("expected 3 providers, found {provider_count}")),
    ));

    for selected in selected_platforms(platform) {
        let variable = AuthStore::cookie_env(selected);
        let env_present = AuthStore::environment_cookie(selected).is_some();
        let stored_present = auth.has_session(selected);
        let present = env_present || stored_present;
        has_warn |= !present;
        checks.push(json!({
            "name":format!("cookie_{}",selected.as_str()),
            "status":if present {"ok"} else {"warn"},
            "message":if env_present {
                format!("{variable} is present")
            } else if stored_present {
                "private stored session is present".to_owned()
            } else {
                format!("{variable} and private stored session are missing")
            }
        }));
    }
    json!({
        "network_checked":false,
        "status":if has_error {"error"} else if has_warn {"warn"} else {"ok"},
        "checks":checks
    })
}

fn login_outcome(platform: Platform, state: &'static str, source: &'static str) -> Value {
    json!({"platform":platform,"state":state,"source":source})
}

fn selected_platforms(platform: Option<Platform>) -> Vec<Platform> {
    platform.map_or_else(
        || vec![Platform::Zhipin, Platform::Zhilian, Platform::Qiancheng],
        |selected| vec![selected],
    )
}

fn check(name: &str, status: &str, message: Option<String>) -> Value {
    json!({"name":name,"status":status,"message":message})
}

fn probe_data_root(root: &Path) -> Result<(), String> {
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    let missing_directories = missing_directory_chain(root)?;
    let probe_result = fs::create_dir_all(root)
        .map_err(|error| error.to_string())
        .and_then(|()| write_and_remove_probe(root));
    let directory_cleanup = cleanup_created_directories(&missing_directories);
    match (probe_result, directory_cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(probe_error), Ok(())) => Err(probe_error),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Err(probe_error), Err(cleanup_error)) => Err(format!(
            "{probe_error}; directory cleanup failed: {cleanup_error}"
        )),
    }
}

fn missing_directory_chain(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut missing = Vec::new();
    let mut current = root;
    loop {
        if current.try_exists().map_err(|error| error.to_string())? {
            return Ok(missing);
        }
        missing.push(current.to_path_buf());
        current = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
    }
}

fn write_and_remove_probe(root: &Path) -> Result<(), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let probe = root.join(format!(".bosskit-doctor-{}-{nonce}", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|error| error.to_string())?;
    drop(file);
    fs::remove_file(probe).map_err(|error| error.to_string())
}

fn cleanup_created_directories(directories: &[PathBuf]) -> Result<(), String> {
    for directory in directories {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "unable to remove probe-created directory {}: {error}",
                    directory.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[cfg(target_os = "linux")]
    use std::os::unix::fs::PermissionsExt;

    use async_trait::async_trait;
    use tempfile::tempdir;

    use super::*;

    struct MockProvider {
        platform: Platform,
        succeeds: bool,
    }

    #[async_trait]
    impl JobProvider for MockProvider {
        fn platform(&self) -> Platform {
            self.platform
        }

        async fn search(&self, _request: &SearchRequest<'_>) -> Result<Vec<Job>, BossError> {
            if self.succeeds {
                let mut job = Job::new(
                    "stable",
                    self.platform,
                    "remote",
                    "Rust",
                    "https://example.test/job",
                );
                job.company = "Example".to_owned();
                job.city = "深圳".to_owned();
                job.salary = "20K".to_owned();
                Ok(vec![job])
            } else {
                Err(BossError::Http {
                    status: 403,
                    message: "risk control".to_owned(),
                })
            }
        }

        async fn detail(&self, job: &Job) -> Result<Job, BossError> {
            if self.succeeds {
                let mut detailed = job.clone();
                detailed.description = "refreshed detail".to_owned();
                Ok(detailed)
            } else {
                Err(BossError::Parse("detail unavailable".to_owned()))
            }
        }
    }

    #[tokio::test]
    async fn search_preserves_partial_provider_outcomes() {
        let directory = tempdir().expect("temporary directory");
        let service = BossService {
            paths: DataPaths::new(directory.path()),
            cache: JobCache::new(directory.path()),
            config: ConfigStore::from_paths(&DataPaths::new(directory.path())).expect("config"),
            shortlist: ShortlistStore::from_paths(&DataPaths::new(directory.path())),
            history: SearchHistoryStore::from_paths(&DataPaths::new(directory.path())),
            presets: PresetStore::from_paths(&DataPaths::new(directory.path())),
            reply_rules: ReplyStore::from_paths(&DataPaths::new(directory.path())),
            watches: WatchStore::from_paths(&DataPaths::new(directory.path())),
            resumes: ResumeStore::from_paths(&DataPaths::new(directory.path())),
            campaigns: CampaignStore::from_paths(&DataPaths::new(directory.path())),
            ai_profiles: AiProfileStore::from_paths(&DataPaths::new(directory.path())),
            auth: AuthStore::from_paths(&DataPaths::new(directory.path())),
            providers: vec![
                Box::new(MockProvider {
                    platform: Platform::Zhipin,
                    succeeds: true,
                }),
                Box::new(MockProvider {
                    platform: Platform::Zhilian,
                    succeeds: false,
                }),
            ],
        };
        let report = service
            .search("rust", None, Some("深圳"), 1, 20, SearchFilters::default())
            .await
            .expect("search");
        assert!(report.providers[0].error.is_none());
        assert_eq!(
            report.providers[1].error.as_ref().expect("failure").code,
            "authentication_or_risk_control"
        );
        assert_eq!(
            service.history(None, 1).expect("history")[0]
                .providers
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn cache_write_failure_is_a_top_level_error() {
        let directory = tempdir().expect("temporary directory");
        let blocked = directory.path().join("not-a-directory");
        std::fs::write(&blocked, b"file").expect("blocking file");
        let service = BossService {
            paths: DataPaths::new(directory.path()),
            cache: JobCache::new(blocked),
            config: ConfigStore::from_paths(&DataPaths::new(directory.path())).expect("config"),
            shortlist: ShortlistStore::from_paths(&DataPaths::new(directory.path())),
            history: SearchHistoryStore::from_paths(&DataPaths::new(directory.path())),
            presets: PresetStore::from_paths(&DataPaths::new(directory.path())),
            reply_rules: ReplyStore::from_paths(&DataPaths::new(directory.path())),
            watches: WatchStore::from_paths(&DataPaths::new(directory.path())),
            resumes: ResumeStore::from_paths(&DataPaths::new(directory.path())),
            campaigns: CampaignStore::from_paths(&DataPaths::new(directory.path())),
            ai_profiles: AiProfileStore::from_paths(&DataPaths::new(directory.path())),
            auth: AuthStore::from_paths(&DataPaths::new(directory.path())),
            providers: vec![Box::new(MockProvider {
                platform: Platform::Zhipin,
                succeeds: true,
            })],
        };
        let result = service
            .search("rust", None, None, 1, 20, SearchFilters::default())
            .await;
        assert!(matches!(result, Err(BossError::CacheIo(_))));
    }

    #[tokio::test]
    async fn unsupported_all_platform_city_is_a_top_level_error() {
        let directory = tempdir().expect("temporary directory");
        let service = BossService {
            paths: DataPaths::new(directory.path()),
            cache: JobCache::new(directory.path()),
            config: ConfigStore::from_paths(&DataPaths::new(directory.path())).expect("config"),
            shortlist: ShortlistStore::from_paths(&DataPaths::new(directory.path())),
            history: SearchHistoryStore::from_paths(&DataPaths::new(directory.path())),
            presets: PresetStore::from_paths(&DataPaths::new(directory.path())),
            reply_rules: ReplyStore::from_paths(&DataPaths::new(directory.path())),
            watches: WatchStore::from_paths(&DataPaths::new(directory.path())),
            resumes: ResumeStore::from_paths(&DataPaths::new(directory.path())),
            campaigns: CampaignStore::from_paths(&DataPaths::new(directory.path())),
            ai_profiles: AiProfileStore::from_paths(&DataPaths::new(directory.path())),
            auth: AuthStore::from_paths(&DataPaths::new(directory.path())),
            providers: vec![Box::new(MockProvider {
                platform: Platform::Zhipin,
                succeeds: true,
            })],
        };
        let result = service
            .search("rust", None, Some("火星"), 1, 20, SearchFilters::default())
            .await;
        assert!(matches!(
            result,
            Err(BossError::InvalidArgument(message)) if message == "unsupported city: 火星"
        ));
    }

    #[test]
    fn status_never_contains_cookie_value() {
        // SAFETY: The test restores this dedicated process variable immediately.
        unsafe { std::env::set_var("BOSS_ZHIPIN_COOKIE", "do-not-print-this") };
        let service = BossService::discover().expect("service");
        let serialized = service.status(Some(Platform::Zhipin)).to_string();
        // SAFETY: Restore process state before returning.
        unsafe { std::env::remove_var("BOSS_ZHIPIN_COOKIE") };
        assert!(!serialized.contains("do-not-print-this"));
    }

    #[test]
    fn doctor_missing_cookies_warns_without_error() {
        let service = BossService::discover().expect("service");
        let report = service.doctor(None);
        assert_ne!(report["status"], "error");
    }

    #[test]
    fn doctor_probe_restores_nested_missing_path_to_preexisting_ancestor() {
        let parent = tempdir().expect("temporary directory");
        let first_created = parent.path().join("nested");
        let root = first_created.join("deeper").join("data-root");
        probe_data_root(&root).expect("probe");
        assert!(!first_created.exists());
    }

    #[test]
    fn doctor_probe_preserves_preexisting_empty_root() {
        let root = tempdir().expect("temporary directory");
        probe_data_root(root.path()).expect("probe");
        assert!(root.path().is_dir());
    }

    #[test]
    fn doctor_probe_preserves_preexisting_content() {
        let root = tempdir().expect("temporary directory");
        let content = root.path().join("keep.txt");
        std::fs::write(&content, b"keep").expect("seed content");
        probe_data_root(root.path()).expect("probe");
        assert_eq!(std::fs::read(content).expect("read content"), b"keep");
    }

    #[test]
    fn doctor_cleanup_preserves_injected_content_in_created_root_and_reports_failure() {
        let parent = tempdir().expect("temporary directory");
        let root = parent.path().join("nested").join("data-root");
        let missing = missing_directory_chain(&root).expect("missing chain");
        std::fs::create_dir_all(&root).expect("create path");
        let content = root.join("injected.txt");
        std::fs::write(&content, b"keep").expect("inject content");
        let cleanup = cleanup_created_directories(&missing);
        assert!(
            cleanup.is_err() && content.exists(),
            "cleanup={cleanup:?}, content={}",
            content.exists()
        );
    }

    #[test]
    fn doctor_cleanup_preserves_injected_content_in_created_parent_and_reports_failure() {
        let parent = tempdir().expect("temporary directory");
        let intermediate = parent.path().join("nested");
        let root = intermediate.join("data-root");
        let missing = missing_directory_chain(&root).expect("missing chain");
        std::fs::create_dir_all(&root).expect("create path");
        let content = intermediate.join("injected.txt");
        std::fs::write(&content, b"keep").expect("inject content");
        let cleanup = cleanup_created_directories(&missing);
        assert!(
            cleanup.is_err() && content.exists() && !root.exists(),
            "cleanup={cleanup:?}, content={}, root={}",
            content.exists(),
            root.exists()
        );
    }

    #[tokio::test]
    async fn detail_uses_enriched_cache_without_provider_and_refreshes_when_requested() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let cache = JobCache::from_paths(&paths);
        let mut job = Job::new("stable", Platform::Zhipin, "remote", "Rust", "https://job");
        job.description = "cached detail".to_owned();
        cache.save(std::slice::from_ref(&job)).expect("cache");
        let service = BossService {
            paths: paths.clone(),
            cache,
            config: ConfigStore::from_paths(&paths).expect("config"),
            shortlist: ShortlistStore::from_paths(&paths),
            history: SearchHistoryStore::from_paths(&paths),
            presets: PresetStore::from_paths(&paths),
            reply_rules: ReplyStore::from_paths(&paths),
            watches: WatchStore::from_paths(&paths),
            resumes: ResumeStore::from_paths(&paths),
            campaigns: CampaignStore::from_paths(&paths),
            ai_profiles: AiProfileStore::from_paths(&paths),
            auth: AuthStore::from_paths(&paths),
            providers: vec![Box::new(MockProvider {
                platform: Platform::Zhipin,
                succeeds: true,
            })],
        };
        let cached = service.detail("stable", false).await.expect("cached");
        let refreshed = service.detail("stable", true).await.expect("refresh");
        let persisted = service.show("stable").expect("show").expect("persisted");
        assert_eq!(
            (
                cached.description.as_str(),
                refreshed.description.as_str(),
                persisted.description.as_str(),
            ),
            ("cached detail", "refreshed detail", "refreshed detail")
        );
    }

    #[tokio::test]
    async fn total_provider_failure_is_recorded_in_local_history() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let service = BossService {
            paths: paths.clone(),
            cache: JobCache::from_paths(&paths),
            config: ConfigStore::from_paths(&paths).expect("config"),
            shortlist: ShortlistStore::from_paths(&paths),
            history: SearchHistoryStore::from_paths(&paths),
            presets: PresetStore::from_paths(&paths),
            reply_rules: ReplyStore::from_paths(&paths),
            watches: WatchStore::from_paths(&paths),
            resumes: ResumeStore::from_paths(&paths),
            campaigns: CampaignStore::from_paths(&paths),
            ai_profiles: AiProfileStore::from_paths(&paths),
            auth: AuthStore::from_paths(&paths),
            providers: vec![Box::new(MockProvider {
                platform: Platform::Zhipin,
                succeeds: false,
            })],
        };
        let report = service
            .search("rust", None, None, 1, 20, SearchFilters::default())
            .await
            .expect("report");
        let history = service.history(None, 1).expect("history");
        assert!(
            !report.has_success()
                && history[0].providers[0].error_code.as_deref()
                    == Some("authentication_or_risk_control")
        );
    }

    #[tokio::test]
    async fn history_persistence_failure_is_not_hidden() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::create_dir(paths.history()).expect("blocking history directory");
        let service = BossService {
            paths: paths.clone(),
            cache: JobCache::from_paths(&paths),
            config: ConfigStore::from_paths(&paths).expect("config"),
            shortlist: ShortlistStore::from_paths(&paths),
            history: SearchHistoryStore::from_paths(&paths),
            presets: PresetStore::from_paths(&paths),
            reply_rules: ReplyStore::from_paths(&paths),
            watches: WatchStore::from_paths(&paths),
            resumes: ResumeStore::from_paths(&paths),
            campaigns: CampaignStore::from_paths(&paths),
            ai_profiles: AiProfileStore::from_paths(&paths),
            auth: AuthStore::from_paths(&paths),
            providers: vec![Box::new(MockProvider {
                platform: Platform::Zhipin,
                succeeds: true,
            })],
        };
        let result = service
            .search("rust", None, None, 1, 20, SearchFilters::default())
            .await;
        assert!(matches!(result, Err(BossError::HistoryIo(_))));
    }

    #[test]
    fn search_spec_resolution_uses_config_then_explicit_overrides() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let mut service = BossService::from_paths(paths).expect("service");
        service.config_set("page_size", "7").expect("config");
        let ordinary = service
            .resolve_search_spec(
                None,
                SearchSpecPatch {
                    query: Some("rust".to_owned()),
                    ..SearchSpecPatch::default()
                },
            )
            .expect("ordinary");
        service.preset_add("saved", ordinary).expect("preset");
        let overridden = service
            .resolve_search_spec(
                Some("saved"),
                SearchSpecPatch {
                    query: Some("backend".to_owned()),
                    limit: Some(3),
                    company: Some("Example".to_owned()),
                    ..SearchSpecPatch::default()
                },
            )
            .expect("override");
        assert_eq!(
            (
                overridden.query,
                overridden.limit,
                overridden.filters.company,
            ),
            ("backend".to_owned(), 3, Some("Example".to_owned()))
        );
    }

    #[test]
    fn stats_reports_exact_local_counts_and_file_sizes() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let service = BossService::from_paths(paths.clone()).expect("service");
        let mut job = Job::new(
            "job",
            Platform::Zhipin,
            "remote",
            "Rust",
            "https://example.test",
        );
        job.description = "detail".to_owned();
        service.cache.save(&[job]).expect("cache");
        let stats = service.stats(30).expect("stats");
        assert!(
            stats["jobs"]["total"] == 1
                && stats["jobs"]["enriched"] == 1
                && stats["jobs"]["by_platform"]["zhipin"] == 1
                && stats["file_bytes"]["jobs"]
                    .as_u64()
                    .is_some_and(|size| size > 0)
        );
    }

    #[test]
    fn clean_all_preserves_config_and_rejects_non_files() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.config(), b"{}").expect("config");
        std::fs::create_dir(paths.jobs()).expect("blocking directory");
        let service = BossService::from_paths(paths.clone()).expect("service");
        assert!(matches!(
            service.clean("all", true),
            Err(BossError::Cleanup(_))
        ));
        assert!(paths.config().is_file() && paths.jobs().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn clean_rejects_symlinks_without_touching_their_targets() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path().join("data"));
        std::fs::create_dir_all(paths.root()).expect("data root");
        let target = directory.path().join("outside.json");
        std::fs::write(&target, b"keep").expect("target");
        std::os::unix::fs::symlink(&target, paths.jobs()).expect("symlink");
        let service = BossService::from_paths(paths).expect("service");
        assert!(matches!(
            service.clean("jobs", true),
            Err(BossError::Cleanup(_))
        ));
        assert_eq!(std::fs::read(target).expect("target remains"), b"keep");
    }

    #[cfg(target_os = "linux")]
    fn archive_transactions(root: &Path) -> Vec<PathBuf> {
        let archive = root.join(".bosskit-clean-archive");
        std::fs::read_dir(archive)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_swap_before_rename_preserves_replacement_and_displaced_inode() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::create_dir_all(paths.root()).expect("data root");
        std::fs::write(paths.jobs(), b"original").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        let held = directory.path().join("held-original.json");
        let result = service.clean_with_hook("jobs", true, |stage, _, original, _| {
            if stage == CleanStage::BeforeArchiveMove {
                std::fs::rename(original, &held).expect("hold original");
                std::fs::write(original, b"replacement").expect("replacement");
            }
        });
        assert!(
            result.is_err_and(|error| error.to_string().contains("no archived file was deleted"))
                && std::fs::read(paths.jobs()).expect("replacement remains") == b"replacement"
                && std::fs::read(held).expect("original remains") == b"original"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_creation_at_original_path_after_archive_move_remains_untouched() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.jobs(), b"captured").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        let result = service.clean_with_hook("jobs", true, |stage, _, original, _| {
            if stage == CleanStage::AfterArchiveMoveVerified {
                std::fs::write(original, b"new").expect("new original");
            }
        });
        let recovery = result
            .as_ref()
            .ok()
            .and_then(|value| value["files"][0]["recovery_path"].as_str())
            .map(PathBuf::from);
        assert!(
            recovery
                .is_some_and(|path| std::fs::read(path).is_ok_and(|bytes| bytes == b"captured"))
                && std::fs::read(paths.jobs()).expect("new remains") == b"new"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_namespace_exchange_with_blocked_rollback_reports_verified_rescue_path() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.jobs(), b"captured").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        let mut stale_recovery = None;
        let result = service.clean_with_hook("jobs", true, |stage, _, original, recovery| {
            if stage == CleanStage::AfterArchiveMoveVerified {
                let transaction = recovery.parent().expect("transaction");
                let archive = transaction.parent().expect("archive");
                let displaced = paths.root().join("archive-displaced");
                let transaction_name = transaction.file_name().expect("transaction name");
                std::fs::rename(archive, &displaced).expect("displace archive root");
                std::fs::create_dir(archive).expect("replacement archive root");
                std::fs::set_permissions(archive, std::fs::Permissions::from_mode(0o700))
                    .expect("archive permissions");
                let replacement_transaction = archive.join(transaction_name);
                std::fs::create_dir(&replacement_transaction)
                    .expect("replacement archive transaction");
                std::fs::set_permissions(
                    replacement_transaction,
                    std::fs::Permissions::from_mode(0o700),
                )
                .expect("transaction permissions");
                std::fs::write(original, b"new").expect("new active file");
                stale_recovery = Some(recovery.to_path_buf());
            }
        });
        let error = result
            .expect_err("namespace exchange must fail")
            .to_string();
        let stale_recovery = stale_recovery.expect("stale recovery path");
        let rescue = std::fs::read_dir(paths.root())
            .expect("data root")
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".bosskit-clean-recovery-")
            })
            .map(|entry| entry.path())
            .expect("rescue directory");
        let rescued_file = rescue.join("jobs.json");
        assert!(
            !error.contains(&stale_recovery.display().to_string())
                && error.contains(&rescued_file.display().to_string())
                && std::fs::read(paths.jobs()).expect("new active file remains") == b"new"
                && std::fs::read(&rescued_file).expect("rescued file") == b"captured"
                && std::fs::symlink_metadata(rescue)
                    .expect("rescue metadata")
                    .permissions()
                    .mode()
                    & 0o777
                    == 0o700
                && !stale_recovery.exists()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_second_target_mismatch_rolls_back_first_without_deleting() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        std::fs::write(paths.history(), b"history-original").expect("history");
        let service = BossService::from_paths(paths.clone()).expect("service");
        let displaced = directory.path().join("history-displaced.json");
        let result = service.clean_with_hook("all", true, |stage, index, original, _| {
            if stage == CleanStage::BeforeArchiveMove && index == 1 {
                std::fs::rename(original, &displaced).expect("displace history");
                std::fs::write(original, b"history-replacement").expect("replacement");
            }
        });
        assert!(
            result.is_err()
                && std::fs::read(paths.jobs()).expect("jobs restored") == b"jobs"
                && std::fs::read(paths.history()).expect("replacement restored")
                    == b"history-replacement"
                && std::fs::read(displaced).expect("history preserved") == b"history-original"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_archive_collision_never_clobbers_existing_file() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        let mut collision = None;
        let result = service.clean_with_hook("jobs", true, |stage, _, _, recovery| {
            if stage == CleanStage::BeforeArchiveMove {
                std::fs::write(recovery, b"collision").expect("collision");
                collision = Some(recovery.to_path_buf());
            }
        });
        let collision = collision.expect("collision path");
        assert!(
            result.is_err()
                && std::fs::read(paths.jobs()).expect("jobs remains") == b"jobs"
                && std::fs::read(collision).expect("collision remains") == b"collision"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn archive_transaction_name_collision_never_clobbers_existing_transaction() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let first =
            create_archive_transaction_with_name(&paths, "fixed".to_owned(), &mut |_, _, _, _| {})
                .expect("first transaction");
        let marker = first.path.join("marker");
        std::fs::write(&marker, b"keep").expect("marker");
        let second =
            create_archive_transaction_with_name(&paths, "fixed".to_owned(), &mut |_, _, _, _| {});
        assert!(
            second.is_err()
                && std::fs::read(marker).expect("marker remains") == b"keep"
                && archive_transactions(paths.root()).len() == 1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_success_archives_exact_files_without_temporary_residue() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        std::fs::write(paths.history(), b"history").expect("history");
        let jobs_identity =
            file_identity(&std::fs::symlink_metadata(paths.jobs()).expect("metadata"));
        let service = BossService::from_paths(paths.clone()).expect("service");
        let result = service.clean("all", true).expect("clean");
        let recovery = result["files"]
            .as_array()
            .expect("files")
            .iter()
            .find(|file| file["target"] == "jobs")
            .and_then(|file| file["recovery_path"].as_str())
            .map(PathBuf::from)
            .expect("recovery path");
        let recovered_identity =
            file_identity(&std::fs::symlink_metadata(&recovery).expect("metadata"));
        let transaction = PathBuf::from(
            result["archive_transaction"]
                .as_str()
                .expect("archive transaction"),
        );
        assert!(
            result["action"] == "archive"
                && result["recoverable"] == true
                && !paths.jobs().exists()
                && !paths.history().exists()
                && std::fs::read(recovery).expect("archived jobs") == b"jobs"
                && recovered_identity == jobs_identity
                && std::fs::read_dir(transaction)
                    .expect("transaction")
                    .filter_map(Result::ok)
                    .count()
                    == 2
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn repeated_clean_does_not_rearchive_previous_transactions() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        service.clean("all", true).expect("first clean");
        let before = archive_transactions(paths.root());
        let repeated = service.clean("all", true).expect("repeated clean");
        assert!(
            before.len() == 1
                && archive_transactions(paths.root()) == before
                && repeated["archive_transaction"].is_null()
                && repeated["files"]
                    .as_array()
                    .is_some_and(|files| files.iter().all(|file| file["archived"] == false))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_rejects_archive_root_exchange_between_stat_and_open() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let archive = paths.root().join(".bosskit-clean-archive");
        let displaced = paths.root().join("archive-root-displaced");
        std::fs::create_dir(&archive).expect("archive root");
        std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o700))
            .expect("archive permissions");
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        let result = service.clean_with_hook("all", true, |stage, _, _, _| {
            if stage == CleanStage::AfterArchiveRootChecked {
                std::fs::rename(&archive, &displaced).expect("displace archive root");
                std::fs::create_dir(&archive).expect("replacement archive root");
                std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o700))
                    .expect("replacement permissions");
            }
        });
        assert!(
            result.is_err()
                && std::fs::read(paths.jobs()).expect("jobs remains") == b"jobs"
                && displaced.is_dir()
                && archive.is_dir()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_rejects_transaction_exchange_between_mkdir_and_open() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        let mut displaced = None;
        let result = service.clean_with_hook("all", true, |stage, _, transaction, _| {
            if stage == CleanStage::AfterTransactionCreated {
                let held = transaction.with_extension("displaced");
                std::fs::rename(transaction, &held).expect("displace transaction");
                std::fs::create_dir(transaction).expect("replacement transaction");
                std::fs::set_permissions(transaction, std::fs::Permissions::from_mode(0o700))
                    .expect("replacement permissions");
                displaced = Some(held);
            }
        });
        assert!(
            result.is_err()
                && std::fs::read(paths.jobs()).expect("jobs remains") == b"jobs"
                && displaced.is_some_and(|path| path.is_dir())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_rejects_existing_archive_root_with_mode_0755() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let archive = paths.root().join(".bosskit-clean-archive");
        std::fs::create_dir(&archive).expect("archive root");
        std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o755))
            .expect("archive permissions");
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        let service = BossService::from_paths(paths.clone()).expect("service");
        assert!(
            service.clean("all", true).is_err()
                && std::fs::read(paths.jobs()).expect("jobs remains") == b"jobs"
                && archive.is_dir()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clean_refuses_malicious_symlink_archive_root() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path().join("data"));
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(paths.root()).expect("data root");
        std::fs::create_dir(&outside).expect("outside");
        std::fs::write(paths.jobs(), b"jobs").expect("jobs");
        std::os::unix::fs::symlink(&outside, paths.root().join(".bosskit-clean-archive"))
            .expect("archive symlink");
        let service = BossService::from_paths(paths.clone()).expect("service");
        assert!(
            service.clean("all", true).is_err()
                && std::fs::read(paths.jobs()).expect("jobs remains") == b"jobs"
                && std::fs::read_dir(outside)
                    .expect("outside")
                    .next()
                    .is_none()
        );
    }

    #[tokio::test]
    async fn total_watch_failure_does_not_mutate_seen_or_last_run() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let service = BossService {
            paths: paths.clone(),
            cache: JobCache::from_paths(&paths),
            config: ConfigStore::from_paths(&paths).expect("config"),
            shortlist: ShortlistStore::from_paths(&paths),
            history: SearchHistoryStore::from_paths(&paths),
            presets: PresetStore::from_paths(&paths),
            reply_rules: ReplyStore::from_paths(&paths),
            watches: WatchStore::from_paths(&paths),
            resumes: ResumeStore::from_paths(&paths),
            campaigns: CampaignStore::from_paths(&paths),
            ai_profiles: AiProfileStore::from_paths(&paths),
            auth: AuthStore::from_paths(&paths),
            providers: vec![Box::new(MockProvider {
                platform: Platform::Zhipin,
                succeeds: false,
            })],
        };
        let spec = service
            .resolve_search_spec(
                None,
                SearchSpecPatch {
                    query: Some("rust".to_owned()),
                    platform: Some(PlatformSelector::Zhipin),
                    ..SearchSpecPatch::default()
                },
            )
            .expect("spec");
        service.watch_add("daily", spec).expect("watch");
        let before = service.watch_show("daily").expect("before");
        let outcome = service.watch_run("daily").await.expect("run");
        let after = service.watch_show("daily").expect("after");
        assert!(outcome["ok"] == false && before == after);
    }

    #[tokio::test]
    async fn run_all_preserves_mixed_watch_outcomes() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let service = BossService {
            paths: paths.clone(),
            cache: JobCache::from_paths(&paths),
            config: ConfigStore::from_paths(&paths).expect("config"),
            shortlist: ShortlistStore::from_paths(&paths),
            history: SearchHistoryStore::from_paths(&paths),
            presets: PresetStore::from_paths(&paths),
            reply_rules: ReplyStore::from_paths(&paths),
            watches: WatchStore::from_paths(&paths),
            resumes: ResumeStore::from_paths(&paths),
            campaigns: CampaignStore::from_paths(&paths),
            ai_profiles: AiProfileStore::from_paths(&paths),
            auth: AuthStore::from_paths(&paths),
            providers: vec![
                Box::new(MockProvider {
                    platform: Platform::Zhipin,
                    succeeds: true,
                }),
                Box::new(MockProvider {
                    platform: Platform::Zhilian,
                    succeeds: false,
                }),
            ],
        };
        for (name, platform) in [
            ("success", PlatformSelector::Zhipin),
            ("failure", PlatformSelector::Zhilian),
        ] {
            let spec = service
                .resolve_search_spec(
                    None,
                    SearchSpecPatch {
                        query: Some("rust".to_owned()),
                        platform: Some(platform),
                        ..SearchSpecPatch::default()
                    },
                )
                .expect("spec");
            service.watch_add(name, spec).expect("watch");
        }
        let outcomes = service.watch_run_all().await.expect("run all");
        assert_eq!(
            outcomes
                .iter()
                .map(|outcome| outcome["ok"].as_bool())
                .collect::<Vec<_>>(),
            vec![Some(true), Some(false)]
        );
    }

    #[test]
    fn resume_import_rejects_oversized_and_unknown_schema() {
        let directory = tempdir().expect("temporary directory");
        let paths = DataPaths::new(directory.path());
        let service = BossService::from_paths(paths).expect("service");
        let oversized = directory.path().join("oversized.json");
        std::fs::write(&oversized, vec![b' '; 2 * 1024 * 1024 + 1]).expect("oversized");
        let unknown = directory.path().join("unknown.json");
        std::fs::write(&unknown, br#"{"name":"x","unknown":true}"#).expect("unknown");
        assert!(
            service.resume_import(&oversized, false).is_err()
                && service.resume_import(&unknown, false).is_err()
        );
    }

    #[test]
    fn resume_import_reads_actual_size_from_one_open_handle() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("growing.json");
        std::fs::write(&path, br#"{"name":"small"}"#).expect("small");
        let service = BossService::from_paths(DataPaths::new(directory.path())).expect("service");
        let result = service.resume_import_with_hook(&path, false, || {
            std::fs::write(&path, vec![b' '; MAX_RESUME_IMPORT_BYTES + 1]).expect("grow");
        });
        assert!(
            matches!(result, Err(BossError::Resume(message)) if message.contains("exceeds 2 MiB"))
        );
    }

    #[test]
    fn resume_import_rejects_directories_and_accepts_uppercase_json_extension() {
        let directory = tempdir().expect("temporary directory");
        let service = BossService::from_paths(DataPaths::new(directory.path())).expect("service");
        let directory_input = directory.path().join("directory.json");
        std::fs::create_dir(&directory_input).expect("directory input");
        let uppercase = directory.path().join("resume.JSON");
        std::fs::write(
            &uppercase,
            br#"{"name":"imported","title":"","summary":"","basics":{},"skills":[],"experience":[],"education":[],"projects":[],"created_at":1,"updated_at":1}"#,
        )
        .expect("resume");
        assert!(
            service.resume_import(&directory_input, false).is_err()
                && service
                    .resume_import(&uppercase, false)
                    .is_ok_and(|document| document.name == "imported")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resume_import_rejects_symlink_without_reading_target() {
        let directory = tempdir().expect("temporary directory");
        let service = BossService::from_paths(DataPaths::new(directory.path())).expect("service");
        let target = directory.path().join("target.json");
        let link = directory.path().join("link.json");
        std::fs::write(
            &target,
            br#"{"name":"target","title":"","summary":"","basics":{},"skills":[],"experience":[],"education":[],"projects":[],"created_at":1,"updated_at":1}"#,
        )
        .expect("target");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(
            matches!(service.resume_import(&link, false), Err(BossError::Resume(message)) if message.contains("symlinks"))
        );
    }
}
