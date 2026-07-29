//! Strictly local campaign planning over cached normalized jobs.
//!
//! This module deliberately has no provider, browser, network, or credential
//! dependency. Its plans are review artifacts only and never submit an
//! application or send a greeting.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::data::atomic_write;
use crate::resume::ResumeDocument;
use crate::{BossError, DataPaths, Job};

pub const MAX_CAMPAIGN_NAME_CHARS: usize = 64;
pub const MAX_RULE_VALUE_CHARS: usize = 256;
pub const MAX_RULES: usize = 32;
pub const MAX_WELFARE_REQUIREMENTS: usize = 16;
pub const MAX_TEMPLATE_CHARS: usize = 2_000;
pub const MAX_GREETING_PREVIEW_CHARS: usize = 4_000;
pub const MAX_PLANS_PER_BUILD: usize = 100;
pub const MAX_STATE_NOTE_CHARS: usize = 1_000;
pub const DEFAULT_MINIMUM_RESUME_SCORE: u8 = 40;

/// Normalized cached-job field available to a local campaign rule.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignField {
    Title,
    Company,
    City,
    District,
    Salary,
    Experience,
    Education,
    EmploymentType,
    Skills,
    Welfare,
    Description,
    Address,
}

impl CampaignField {
    /// Returns the stable CLI and MCP value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Company => "company",
            Self::City => "city",
            Self::District => "district",
            Self::Salary => "salary",
            Self::Experience => "experience",
            Self::Education => "education",
            Self::EmploymentType => "employment_type",
            Self::Skills => "skills",
            Self::Welfare => "welfare",
            Self::Description => "description",
            Self::Address => "address",
        }
    }

    fn text(self, job: &Job) -> String {
        match self {
            Self::Title => job.title.clone(),
            Self::Company => job.company.clone(),
            Self::City => job.city.clone(),
            Self::District => job.district.clone(),
            Self::Salary => job.salary.clone(),
            Self::Experience => job.experience.clone(),
            Self::Education => job.education.clone(),
            Self::EmploymentType => job.employment_type.clone(),
            Self::Skills => job.skills.join(" "),
            Self::Welfare => job.welfare.join(" "),
            Self::Description => job.description.clone(),
            Self::Address => job.address.clone(),
        }
    }
}

/// One normalized local field substring rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignRule {
    pub field: CampaignField,
    pub value: String,
}

/// A named reusable policy applied only to locally cached jobs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignPolicy {
    pub name: String,
    /// Positive preference rules. Their match ratio becomes `score`.
    #[serde(default)]
    pub include: Vec<CampaignRule>,
    /// Any matching rule removes a job before it can be planned.
    #[serde(default)]
    pub exclude: Vec<CampaignRule>,
    /// Every normalized welfare token must occur in the job welfare labels.
    #[serde(default)]
    pub required_welfare: Vec<String>,
    /// Optional lower monthly salary bound in yuan.
    #[serde(default)]
    pub monthly_salary_min: Option<u32>,
    /// Optional upper monthly salary bound in yuan.
    #[serde(default)]
    pub monthly_salary_max: Option<u32>,
    /// Optional 0..=100 floor over the positive-rule match ratio.
    #[serde(default)]
    pub minimum_score: Option<u8>,
}

/// Deterministic local policy evaluation for one cached job.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PolicyEvaluation {
    pub eligible: bool,
    pub score: u8,
    pub excluded_by: Option<String>,
}

/// A local blacklist target category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlacklistKind {
    Company,
    Description,
    Job,
}

impl BlacklistKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Company => "company",
            Self::Description => "description",
            Self::Job => "job",
        }
    }
}

/// One strictly local blacklist rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlacklistRule {
    pub kind: BlacklistKind,
    pub value: String,
    pub created_at: u64,
}

/// A named greeting template that can only render allow-listed job fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GreetingTemplate {
    pub name: String,
    pub body: String,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Strict local-only lifecycle for an application plan.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationPlanState {
    /// The plan awaits a human decision.
    #[default]
    ManualReview,
    /// A human approved the locally rendered plan.
    Approved,
    /// A human rejected the plan; this is terminal.
    Rejected,
    /// A human attested that they submitted externally; this is terminal.
    RecordedSubmitted,
}

impl ApplicationPlanState {
    /// Returns the stable CLI, MCP, and persisted value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ManualReview => "manual_review",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::RecordedSubmitted => "recorded_submitted",
        }
    }

    fn permits(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::ManualReview, Self::Approved | Self::Rejected)
                | (Self::Approved, Self::RecordedSubmitted | Self::Rejected)
        )
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Rejected | Self::RecordedSubmitted)
    }
}

impl FromStr for ApplicationPlanState {
    type Err = BossError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "manual_review" => Ok(Self::ManualReview),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "recorded_submitted" => Ok(Self::RecordedSubmitted),
            other => Err(BossError::InvalidArgument(format!(
                "unknown campaign plan state: {other}"
            ))),
        }
    }
}

/// A local application plan which never performs a remote write.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationPlan {
    pub job_id: String,
    pub job_title: String,
    pub company: String,
    pub policy_name: String,
    pub template_name: Option<String>,
    /// Bound local resume name, not a copy of its private content.
    #[serde(default)]
    pub resume_name: Option<String>,
    /// Bound local resume's update timestamp at plan creation.
    #[serde(default)]
    pub resume_updated_at: Option<u64>,
    /// Policy-only score before any resume weighting.
    #[serde(default)]
    pub policy_score: Option<u8>,
    /// Deterministic score from explicit resume title and skills only.
    #[serde(default)]
    pub resume_score: Option<u8>,
    /// Whether the explicit resume title matched cached job text.
    #[serde(default)]
    pub title_match: bool,
    /// Explicit resume skills found in cached job text, in resume order.
    #[serde(default)]
    pub matched_skills: Vec<String>,
    /// Policy score for plan creation, or the weighted score for resume screening.
    pub score: u8,
    #[serde(default)]
    pub state: ApplicationPlanState,
    /// Timestamp of the most recent local state transition.
    #[serde(default)]
    pub state_changed_at: u64,
    /// Optional local-only transition note.
    #[serde(default)]
    pub state_note: Option<String>,
    pub dry_run: bool,
    pub created_at: u64,
}

/// An ephemeral template rendering returned only by plan creation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanGreetingPreview {
    /// Stable local job identifier for this rendering.
    pub job_id: String,
    /// Always false: a preview is never sent to a platform.
    pub sent: bool,
    /// Locally rendered candidate-context text; never persisted with the plan.
    pub text: String,
}

/// The result of one local review-plan build.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanBuildResult {
    pub mode: String,
    pub dry_run: bool,
    pub considered: usize,
    pub eligible: usize,
    pub planned: usize,
    pub skipped_existing: usize,
    pub skipped_blacklist: usize,
    pub skipped_resume_score: usize,
    pub plans: Vec<ApplicationPlan>,
    /// Ephemeral local renderings for this create response only.
    pub greeting_previews: Vec<PlanGreetingPreview>,
}

/// Exact counts of campaign-local persistent data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CampaignStats {
    pub policies: usize,
    pub blacklist: CampaignBlacklistStats,
    pub templates: usize,
    pub plans: CampaignPlanStats,
}

/// Counts grouped by blacklist kind.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CampaignBlacklistStats {
    pub total: usize,
    pub company: usize,
    pub description: usize,
    pub job: usize,
}

/// Counts grouped by immutable review-plan state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CampaignPlanStats {
    pub total: usize,
    pub manual_review: usize,
    pub approved: usize,
    pub rejected: usize,
    pub recorded_submitted: usize,
    pub dry_run: usize,
}

/// Atomic stores for every campaign-local JSON collection.
#[derive(Clone, Debug)]
pub struct CampaignStore {
    policies_path: PathBuf,
    blacklist_path: PathBuf,
    templates_path: PathBuf,
    plans_path: PathBuf,
}

#[derive(Clone, Debug)]
struct PlanCandidate<'a> {
    job: &'a Job,
    score: u8,
    policy_score: u8,
    resume_score: Option<u8>,
    title_match: bool,
    matched_skills: Vec<String>,
}

#[derive(Debug)]
struct GatedCandidates<'a> {
    considered: usize,
    skipped_blacklist: usize,
    candidates: Vec<PlanCandidate<'a>>,
}

struct PlanPersistence<'a> {
    policy: &'a CampaignPolicy,
    template: Option<&'a GreetingTemplate>,
    resume: Option<&'a ResumeDocument>,
    limit: usize,
    now: u64,
    mode: &'static str,
    eligible: usize,
    skipped_resume_score: usize,
    always_preview: bool,
}

pub(crate) struct ScreenPlanOptions<'a> {
    pub(crate) template: Option<&'a GreetingTemplate>,
    pub(crate) limit: usize,
    pub(crate) minimum_resume_score: u8,
    pub(crate) now: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResumeEvaluation {
    score: u8,
    title_match: bool,
    matched_skills: Vec<String>,
}

#[derive(Clone, Copy)]
enum ResumeRenderContext<'a> {
    Unbound,
    Full(&'a ResumeDocument),
    Screening {
        title: &'a str,
        matched_skills: &'a [String],
    },
}

impl CampaignStore {
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            policies_path: paths.campaign_policies(),
            blacklist_path: paths.campaign_blacklist(),
            templates_path: paths.greeting_templates(),
            plans_path: paths.application_plans(),
        }
    }

    pub fn add_policy(&self, policy: CampaignPolicy) -> Result<CampaignPolicy, BossError> {
        let policy = normalize_policy(policy)?;
        let mut policies = self.read_policies()?;
        if let Some(existing) = policies
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&policy.name))
        {
            *existing = policy.clone();
        } else {
            policies.push(policy.clone());
        }
        self.save_policies(&policies)?;
        Ok(policy)
    }

    pub fn list_policies(&self) -> Result<Vec<CampaignPolicy>, BossError> {
        self.read_policies()
    }

    pub fn show_policy(&self, name: &str) -> Result<CampaignPolicy, BossError> {
        let name = normalize_name(name, "policy name")?;
        self.read_policies()?
            .into_iter()
            .find(|policy| policy.name.eq_ignore_ascii_case(&name))
            .ok_or_else(|| BossError::Campaign(format!("policy not found: {name}")))
    }

    pub fn remove_policy(&self, name: &str) -> Result<CampaignPolicy, BossError> {
        let name = normalize_name(name, "policy name")?;
        let mut policies = self.read_policies()?;
        let index = policies
            .iter()
            .position(|policy| policy.name.eq_ignore_ascii_case(&name))
            .ok_or_else(|| BossError::Campaign(format!("policy not found: {name}")))?;
        let removed = policies.remove(index);
        self.save_policies(&policies)?;
        Ok(removed)
    }

    pub fn add_blacklist(
        &self,
        kind: BlacklistKind,
        value: &str,
        now: u64,
    ) -> Result<BlacklistRule, BossError> {
        let value = normalize_rule_value(value, "blacklist value")?;
        let mut rules = self.read_blacklist()?;
        if let Some(rule) = rules
            .iter()
            .find(|rule| rule.kind == kind && rule.value.eq_ignore_ascii_case(&value))
        {
            return Ok(rule.clone());
        }
        let rule = BlacklistRule {
            kind,
            value,
            created_at: now,
        };
        rules.push(rule.clone());
        self.save_blacklist(&rules)?;
        Ok(rule)
    }

    pub fn list_blacklist(&self) -> Result<Vec<BlacklistRule>, BossError> {
        self.read_blacklist()
    }

    pub fn remove_blacklist(
        &self,
        kind: BlacklistKind,
        value: &str,
    ) -> Result<BlacklistRule, BossError> {
        let value = normalize_rule_value(value, "blacklist value")?;
        let mut rules = self.read_blacklist()?;
        let index = rules
            .iter()
            .position(|rule| rule.kind == kind && rule.value.eq_ignore_ascii_case(&value))
            .ok_or_else(|| {
                BossError::Campaign(format!(
                    "blacklist rule not found: {}:{value}",
                    kind.as_str()
                ))
            })?;
        let removed = rules.remove(index);
        self.save_blacklist(&rules)?;
        Ok(removed)
    }

    pub fn add_template(
        &self,
        name: &str,
        body: &str,
        now: u64,
    ) -> Result<GreetingTemplate, BossError> {
        let name = normalize_name(name, "template name")?;
        let body = normalize_template(body)?;
        let mut templates = self.read_templates()?;
        if let Some(template) = templates
            .iter_mut()
            .find(|template| template.name.eq_ignore_ascii_case(&name))
        {
            template.body = body;
            template.updated_at = now;
            let updated = template.clone();
            self.save_templates(&templates)?;
            return Ok(updated);
        }
        let template = GreetingTemplate {
            name,
            body,
            created_at: now,
            updated_at: now,
        };
        templates.push(template.clone());
        self.save_templates(&templates)?;
        Ok(template)
    }

    pub fn list_templates(&self) -> Result<Vec<GreetingTemplate>, BossError> {
        self.read_templates()
    }

    pub fn show_template(&self, name: &str) -> Result<GreetingTemplate, BossError> {
        let name = normalize_name(name, "template name")?;
        self.read_templates()?
            .into_iter()
            .find(|template| template.name.eq_ignore_ascii_case(&name))
            .ok_or_else(|| BossError::Campaign(format!("template not found: {name}")))
    }

    pub fn remove_template(&self, name: &str) -> Result<GreetingTemplate, BossError> {
        let name = normalize_name(name, "template name")?;
        let mut templates = self.read_templates()?;
        let index = templates
            .iter()
            .position(|template| template.name.eq_ignore_ascii_case(&name))
            .ok_or_else(|| BossError::Campaign(format!("template not found: {name}")))?;
        let removed = templates.remove(index);
        self.save_templates(&templates)?;
        Ok(removed)
    }

    pub fn render_template(&self, name: &str, job: &Job) -> Result<String, BossError> {
        let template = self.show_template(name)?;
        render_body(&template.body, job, ResumeRenderContext::Unbound)
    }

    pub fn build_plans(
        &self,
        jobs: &[Job],
        policy: &CampaignPolicy,
        template: Option<&GreetingTemplate>,
        resume: Option<&ResumeDocument>,
        limit: usize,
        now: u64,
    ) -> Result<PlanBuildResult, BossError> {
        validate_plan_limit(limit)?;
        let gated = self.gate_candidates(jobs, policy)?;
        let eligible = gated.candidates.len();
        self.persist_candidates(
            gated,
            PlanPersistence {
                policy,
                template,
                resume,
                limit,
                now,
                mode: "manual_review",
                eligible,
                skipped_resume_score: 0,
                always_preview: false,
            },
        )
    }

    /// Screens cached jobs against one explicit local resume and persists only review plans.
    pub(crate) fn screen_plans(
        &self,
        jobs: &[Job],
        policy: &CampaignPolicy,
        resume: &ResumeDocument,
        options: ScreenPlanOptions<'_>,
    ) -> Result<PlanBuildResult, BossError> {
        validate_plan_limit(options.limit)?;
        if options.minimum_resume_score > 100 {
            return Err(BossError::InvalidArgument(
                "minimum resume score must be in 0..=100".to_owned(),
            ));
        }
        validate_screen_resume(resume)?;
        if let Some(template) = options.template {
            validate_screening_template(&template.body)?;
        }

        let mut gated = self.gate_candidates(jobs, policy)?;
        let policy_eligible = gated.candidates.len();
        gated.candidates = gated
            .candidates
            .into_iter()
            .filter_map(|candidate| {
                let evaluation = evaluate_resume(resume, candidate.job);
                (evaluation.score >= options.minimum_resume_score).then(|| PlanCandidate {
                    score: combined_score(evaluation.score, candidate.policy_score),
                    resume_score: Some(evaluation.score),
                    title_match: evaluation.title_match,
                    matched_skills: evaluation.matched_skills,
                    ..candidate
                })
            })
            .collect();
        gated.candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.job.id.cmp(&right.job.id))
        });
        let eligible = gated.candidates.len();
        self.persist_candidates(
            gated,
            PlanPersistence {
                policy,
                template: options.template,
                resume: Some(resume),
                limit: options.limit,
                now: options.now,
                mode: "resume_screening_manual_review",
                eligible,
                skipped_resume_score: policy_eligible.saturating_sub(eligible),
                always_preview: true,
            },
        )
    }

    pub fn list_plans(&self) -> Result<Vec<ApplicationPlan>, BossError> {
        self.read_plans()
    }

    /// Records a local human workflow transition without contacting a platform.
    pub fn transition_plan(
        &self,
        job_id: &str,
        state: ApplicationPlanState,
        note: Option<String>,
        now: u64,
    ) -> Result<ApplicationPlan, BossError> {
        let job_id = normalize_plan_job_id(job_id)?;
        let note = normalize_state_note(note)?;
        let mut plans = self.read_plans()?;
        let plan = plans
            .iter_mut()
            .find(|plan| plan.job_id == job_id)
            .ok_or_else(|| BossError::Campaign(format!("application plan not found: {job_id}")))?;
        if plan.state.is_terminal() {
            return Err(BossError::Campaign(format!(
                "application plan {job_id} is terminal in state {}",
                plan.state.as_str()
            )));
        }
        if !plan.state.permits(state) {
            return Err(BossError::Campaign(format!(
                "invalid application plan transition: {} -> {}",
                plan.state.as_str(),
                state.as_str()
            )));
        }
        plan.state = state;
        plan.state_changed_at = now;
        plan.state_note = note;
        let transitioned = plan.clone();
        self.save_plans(&plans)?;
        Ok(transitioned)
    }

    pub fn stats(&self) -> Result<CampaignStats, BossError> {
        let blacklist = self.read_blacklist()?;
        let plans = self.read_plans()?;
        Ok(CampaignStats {
            policies: self.read_policies()?.len(),
            blacklist: CampaignBlacklistStats {
                total: blacklist.len(),
                company: blacklist
                    .iter()
                    .filter(|rule| rule.kind == BlacklistKind::Company)
                    .count(),
                description: blacklist
                    .iter()
                    .filter(|rule| rule.kind == BlacklistKind::Description)
                    .count(),
                job: blacklist
                    .iter()
                    .filter(|rule| rule.kind == BlacklistKind::Job)
                    .count(),
            },
            templates: self.read_templates()?.len(),
            plans: CampaignPlanStats {
                total: plans.len(),
                manual_review: plans
                    .iter()
                    .filter(|plan| plan.state == ApplicationPlanState::ManualReview)
                    .count(),
                approved: plans
                    .iter()
                    .filter(|plan| plan.state == ApplicationPlanState::Approved)
                    .count(),
                rejected: plans
                    .iter()
                    .filter(|plan| plan.state == ApplicationPlanState::Rejected)
                    .count(),
                recorded_submitted: plans
                    .iter()
                    .filter(|plan| plan.state == ApplicationPlanState::RecordedSubmitted)
                    .count(),
                dry_run: plans.iter().filter(|plan| plan.dry_run).count(),
            },
        })
    }

    fn read_policies(&self) -> Result<Vec<CampaignPolicy>, BossError> {
        let values: Vec<CampaignPolicy> = read_json(&self.policies_path, "campaign policies")?;
        for value in &values {
            if normalize_policy(value.clone())? != *value {
                return Err(BossError::Campaign(
                    "stored campaign policy contains unnormalized content".to_owned(),
                ));
            }
        }
        ensure_unique_names(values.iter().map(|value| value.name.as_str()), "policy")?;
        Ok(values)
    }

    fn save_policies(&self, values: &[CampaignPolicy]) -> Result<(), BossError> {
        save_json(&self.policies_path, values, "campaign policies")
    }

    fn read_blacklist(&self) -> Result<Vec<BlacklistRule>, BossError> {
        let values: Vec<BlacklistRule> = read_json(&self.blacklist_path, "campaign blacklist")?;
        for (index, value) in values.iter().enumerate() {
            let normalized = normalize_rule_value(&value.value, "blacklist value")?;
            if normalized != value.value
                || values[..index].iter().any(|previous| {
                    previous.kind == value.kind && previous.value.eq_ignore_ascii_case(&value.value)
                })
            {
                return Err(BossError::Campaign(
                    "stored blacklist contains invalid or duplicate rules".to_owned(),
                ));
            }
        }
        Ok(values)
    }

    fn save_blacklist(&self, values: &[BlacklistRule]) -> Result<(), BossError> {
        save_json(&self.blacklist_path, values, "campaign blacklist")
    }

    fn read_templates(&self) -> Result<Vec<GreetingTemplate>, BossError> {
        let values: Vec<GreetingTemplate> = read_json(&self.templates_path, "greeting templates")?;
        for value in &values {
            if normalize_name(&value.name, "template name")? != value.name
                || normalize_template(&value.body)? != value.body
            {
                return Err(BossError::Campaign(
                    "stored greeting template contains unnormalized content".to_owned(),
                ));
            }
        }
        ensure_unique_names(values.iter().map(|value| value.name.as_str()), "template")?;
        Ok(values)
    }

    fn save_templates(&self, values: &[GreetingTemplate]) -> Result<(), BossError> {
        save_json(&self.templates_path, values, "greeting templates")
    }

    fn read_plans(&self) -> Result<Vec<ApplicationPlan>, BossError> {
        let values: Vec<ApplicationPlan> = read_json(&self.plans_path, "application plans")?;
        let mut ids = HashSet::new();
        for plan in &values {
            if plan.job_id.trim().is_empty()
                || plan.job_id != plan.job_id.trim()
                || !plan.dry_run
                || plan.resume_name.is_some() != plan.resume_updated_at.is_some()
                || plan
                    .resume_name
                    .as_deref()
                    .is_some_and(|name| normalize_plan_job_id(name).is_err())
                || plan
                    .state_note
                    .as_ref()
                    .is_some_and(|note| normalize_state_note(Some(note.clone())).is_err())
                || !ids.insert(plan.job_id.as_str())
            {
                return Err(BossError::Campaign(
                    "stored application plans must be unique normalized local dry runs".to_owned(),
                ));
            }
        }
        Ok(values)
    }

    fn save_plans(&self, values: &[ApplicationPlan]) -> Result<(), BossError> {
        save_json(&self.plans_path, values, "application plans")
    }

    fn gate_candidates<'a>(
        &self,
        jobs: &'a [Job],
        policy: &CampaignPolicy,
    ) -> Result<GatedCandidates<'a>, BossError> {
        let blacklist = self.read_blacklist()?;
        let mut skipped_blacklist = 0;
        let candidates = jobs
            .iter()
            .filter_map(|job| {
                if blacklist.iter().any(|rule| blacklist_matches(rule, job)) {
                    skipped_blacklist += 1;
                    return None;
                }
                let evaluation = policy.evaluate(job);
                evaluation.eligible.then_some(PlanCandidate {
                    job,
                    score: evaluation.score,
                    policy_score: evaluation.score,
                    resume_score: None,
                    title_match: false,
                    matched_skills: Vec::new(),
                })
            })
            .collect();
        Ok(GatedCandidates {
            considered: jobs.len(),
            skipped_blacklist,
            candidates,
        })
    }

    fn persist_candidates(
        &self,
        gated: GatedCandidates<'_>,
        request: PlanPersistence<'_>,
    ) -> Result<PlanBuildResult, BossError> {
        let mut stored = self.read_plans()?;
        let mut planned_or_existing: HashSet<String> =
            stored.iter().map(|plan| plan.job_id.clone()).collect();
        let mut plans = Vec::new();
        let mut greeting_previews = Vec::new();
        let mut skipped_existing = 0;
        for candidate in gated.candidates {
            if planned_or_existing.contains(&candidate.job.id) {
                skipped_existing += 1;
                continue;
            }
            if plans.len() == request.limit {
                continue;
            }
            planned_or_existing.insert(candidate.job.id.clone());
            let preview = match request.template {
                Some(selected) => {
                    let resume_context = if request.always_preview {
                        ResumeRenderContext::Screening {
                            title: &request
                                .resume
                                .ok_or_else(|| {
                                    BossError::InvalidArgument(
                                        "resume screening preview requires a bound resume"
                                            .to_owned(),
                                    )
                                })?
                                .title,
                            matched_skills: &candidate.matched_skills,
                        }
                    } else {
                        request
                            .resume
                            .map_or(ResumeRenderContext::Unbound, ResumeRenderContext::Full)
                    };
                    Some(render_body(&selected.body, candidate.job, resume_context)?)
                }
                None if request.always_preview => Some(render_screen_greeting_preview(
                    &candidate,
                    request.resume.ok_or_else(|| {
                        BossError::InvalidArgument(
                            "resume screening preview requires a bound resume".to_owned(),
                        )
                    })?,
                )?),
                None => None,
            };
            if let Some(text) = preview {
                greeting_previews.push(PlanGreetingPreview {
                    job_id: candidate.job.id.clone(),
                    sent: false,
                    text,
                });
            }
            plans.push(ApplicationPlan {
                job_id: candidate.job.id.clone(),
                job_title: candidate.job.title.clone(),
                company: candidate.job.company.clone(),
                policy_name: request.policy.name.clone(),
                template_name: request.template.map(|selected| selected.name.clone()),
                resume_name: request.resume.map(|document| document.name.clone()),
                resume_updated_at: request.resume.map(|document| document.updated_at),
                policy_score: Some(candidate.policy_score),
                resume_score: candidate.resume_score,
                title_match: candidate.title_match,
                matched_skills: candidate.matched_skills,
                score: candidate.score,
                state: ApplicationPlanState::ManualReview,
                state_changed_at: request.now,
                state_note: None,
                dry_run: true,
                created_at: request.now,
            });
        }
        if !plans.is_empty() {
            stored.extend(plans.iter().cloned());
            self.save_plans(&stored)?;
        }
        Ok(PlanBuildResult {
            mode: request.mode.to_owned(),
            dry_run: true,
            considered: gated.considered,
            eligible: request.eligible,
            planned: plans.len(),
            skipped_existing,
            skipped_blacklist: gated.skipped_blacklist,
            skipped_resume_score: request.skipped_resume_score,
            plans,
            greeting_previews,
        })
    }
}

impl CampaignPolicy {
    /// Evaluates only normalized cached fields with no remote operation.
    #[must_use]
    pub fn evaluate(&self, job: &Job) -> PolicyEvaluation {
        if let Some(rule) = self
            .exclude
            .iter()
            .find(|rule| contains_normalized(&rule.field.text(job), &rule.value))
        {
            return PolicyEvaluation {
                eligible: false,
                score: 0,
                excluded_by: Some(format!("exclude:{}", rule.field.as_str())),
            };
        }
        if !self.required_welfare.iter().all(|needed| {
            job.welfare
                .iter()
                .any(|actual| contains_normalized(actual, needed))
        }) {
            return PolicyEvaluation {
                eligible: false,
                score: 0,
                excluded_by: Some("required_welfare".to_owned()),
            };
        }
        if !salary_in_range(job, self.monthly_salary_min, self.monthly_salary_max) {
            return PolicyEvaluation {
                eligible: false,
                score: 0,
                excluded_by: Some("monthly_salary".to_owned()),
            };
        }
        let matched = self
            .include
            .iter()
            .filter(|rule| contains_normalized(&rule.field.text(job), &rule.value))
            .count();
        let score = if self.include.is_empty() {
            100
        } else {
            ((matched * 100) / self.include.len()) as u8
        };
        let eligible = self.minimum_score.is_none_or(|minimum| score >= minimum);
        PolicyEvaluation {
            eligible,
            score,
            excluded_by: (!eligible).then_some("minimum_score".to_owned()),
        }
    }
}

fn normalize_policy(mut policy: CampaignPolicy) -> Result<CampaignPolicy, BossError> {
    policy.name = normalize_name(&policy.name, "policy name")?;
    policy.include = normalize_rules(policy.include, "include")?;
    policy.exclude = normalize_rules(policy.exclude, "exclude")?;
    policy.required_welfare = normalize_values(
        policy.required_welfare,
        "required welfare",
        MAX_WELFARE_REQUIREMENTS,
    )?;
    if policy.minimum_score.is_some_and(|value| value > 100) {
        return Err(BossError::InvalidArgument(
            "minimum score must be in 0..=100".to_owned(),
        ));
    }
    if policy.monthly_salary_min.is_some_and(|value| value == 0)
        || policy.monthly_salary_max.is_some_and(|value| value == 0)
        || policy
            .monthly_salary_min
            .zip(policy.monthly_salary_max)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(BossError::InvalidArgument(
            "monthly salary bounds must be positive and min must not exceed max".to_owned(),
        ));
    }
    Ok(policy)
}

fn normalize_rules(rules: Vec<CampaignRule>, field: &str) -> Result<Vec<CampaignRule>, BossError> {
    if rules.len() > MAX_RULES {
        return Err(BossError::InvalidArgument(format!(
            "{field} supports at most {MAX_RULES} rules"
        )));
    }
    let mut normalized = Vec::with_capacity(rules.len());
    for mut rule in rules {
        rule.value = normalize_rule_value(&rule.value, field)?;
        if normalized.iter().any(|previous: &CampaignRule| {
            previous.field == rule.field && previous.value.eq_ignore_ascii_case(&rule.value)
        }) {
            return Err(BossError::InvalidArgument(format!(
                "{field} contains duplicate rules"
            )));
        }
        normalized.push(rule);
    }
    Ok(normalized)
}

fn normalize_values(
    values: Vec<String>,
    field: &str,
    maximum: usize,
) -> Result<Vec<String>, BossError> {
    if values.len() > maximum {
        return Err(BossError::InvalidArgument(format!(
            "{field} supports at most {maximum} values"
        )));
    }
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let value = normalize_rule_value(&value, field)?;
        if normalized
            .iter()
            .any(|previous: &String| previous.eq_ignore_ascii_case(&value))
        {
            return Err(BossError::InvalidArgument(format!(
                "{field} contains duplicate values"
            )));
        }
        normalized.push(value);
    }
    Ok(normalized)
}

fn normalize_name(value: &str, field: &str) -> Result<String, BossError> {
    normalize_bounded(value, field, MAX_CAMPAIGN_NAME_CHARS)
}

fn normalize_rule_value(value: &str, field: &str) -> Result<String, BossError> {
    normalize_bounded(value, field, MAX_RULE_VALUE_CHARS)
}

fn normalize_template(value: &str) -> Result<String, BossError> {
    let value = normalize_bounded(value, "template body", MAX_TEMPLATE_CHARS)?;
    validate_template(&value)?;
    Ok(value)
}

fn normalize_plan_job_id(value: &str) -> Result<String, BossError> {
    normalize_bounded(value, "application plan job id", MAX_RULE_VALUE_CHARS)
}

fn normalize_state_note(note: Option<String>) -> Result<Option<String>, BossError> {
    note.map(|value| normalize_bounded(&value, "application plan note", MAX_STATE_NOTE_CHARS))
        .transpose()
}

fn normalize_bounded(value: &str, field: &str, maximum: usize) -> Result<String, BossError> {
    let value = value.trim();
    let count = value.chars().count();
    if count == 0 || count > maximum {
        return Err(BossError::InvalidArgument(format!(
            "{field} must contain 1..={maximum} characters"
        )));
    }
    Ok(value.to_owned())
}

fn validate_plan_limit(limit: usize) -> Result<(), BossError> {
    if limit == 0 || limit > MAX_PLANS_PER_BUILD {
        return Err(BossError::InvalidArgument(format!(
            "plan limit must be 1..={MAX_PLANS_PER_BUILD}"
        )));
    }
    Ok(())
}

fn validate_screen_resume(resume: &ResumeDocument) -> Result<(), BossError> {
    if resume.title.trim().is_empty() && resume.skills.iter().all(|skill| skill.trim().is_empty()) {
        return Err(BossError::InvalidArgument(
            "resume screening requires a non-empty title or at least one skill".to_owned(),
        ));
    }
    Ok(())
}

fn validate_screening_template(body: &str) -> Result<(), BossError> {
    validate_template(body)?;
    if body.contains("{{resume_summary}}") {
        return Err(BossError::InvalidArgument(
            "campaign screening templates cannot use {{resume_summary}}".to_owned(),
        ));
    }
    Ok(())
}

fn contains_normalized(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn evaluate_resume(resume: &ResumeDocument, job: &Job) -> ResumeEvaluation {
    let searchable = normalize_match_text(&format!(
        "{} {} {}",
        job.title,
        job.skills.join(" "),
        job.description
    ));
    let normalized_title = normalize_match_text(&resume.title);
    let title_match =
        !normalized_title.is_empty() && explicit_text_match(&searchable, &normalized_title);
    let matched_skills: Vec<String> = resume
        .skills
        .iter()
        .filter(|skill| {
            let normalized = normalize_match_text(skill);
            !normalized.is_empty() && explicit_text_match(&searchable, &normalized)
        })
        .cloned()
        .collect();
    let skill_score = if resume.skills.is_empty() {
        0
    } else {
        ((matched_skills.len() * 50) / resume.skills.len()) as u8
    };
    ResumeEvaluation {
        score: u8::from(title_match) * 50 + skill_score,
        title_match,
        matched_skills,
    }
}

fn normalize_match_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut separator = true;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() || is_significant_match_punctuation(character) {
            normalized.push(character);
            separator = false;
        } else if !separator {
            normalized.push(' ');
            separator = true;
        }
    }
    if separator {
        normalized.pop();
    }
    normalized
}

const fn is_significant_match_punctuation(character: char) -> bool {
    // These characters carry identity inside common technical tokens such as
    // C++, C#, .NET, snake_case, kebab-case, and path-like names.
    matches!(character, '+' | '#' | '.' | '_' | '-' | '/')
}

fn explicit_text_match(searchable: &str, needle: &str) -> bool {
    if needle.is_ascii() {
        format!(" {searchable} ").contains(&format!(" {needle} "))
    } else {
        searchable.contains(needle)
    }
}

fn combined_score(resume_score: u8, policy_score: u8) -> u8 {
    ((u16::from(resume_score) * 70 + u16::from(policy_score) * 30) / 100) as u8
}

fn render_screen_greeting_preview(
    candidate: &PlanCandidate<'_>,
    resume: &ResumeDocument,
) -> Result<String, BossError> {
    let mut preview = if candidate.job.company.trim().is_empty() {
        format!("您好，我关注到 {} 职位。", candidate.job.title)
    } else {
        format!(
            "您好，我关注到 {} 的 {} 职位。",
            candidate.job.company, candidate.job.title
        )
    };
    if candidate.title_match {
        preview.push_str(&format!("我的求职方向是 {}。", resume.title.trim()));
    }
    if !candidate.matched_skills.is_empty() {
        preview.push_str(&format!(
            "与岗位匹配的技能包括 {}。",
            candidate.matched_skills.join("、")
        ));
    }
    validate_greeting_preview_length(preview)
}

fn salary_in_range(job: &Job, minimum: Option<u32>, maximum: Option<u32>) -> bool {
    if minimum.is_none() && maximum.is_none() {
        return true;
    }
    let Some((low, high)) = monthly_salary_range(&job.salary) else {
        return false;
    };
    minimum.is_none_or(|value| high >= value) && maximum.is_none_or(|value| low <= value)
}

fn monthly_salary_range(text: &str) -> Option<(u32, u32)> {
    if text.contains("年薪") || text.contains("/年") {
        return None;
    }
    let all_units: Vec<(usize, char)> = text
        .char_indices()
        .filter(|(_, character)| *character == '万' || character.eq_ignore_ascii_case(&'k'))
        .collect();
    let final_unit = all_units.last()?.0;
    let annual_marker = text
        .char_indices()
        .find(|(index, character)| *character == '薪' && *index > final_unit)
        .map_or(text.len(), |(index, _)| index);
    let before_annual_metadata = &text[..annual_marker];
    let units: Vec<(usize, char)> = all_units
        .into_iter()
        .filter(|(index, _)| *index < annual_marker)
        .collect();
    let (_, unit) = *units.last()?;
    if units.iter().any(|(_, candidate)| *candidate != unit) {
        return None;
    }
    let end = units.last()?.0 + unit.len_utf8();
    let values = decimal_values(&before_annual_metadata[..end]);
    let last = *values.last()?;
    let first = values
        .get(values.len().saturating_sub(2))
        .copied()
        .unwrap_or(last);
    let multiplier = if unit == '万' { 10_000.0 } else { 1_000.0 };
    let low = (first.min(last) * multiplier).round();
    let high = (first.max(last) * multiplier).round();
    if !(1.0..=(u32::MAX as f64)).contains(&low) || !(1.0..=(u32::MAX as f64)).contains(&high) {
        return None;
    }
    Some((low as u32, high as u32))
}

fn decimal_values(text: &str) -> Vec<f64> {
    let mut values = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() || character == '.' {
            current.push(character);
        } else if !current.is_empty() {
            if let Ok(value) = current.parse::<f64>() {
                values.push(value);
            }
            current.clear();
        }
    }
    if !current.is_empty()
        && let Ok(value) = current.parse::<f64>()
    {
        values.push(value);
    }
    values
}

fn blacklist_matches(rule: &BlacklistRule, job: &Job) -> bool {
    match rule.kind {
        BlacklistKind::Company => contains_normalized(&job.company, &rule.value),
        BlacklistKind::Description => contains_normalized(&job.description, &rule.value),
        BlacklistKind::Job => {
            contains_normalized(&job.title, &rule.value)
                || contains_normalized(&job.description, &rule.value)
        }
    }
}

fn validate_template(body: &str) -> Result<(), BossError> {
    let mut remainder = body;
    while let Some(start) = remainder.find("{{") {
        let before = &remainder[..start];
        if before.contains("}}") {
            return Err(BossError::InvalidArgument(
                "template contains an unmatched closing placeholder".to_owned(),
            ));
        }
        let after_start = &remainder[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err(BossError::InvalidArgument(
                "template contains an unclosed placeholder".to_owned(),
            ));
        };
        let placeholder = &after_start[..end];
        if !allowed_placeholder(placeholder) {
            return Err(BossError::InvalidArgument(format!(
                "unsupported greeting placeholder: {placeholder}"
            )));
        }
        remainder = &after_start[end + 2..];
    }
    if remainder.contains("}}") {
        return Err(BossError::InvalidArgument(
            "template contains an unmatched closing placeholder".to_owned(),
        ));
    }
    Ok(())
}

fn allowed_placeholder(value: &str) -> bool {
    matches!(
        value,
        "title"
            | "company"
            | "city"
            | "salary"
            | "welfare"
            | "resume_title"
            | "resume_summary"
            | "resume_skills"
    )
}

fn render_body(
    body: &str,
    job: &Job,
    resume: ResumeRenderContext<'_>,
) -> Result<String, BossError> {
    validate_template(body)?;
    let mut rendered = String::with_capacity(body.len());
    let welfare = job.welfare.join("、");
    let resume_skills = match resume {
        ResumeRenderContext::Full(document) => Some(document.skills.join("、")),
        ResumeRenderContext::Screening { matched_skills, .. } => Some(matched_skills.join("、")),
        ResumeRenderContext::Unbound => None,
    };
    let mut remainder = body;
    while let Some(start) = remainder.find("{{") {
        rendered.push_str(&remainder[..start]);
        let after_start = &remainder[start + 2..];
        let end = after_start.find("}}").ok_or_else(|| {
            BossError::InvalidArgument("template contains an unclosed placeholder".to_owned())
        })?;
        rendered.push_str(match &after_start[..end] {
            "title" => &job.title,
            "company" => &job.company,
            "city" => &job.city,
            "salary" => &job.salary,
            "welfare" => &welfare,
            "resume_title" => resume_title(resume)?,
            "resume_summary" => resume_summary(resume)?,
            "resume_skills" => required_resume_value(resume_skills.as_deref())?,
            _ => {
                return Err(BossError::InvalidArgument(
                    "template contains an unsupported placeholder".to_owned(),
                ));
            }
        });
        remainder = &after_start[end + 2..];
    }
    rendered.push_str(remainder);
    validate_greeting_preview_length(rendered)
}

fn validate_greeting_preview_length(rendered: String) -> Result<String, BossError> {
    if rendered.chars().count() > MAX_GREETING_PREVIEW_CHARS {
        return Err(BossError::InvalidArgument(format!(
            "rendered greeting preview must contain at most {MAX_GREETING_PREVIEW_CHARS} characters"
        )));
    }
    Ok(rendered)
}

fn resume_title(resume: ResumeRenderContext<'_>) -> Result<&str, BossError> {
    match resume {
        ResumeRenderContext::Full(document) => Ok(&document.title),
        ResumeRenderContext::Screening { title, .. } => Ok(title),
        ResumeRenderContext::Unbound => Err(unbound_resume_placeholder()),
    }
}

fn resume_summary(resume: ResumeRenderContext<'_>) -> Result<&str, BossError> {
    match resume {
        ResumeRenderContext::Full(document) => Ok(&document.summary),
        ResumeRenderContext::Screening { .. } => Err(BossError::InvalidArgument(
            "campaign screening templates cannot use {{resume_summary}}".to_owned(),
        )),
        ResumeRenderContext::Unbound => Err(unbound_resume_placeholder()),
    }
}

fn required_resume_value(value: Option<&str>) -> Result<&str, BossError> {
    value.ok_or_else(unbound_resume_placeholder)
}

fn unbound_resume_placeholder() -> BossError {
    BossError::InvalidArgument(
        "template uses a resume placeholder but no resume is bound to this plan".to_owned(),
    )
}

fn ensure_unique_names<'a>(
    names: impl Iterator<Item = &'a str>,
    category: &str,
) -> Result<(), BossError> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name.to_lowercase()) {
            return Err(BossError::Campaign(format!(
                "stored {category}s contain duplicate names"
            )));
        }
    }
    Ok(())
}

fn read_json<T: for<'a> Deserialize<'a>>(path: &PathBuf, label: &str) -> Result<Vec<T>, BossError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| BossError::Campaign(format!("{label}: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(BossError::Campaign(format!("{label}: {error}"))),
    }
}

fn save_json<T: Serialize>(path: &Path, values: &[T], label: &str) -> Result<(), BossError> {
    let bytes = serde_json::to_vec_pretty(values)
        .map_err(|error| BossError::Campaign(format!("{label}: {error}")))?;
    atomic_write(path, &bytes, |error| {
        BossError::Campaign(format!("{label}: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::Platform;

    fn resume() -> ResumeDocument {
        ResumeDocument {
            name: "candidate".to_owned(),
            title: "Private Resume Headline".to_owned(),
            summary: "Private resume summary".to_owned(),
            basics: Default::default(),
            skills: vec!["PrivateSkill".to_owned(), "PrivateTool".to_owned()],
            experience: Vec::new(),
            education: Vec::new(),
            projects: Vec::new(),
            created_at: 10,
            updated_at: 20,
        }
    }

    fn job() -> Job {
        let mut job = Job::new(
            "job-1",
            Platform::Zhipin,
            "remote-1",
            "Rust Engineer",
            "https://example.test/job",
        );
        job.company = "Example Labs".to_owned();
        job.city = "深圳".to_owned();
        job.salary = "20-30K".to_owned();
        job.welfare = vec!["五险一金".to_owned(), "远程".to_owned()];
        job.description = "Contact: Alice".to_owned();
        job
    }

    fn store() -> (tempfile::TempDir, CampaignStore) {
        let directory = tempdir().expect("temporary directory");
        let store = CampaignStore::from_paths(&DataPaths::new(directory.path()));
        (directory, store)
    }

    #[test]
    fn policy_filters_exclusions_welfare_salary_and_score() {
        let policy = CampaignPolicy {
            name: "rust".to_owned(),
            include: vec![
                CampaignRule {
                    field: CampaignField::Title,
                    value: "Rust".to_owned(),
                },
                CampaignRule {
                    field: CampaignField::City,
                    value: "深圳".to_owned(),
                },
            ],
            exclude: vec![CampaignRule {
                field: CampaignField::Company,
                value: "Agency".to_owned(),
            }],
            required_welfare: vec!["远程".to_owned()],
            monthly_salary_min: Some(25_000),
            monthly_salary_max: Some(35_000),
            minimum_score: Some(100),
        };
        assert_eq!(
            policy.evaluate(&job()),
            PolicyEvaluation {
                eligible: true,
                score: 100,
                excluded_by: None
            }
        );

        let mut excluded = job();
        excluded.company = "Agency Example".to_owned();
        assert_eq!(
            policy.evaluate(&excluded).excluded_by.as_deref(),
            Some("exclude:company")
        );

        let mut insufficient = job();
        insufficient.welfare.clear();
        assert_eq!(
            policy.evaluate(&insufficient).excluded_by.as_deref(),
            Some("required_welfare")
        );
    }

    #[test]
    fn salary_range_uses_the_monthly_range_and_rejects_unsupported_or_ambiguous_values() {
        assert_eq!(monthly_salary_range("20-30K·13薪"), Some((20_000, 30_000)));
        assert_eq!(monthly_salary_range("20K-3万"), None);
        assert_eq!(monthly_salary_range("面议"), None);
        assert_eq!(monthly_salary_range("20K/年"), None);

        let policy = CampaignPolicy {
            name: "monthly-minimum".to_owned(),
            include: Vec::new(),
            exclude: Vec::new(),
            required_welfare: Vec::new(),
            monthly_salary_min: Some(25_000),
            monthly_salary_max: None,
            minimum_score: None,
        };
        let mut qualified = job();
        qualified.salary = "20-30K·13薪".to_owned();
        assert!(policy.evaluate(&qualified).eligible);

        let mut ambiguous = qualified;
        ambiguous.salary = "20K-3万".to_owned();
        assert_eq!(
            policy.evaluate(&ambiguous).excluded_by.as_deref(),
            Some("monthly_salary")
        );
    }

    #[test]
    fn blacklist_is_deduplicated_and_applied_without_network_state() {
        let (_directory, store) = store();
        let first = store
            .add_blacklist(BlacklistKind::Company, " Example ", 10)
            .expect("add");
        let duplicate = store
            .add_blacklist(BlacklistKind::Company, "example", 20)
            .expect("duplicate");
        assert_eq!(first, duplicate);
        let policy = store
            .add_policy(CampaignPolicy {
                name: "all".to_owned(),
                include: Vec::new(),
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: None,
            })
            .expect("policy");
        let result = store
            .build_plans(&[job()], &policy, None, None, 10, 30)
            .expect("plan");
        assert_eq!((result.planned, result.skipped_blacklist), (0, 1));
        assert!(store.list_plans().expect("plans").is_empty());
    }

    #[test]
    fn description_blacklist_is_explicitly_scoped_to_cached_description_text() {
        let (_directory, store) = store();
        store
            .add_blacklist(BlacklistKind::Description, "Contact: Alice", 10)
            .expect("description blacklist");
        let policy = store
            .add_policy(CampaignPolicy {
                name: "all".to_owned(),
                include: Vec::new(),
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: None,
            })
            .expect("policy");
        let result = store
            .build_plans(&[job()], &policy, None, None, 10, 30)
            .expect("plan");
        assert_eq!((result.planned, result.skipped_blacklist), (0, 1));
        assert_eq!(store.stats().expect("stats").blacklist.description, 1);
    }

    #[test]
    fn templates_allow_only_known_placeholders_and_render_without_recursion() {
        let (_directory, store) = store();
        store
            .add_template("hello", "Hi {{company}}, {{title}} — {{welfare}}", 1)
            .expect("template");
        assert_eq!(
            store.render_template("hello", &job()).expect("render"),
            "Hi Example Labs, Rust Engineer — 五险一金、远程"
        );
        assert!(store.add_template("bad", "{{url}}", 2).is_err());
        assert!(store.add_template("bad", "{{title}", 2).is_err());
    }

    #[test]
    fn resume_placeholders_require_a_bound_resume_and_record_only_its_metadata() {
        let (directory, store) = store();
        let policy = store
            .add_policy(CampaignPolicy {
                name: "all".to_owned(),
                include: Vec::new(),
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: None,
            })
            .expect("policy");
        let template = store
            .add_template(
                "resume",
                "{{resume_title}}: {{resume_summary}} ({{resume_skills}})",
                1,
            )
            .expect("template");
        let missing = store.build_plans(&[job()], &policy, Some(&template), None, 10, 30);
        assert!(
            missing
                .expect_err("missing resume")
                .to_string()
                .contains("no resume is bound")
        );

        let document = resume();
        let planned = store
            .build_plans(&[job()], &policy, Some(&template), Some(&document), 10, 31)
            .expect("plan");
        let plan = &planned.plans[0];
        let persisted = std::fs::read_to_string(directory.path().join("application_plans.json"))
            .expect("stored plans");
        assert_eq!(
            (plan.resume_name.as_deref(), plan.resume_updated_at),
            (Some("candidate"), Some(20))
        );
        assert_eq!(
            planned.greeting_previews,
            vec![PlanGreetingPreview {
                job_id: "job-1".to_owned(),
                sent: false,
                text: "Private Resume Headline: Private resume summary (PrivateSkill、PrivateTool)"
                    .to_owned(),
            }]
        );
        assert!(
            !persisted.contains("Private Resume Headline")
                && !persisted.contains("Private resume summary")
                && !persisted.contains("PrivateSkill、PrivateTool")
        );
    }

    #[test]
    fn plan_transitions_follow_the_strict_local_state_machine() {
        let (_directory, store) = store();
        let policy = store
            .add_policy(CampaignPolicy {
                name: "all".to_owned(),
                include: Vec::new(),
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: None,
            })
            .expect("policy");
        store
            .build_plans(&[job()], &policy, None, None, 10, 10)
            .expect("plan");
        let invalid =
            store.transition_plan("job-1", ApplicationPlanState::RecordedSubmitted, None, 11);
        assert!(
            invalid
                .expect_err("invalid transition")
                .to_string()
                .contains("invalid")
        );

        let approved = store
            .transition_plan(
                "job-1",
                ApplicationPlanState::Approved,
                Some("reviewed locally".to_owned()),
                12,
            )
            .expect("approved");
        assert_eq!(
            (
                approved.state,
                approved.state_changed_at,
                approved.state_note.as_deref()
            ),
            (ApplicationPlanState::Approved, 12, Some("reviewed locally"))
        );
        let recorded = store
            .transition_plan("job-1", ApplicationPlanState::RecordedSubmitted, None, 13)
            .expect("recorded");
        assert_eq!(recorded.state, ApplicationPlanState::RecordedSubmitted);
        let stats = store.stats().expect("stats");
        assert_eq!(
            (
                stats.plans.manual_review,
                stats.plans.approved,
                stats.plans.rejected,
                stats.plans.recorded_submitted,
            ),
            (0, 0, 0, 1)
        );
        assert!(
            store
                .transition_plan("job-1", ApplicationPlanState::Rejected, None, 14)
                .expect_err("terminal")
                .to_string()
                .contains("terminal")
        );
    }

    #[test]
    fn plans_are_deduplicated_and_remain_manual_review_dry_runs() {
        let (_directory, store) = store();
        let policy = store
            .add_policy(CampaignPolicy {
                name: "all".to_owned(),
                include: Vec::new(),
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: None,
            })
            .expect("policy");
        let first = store
            .build_plans(&[job()], &policy, None, None, 10, 1)
            .expect("first");
        let second = store
            .build_plans(&[job()], &policy, None, None, 10, 2)
            .expect("second");
        assert_eq!(
            (first.planned, second.planned, second.skipped_existing),
            (1, 0, 1)
        );
        let plans = store.list_plans().expect("plans");
        assert_eq!(
            (plans.len(), plans[0].state.as_str(), plans[0].dry_run),
            (1, "manual_review", true)
        );
    }

    #[test]
    fn plan_build_counts_every_inspected_job_even_after_the_plan_limit() {
        let (_directory, store) = store();
        let policy = store
            .add_policy(CampaignPolicy {
                name: "all".to_owned(),
                include: Vec::new(),
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: None,
            })
            .expect("policy");
        let first = job();
        let mut second = job();
        second.id = "job-2".to_owned();
        let mut third = job();
        third.id = "job-3".to_owned();
        let result = store
            .build_plans(&[first, second, third], &policy, None, None, 1, 10)
            .expect("plan");
        assert_eq!(
            (
                result.considered,
                result.eligible,
                result.planned,
                result.skipped_existing,
                result.skipped_blacklist,
            ),
            (3, 3, 1, 0, 0)
        );
        assert_eq!(store.list_plans().expect("plans").len(), 1);
    }

    #[test]
    fn resume_screening_ranks_explicit_matches_and_persists_review_metadata_only() {
        let (directory, store) = store();
        let policy = store
            .add_policy(CampaignPolicy {
                name: "rust".to_owned(),
                include: vec![CampaignRule {
                    field: CampaignField::Title,
                    value: "Rust".to_owned(),
                }],
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: Some(100),
            })
            .expect("policy");
        store
            .add_blacklist(BlacklistKind::Company, "Blocked", 1)
            .expect("blacklist");
        let document = ResumeDocument {
            title: "Rust Engineer".to_owned(),
            skills: vec!["Rust".to_owned(), "Tokio".to_owned()],
            ..resume()
        };
        let mut tied_b = job();
        tied_b.id = "job-b".to_owned();
        tied_b.skills = vec!["Rust".to_owned()];
        tied_b.description = "Tokio services".to_owned();
        let mut tied_a = tied_b.clone();
        tied_a.id = "job-a".to_owned();
        let mut below_floor = tied_b.clone();
        below_floor.id = "job-low".to_owned();
        below_floor.title = "Rust Product".to_owned();
        below_floor.description.clear();
        let mut blacklisted = tied_b.clone();
        blacklisted.id = "job-blocked".to_owned();
        blacklisted.company = "Blocked Company".to_owned();

        let result = store
            .screen_plans(
                &[tied_b, below_floor, blacklisted, tied_a],
                &policy,
                &document,
                ScreenPlanOptions {
                    template: None,
                    limit: 10,
                    minimum_resume_score: DEFAULT_MINIMUM_RESUME_SCORE,
                    now: 10,
                },
            )
            .expect("screen");
        assert_eq!(
            (
                result.considered,
                result.eligible,
                result.skipped_blacklist,
                result.skipped_resume_score,
            ),
            (4, 2, 1, 1)
        );
        assert_eq!(
            result
                .plans
                .iter()
                .map(|plan| plan.job_id.as_str())
                .collect::<Vec<_>>(),
            vec!["job-a", "job-b"]
        );
        assert_eq!(
            result.greeting_previews,
            vec![
                PlanGreetingPreview {
                    job_id: "job-a".to_owned(),
                    sent: false,
                    text: "您好，我关注到 Example Labs 的 Rust Engineer 职位。我的求职方向是 Rust Engineer。与岗位匹配的技能包括 Rust、Tokio。"
                        .to_owned(),
                },
                PlanGreetingPreview {
                    job_id: "job-b".to_owned(),
                    sent: false,
                    text: "您好，我关注到 Example Labs 的 Rust Engineer 职位。我的求职方向是 Rust Engineer。与岗位匹配的技能包括 Rust、Tokio。"
                        .to_owned(),
                },
            ]
        );
        let plan = &result.plans[0];
        assert_eq!(
            (
                plan.score,
                plan.policy_score,
                plan.resume_score,
                plan.title_match,
                plan.matched_skills.clone(),
                plan.state,
                plan.dry_run,
            ),
            (
                100,
                Some(100),
                Some(100),
                true,
                vec!["Rust".to_owned(), "Tokio".to_owned()],
                ApplicationPlanState::ManualReview,
                true,
            )
        );
        let persisted = std::fs::read_to_string(directory.path().join("application_plans.json"))
            .expect("plans");
        assert!(
            persisted.contains("\"matched_skills\"")
                && !persisted.contains("Private resume summary")
                && !persisted.contains("\"greeting_previews\"")
        );
    }

    #[test]
    fn screening_templates_expose_only_title_and_matched_skills() {
        let (directory, store) = store();
        let policy = CampaignPolicy {
            name: "all".to_owned(),
            include: Vec::new(),
            exclude: Vec::new(),
            required_welfare: Vec::new(),
            monthly_salary_min: None,
            monthly_salary_max: None,
            minimum_score: None,
        };
        let document = ResumeDocument {
            title: "Rust Engineer".to_owned(),
            summary: "Private screening summary".to_owned(),
            skills: vec!["Rust".to_owned(), "UnmatchedSkill".to_owned()],
            ..resume()
        };
        let summary_template = store
            .add_template("summary", "{{resume_summary}}", 1)
            .expect("summary template");
        let error = store
            .screen_plans(
                &[job()],
                &policy,
                &document,
                ScreenPlanOptions {
                    template: Some(&summary_template),
                    limit: 1,
                    minimum_resume_score: 1,
                    now: 10,
                },
            )
            .expect_err("screening summary placeholder");
        assert!(error.to_string().contains("cannot use {{resume_summary}}"));
        assert!(!directory.path().join("application_plans.json").exists());

        let matched_template = store
            .add_template(
                "matched",
                "{{resume_title}} / {{resume_skills}} / {{title}}",
                2,
            )
            .expect("matched template");
        let result = store
            .screen_plans(
                &[job()],
                &policy,
                &document,
                ScreenPlanOptions {
                    template: Some(&matched_template),
                    limit: 1,
                    minimum_resume_score: 1,
                    now: 11,
                },
            )
            .expect("screening preview");
        assert_eq!(
            result.greeting_previews,
            vec![PlanGreetingPreview {
                job_id: "job-1".to_owned(),
                sent: false,
                text: "Rust Engineer / Rust / Rust Engineer".to_owned(),
            }]
        );
        assert!(
            !result.greeting_previews[0].text.contains("UnmatchedSkill")
                && !result.greeting_previews[0]
                    .text
                    .contains("Private screening summary")
        );
    }

    #[test]
    fn resume_screening_keeps_punctuation_significant_technical_skills_distinct() {
        let (_directory, store) = store();
        let policy = CampaignPolicy {
            name: "all".to_owned(),
            include: Vec::new(),
            exclude: Vec::new(),
            required_welfare: Vec::new(),
            monthly_salary_min: None,
            monthly_salary_max: None,
            minimum_score: None,
        };
        let mut c_resume = resume();
        c_resume.title.clear();
        c_resume.skills = vec!["C".to_owned()];
        let technical_job = |id: &str, title: &str| {
            let mut value = job();
            value.id = id.to_owned();
            value.title = title.to_owned();
            value.description.clear();
            value.skills.clear();
            value
        };
        let c_result = store
            .screen_plans(
                &[
                    technical_job("job-cpp", "C++ Engineer"),
                    technical_job("job-c", "C Engineer"),
                    technical_job("job-csharp", "C# Engineer"),
                ],
                &policy,
                &c_resume,
                ScreenPlanOptions {
                    template: None,
                    limit: 10,
                    minimum_resume_score: 1,
                    now: 10,
                },
            )
            .expect("C screening");
        assert_eq!(
            (
                c_result.eligible,
                c_result.skipped_resume_score,
                c_result
                    .plans
                    .iter()
                    .map(|plan| plan.job_id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (1, 2, vec!["job-c"])
        );

        let mut dotnet_resume = c_resume;
        dotnet_resume.name = "dotnet".to_owned();
        dotnet_resume.skills = vec![".NET".to_owned()];
        let dotnet_result = store
            .screen_plans(
                &[
                    technical_job("job-net", "NET Engineer"),
                    technical_job("job-aspnet", "ASP.NET Engineer"),
                    technical_job("job-dotnet", ".NET Engineer"),
                ],
                &policy,
                &dotnet_resume,
                ScreenPlanOptions {
                    template: None,
                    limit: 10,
                    minimum_resume_score: 1,
                    now: 11,
                },
            )
            .expect(".NET screening");
        assert_eq!(
            (
                dotnet_result.eligible,
                dotnet_result.skipped_resume_score,
                dotnet_result
                    .plans
                    .iter()
                    .map(|plan| plan.job_id.as_str())
                    .collect::<Vec<_>>(),
            ),
            (1, 2, vec!["job-dotnet"])
        );
    }

    #[test]
    fn resume_screening_rejects_empty_explicit_match_data_without_persisting() {
        let (_directory, store) = store();
        let policy = CampaignPolicy {
            name: "all".to_owned(),
            include: Vec::new(),
            exclude: Vec::new(),
            required_welfare: Vec::new(),
            monthly_salary_min: None,
            monthly_salary_max: None,
            minimum_score: None,
        };
        let mut document = resume();
        document.title = " \t".to_owned();
        document.skills.clear();
        document.summary = "Rust engineer".to_owned();

        let error = store
            .screen_plans(
                &[job()],
                &policy,
                &document,
                ScreenPlanOptions {
                    template: None,
                    limit: 1,
                    minimum_resume_score: 0,
                    now: 10,
                },
            )
            .expect_err("empty explicit match data");
        assert!(
            error
                .to_string()
                .contains("non-empty title or at least one skill")
        );
        assert!(store.list_plans().expect("plans").is_empty());
    }

    #[test]
    fn resume_score_is_bounded_and_ignores_non_matching_resume_sections() {
        let mut explicit = resume();
        explicit.title = "Rust Engineer".to_owned();
        explicit.skills = vec!["Rust".to_owned(), "Tokio".to_owned()];
        let mut matching_job = job();
        matching_job.description = "Tokio services".to_owned();
        matching_job.skills = vec!["Rust".to_owned()];
        assert_eq!(evaluate_resume(&explicit, &matching_job).score, 100);

        explicit.title.clear();
        explicit.skills.clear();
        explicit.summary = "Rust Engineer with Tokio".to_owned();
        explicit.experience.push(crate::resume::ResumeExperience {
            company: "Example".to_owned(),
            role: "Rust Engineer".to_owned(),
            start_date: "2020".to_owned(),
            end_date: "2024".to_owned(),
            summary: "Built Tokio services".to_owned(),
        });
        assert_eq!(evaluate_resume(&explicit, &matching_job).score, 0);
    }

    #[test]
    fn legacy_application_plans_default_resume_screening_metadata() {
        let plan: ApplicationPlan = serde_json::from_value(serde_json::json!({
            "job_id":"legacy",
            "job_title":"Engineer",
            "company":"Example",
            "policy_name":"all",
            "template_name":null,
            "score":100,
            "dry_run":true,
            "created_at":1
        }))
        .expect("legacy plan");
        assert_eq!(
            (
                plan.policy_score,
                plan.resume_score,
                plan.title_match,
                plan.matched_skills,
            ),
            (None, None, false, Vec::<String>::new())
        );
    }

    #[test]
    fn oversized_greeting_preview_is_rejected_before_any_plan_is_persisted() {
        let (_directory, store) = store();
        let policy = store
            .add_policy(CampaignPolicy {
                name: "all".to_owned(),
                include: Vec::new(),
                exclude: Vec::new(),
                required_welfare: Vec::new(),
                monthly_salary_min: None,
                monthly_salary_max: None,
                minimum_score: None,
            })
            .expect("policy");
        let template = store
            .add_template("summary", "{{resume_summary}}", 1)
            .expect("template");
        let mut document = resume();
        document.summary = "x".repeat(MAX_GREETING_PREVIEW_CHARS + 1);
        let error = store
            .build_plans(&[job()], &policy, Some(&template), Some(&document), 1, 2)
            .expect_err("oversized preview");
        assert!(error.to_string().contains("at most"));
        assert!(store.list_plans().expect("plans").is_empty());
    }
}
