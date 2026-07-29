//! Explicitly confirmed, OpenAI-compatible local AI drafting and scoring.
//!
//! Profiles deliberately contain endpoint metadata only. The runtime API key is
//! read from `BOSS_LLM_API_KEY` immediately before an explicitly confirmed
//! request and is never persisted or included in output.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::data::atomic_write;
use crate::preset::validate_name;
use crate::resume::ResumeDocument;
use crate::{BossError, DataPaths, Job};

const MAX_MODEL_CHARS: usize = 256;
/// Maximum character length for one credential-free OpenAI-compatible base URL.
pub const MAX_AI_BASE_URL_CHARS: usize = 2_048;
const MAX_PROMPT_CHARS: usize = 16 * 1024;
const MAX_MODEL_TEXT_CHARS: usize = 12 * 1024;
const MAX_SCORE_REASONS: usize = 12;
const MAX_REASON_CHARS: usize = 1_024;

/// A credential-free OpenAI-compatible model profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiProfile {
    /// Unique local profile name.
    pub name: String,
    /// Validated HTTPS OpenAI-compatible API base URL.
    pub base_url: String,
    /// Remote model identifier.
    pub model: String,
    /// Creation timestamp.
    pub created_at: u64,
    /// Last update timestamp.
    pub updated_at: u64,
}

/// Strict model-fit score returned by a compatible model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AiScore {
    /// Inclusive fit score from zero through one hundred.
    pub score: u8,
    /// Short nonblank reasons supporting the score.
    pub reasons: Vec<String>,
}

/// Atomic local collection of credential-free AI profiles.
pub struct AiProfileStore {
    path: PathBuf,
}

impl AiProfileStore {
    /// Opens the profile collection below shared data paths.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.ai_profiles(),
        }
    }

    /// Adds or updates one validated profile without persisting credentials.
    pub fn add(
        &self,
        name: &str,
        base_url: &str,
        model: &str,
        now: u64,
    ) -> Result<AiProfile, BossError> {
        let name = validate_name(name)?;
        let base_url = validate_base_url(base_url)?;
        let model = validate_model(model)?;
        let mut profiles = self.read_all()?;
        if let Some(profile) = profiles.iter_mut().find(|profile| profile.name == name) {
            profile.base_url = base_url;
            profile.model = model;
            profile.updated_at = now;
            let updated = profile.clone();
            self.save(&profiles)?;
            return Ok(updated);
        }
        let profile = AiProfile {
            name,
            base_url,
            model,
            created_at: now,
            updated_at: now,
        };
        profiles.push(profile.clone());
        self.save(&profiles)?;
        Ok(profile)
    }

    /// Lists every local profile.
    pub fn list(&self) -> Result<Vec<AiProfile>, BossError> {
        self.read_all()
    }

    /// Shows one validated profile.
    pub fn show(&self, name: &str) -> Result<AiProfile, BossError> {
        let name = validate_name(name)?;
        self.read_all()?
            .into_iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| BossError::Ai("profile not found".to_owned()))
    }

    /// Removes one local profile. Profiles never contain credentials.
    pub fn remove(&self, name: &str) -> Result<AiProfile, BossError> {
        let name = validate_name(name)?;
        let mut profiles = self.read_all()?;
        let index = profiles
            .iter()
            .position(|profile| profile.name == name)
            .ok_or_else(|| BossError::Ai("profile not found".to_owned()))?;
        let removed = profiles.remove(index);
        self.save(&profiles)?;
        Ok(removed)
    }

    fn read_all(&self) -> Result<Vec<AiProfile>, BossError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let mut profiles: Vec<AiProfile> = serde_json::from_slice(&bytes)
                    .map_err(|_| BossError::Ai("profile store is invalid".to_owned()))?;
                for profile in &mut profiles {
                    profile.name = validate_name(&profile.name)
                        .map_err(|_| BossError::Ai("profile store is invalid".to_owned()))?;
                    profile.base_url = validate_base_url(&profile.base_url)
                        .map_err(|_| BossError::Ai("profile store is invalid".to_owned()))?;
                    profile.model = validate_model(&profile.model)
                        .map_err(|_| BossError::Ai("profile store is invalid".to_owned()))?;
                }
                Ok(profiles)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err(BossError::Ai("profile store could not be read".to_owned())),
        }
    }

    fn save(&self, profiles: &[AiProfile]) -> Result<(), BossError> {
        let bytes = serde_json::to_vec_pretty(profiles)
            .map_err(|_| BossError::Ai("profile store could not be encoded".to_owned()))?;
        atomic_write(&self.path, &bytes, |_| {
            BossError::Ai("profile store could not be written".to_owned())
        })
    }
}

/// Validates and normalizes an HTTPS OpenAI-compatible API base URL.
pub fn validate_base_url(value: &str) -> Result<String, BossError> {
    let value = value.trim();
    if value.chars().count() > MAX_AI_BASE_URL_CHARS {
        return Err(BossError::InvalidArgument(format!(
            "AI base URL must contain at most {MAX_AI_BASE_URL_CHARS} characters"
        )));
    }
    let lowered = value.to_ascii_lowercase();
    if lowered.contains('%') {
        return Err(BossError::InvalidArgument(
            "AI base URL path contains unsafe encoded segments".to_owned(),
        ));
    }
    let url = Url::parse(value).map_err(|_| {
        BossError::InvalidArgument("AI base URL must be a valid HTTPS URL".to_owned())
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(BossError::InvalidArgument(
            "AI base URL must be HTTPS without credentials, query, or fragment".to_owned(),
        ));
    }
    if url.path().split('/').any(unsafe_path_segment) {
        return Err(BossError::InvalidArgument(
            "AI base URL path contains unsafe encoded segments".to_owned(),
        ));
    }
    // Constructing the endpoint here also prevents accepting a base that cannot
    // safely be extended as a hierarchical URL.
    let _ = chat_completions_endpoint_from_url(url.clone())?;
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

/// Builds a validated OpenAI chat-completions endpoint without URL joining.
pub fn chat_completions_endpoint(base_url: &str) -> Result<Url, BossError> {
    let normalized = validate_base_url(base_url)?;
    let url = Url::parse(&normalized).map_err(|_| {
        BossError::InvalidArgument("AI base URL must be a valid HTTPS URL".to_owned())
    })?;
    chat_completions_endpoint_from_url(url)
}

fn chat_completions_endpoint_from_url(mut url: Url) -> Result<Url, BossError> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(BossError::InvalidArgument(
            "AI base URL must be HTTPS without credentials, query, or fragment".to_owned(),
        ));
    }
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            BossError::InvalidArgument("AI base URL cannot form a chat endpoint".to_owned())
        })?;
        segments.pop_if_empty();
        segments.push("chat");
        segments.push("completions");
    }
    Ok(url)
}

fn validate_model(value: &str) -> Result<String, BossError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_MODEL_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(BossError::InvalidArgument(format!(
            "AI model must contain 1..{MAX_MODEL_CHARS} non-control characters"
        )));
    }
    Ok(value.to_owned())
}

fn unsafe_path_segment(segment: &str) -> bool {
    let segment = segment.to_ascii_lowercase();
    segment == "."
        || segment == ".."
        || segment.contains("%2f")
        || segment.contains("%5c")
        || segment.contains("%2e")
}

/// Executes a confirmed chat completion with a runtime-only API key.
pub async fn draft(
    profile: &AiProfile,
    job: &Job,
    resume: &ResumeDocument,
) -> Result<String, BossError> {
    let content = chat_completion(
        profile,
        "Write a concise, professional job-application draft based only on the supplied cached job and typed local resume. Return only the draft text. Do not claim unprovided experience.",
        &draft_prompt(job, resume),
    )
    .await?;
    let content = content.trim();
    if content.is_empty() || content.chars().count() > MAX_MODEL_TEXT_CHARS {
        return Err(BossError::AiResponse(
            "draft text was empty or exceeded its bound",
        ));
    }
    Ok(content.to_owned())
}

/// Executes a confirmed strict JSON job-fit score completion.
pub async fn score(
    profile: &AiProfile,
    job: &Job,
    resume: &ResumeDocument,
) -> Result<AiScore, BossError> {
    let content = chat_completion(
        profile,
        "Score the fit between the supplied cached job and typed local resume. Return only strict JSON with exactly {\"score\":0..100,\"reasons\":[nonblank strings]}. Do not use markdown or add fields.",
        &score_prompt(job, resume),
    )
    .await?;
    parse_score_response(&content)
}

async fn chat_completion(
    profile: &AiProfile,
    system: &str,
    user: &str,
) -> Result<String, BossError> {
    let endpoint = chat_completions_endpoint(&profile.base_url)?;
    let key = std::env::var("BOSS_LLM_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .ok_or(BossError::AiApiKeyMissing)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .build()
        .map_err(|_| BossError::AiNetwork)?;
    let response = client
        .post(endpoint)
        .bearer_auth(key)
        .json(&json!({
            "model": profile.model,
            "messages": [
                {"role":"system", "content":system},
                {"role":"user", "content":user}
            ],
            "temperature": 0.2
        }))
        .send()
        .await
        .map_err(|_| BossError::AiNetwork)?;
    let status = response.status();
    if !status.is_success() {
        return Err(BossError::AiHttp {
            status: status.as_u16(),
        });
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| BossError::AiResponse("model response was not valid JSON"))?;
    extract_content(&body)
}

fn extract_content(body: &Value) -> Result<String, BossError> {
    body.get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(BossError::AiResponse(
            "model response had no text completion",
        ))
}

/// Parses the exact score object returned by a model without accepting extras.
pub fn parse_score_response(content: &str) -> Result<AiScore, BossError> {
    let mut score: AiScore = serde_json::from_str(content)
        .map_err(|_| BossError::AiResponse("score response was not the required JSON object"))?;
    if score.score > 100
        || score.reasons.is_empty()
        || score.reasons.len() > MAX_SCORE_REASONS
        || score
            .reasons
            .iter()
            .any(|reason| reason.trim().is_empty() || reason.chars().count() > MAX_REASON_CHARS)
    {
        return Err(BossError::AiResponse(
            "score response did not satisfy score or reason bounds",
        ));
    }
    for reason in &mut score.reasons {
        *reason = reason.trim().to_owned();
    }
    Ok(score)
}

fn draft_prompt(job: &Job, resume: &ResumeDocument) -> String {
    bounded_prompt(
        "Task: draft a professional application message.\n",
        job,
        resume,
    )
}

fn score_prompt(job: &Job, resume: &ResumeDocument) -> String {
    bounded_prompt("Task: assess role fit.\n", job, resume)
}

fn bounded_prompt(prefix: &str, job: &Job, resume: &ResumeDocument) -> String {
    let mut prompt = String::from(prefix);
    prompt.push_str("Cached job:\n");
    append_field(&mut prompt, "title", &job.title, 512);
    append_field(&mut prompt, "company", &job.company, 512);
    append_field(&mut prompt, "city", &job.city, 256);
    append_field(&mut prompt, "salary", &job.salary, 256);
    append_field(&mut prompt, "skills", &job.skills.join(", "), 1_024);
    append_field(&mut prompt, "description", &job.description, 4_096);
    prompt.push_str("Typed local resume:\n");
    append_field(&mut prompt, "title", &resume.title, 512);
    append_field(&mut prompt, "summary", &resume.summary, 2_048);
    append_field(&mut prompt, "skills", &resume.skills.join(", "), 1_024);
    for (key, value) in resume.basics.iter().take(16) {
        append_field(&mut prompt, &format!("basic.{key}"), value, 256);
    }
    for experience in resume.experience.iter().take(8) {
        append_field(
            &mut prompt,
            "experience",
            &format!(
                "{} | {} | {}-{} | {}",
                experience.company,
                experience.role,
                experience.start_date,
                experience.end_date,
                experience.summary
            ),
            1_024,
        );
    }
    for education in resume.education.iter().take(6) {
        append_field(
            &mut prompt,
            "education",
            &format!(
                "{} | {} | {} | {}-{}",
                education.institution,
                education.degree,
                education.field,
                education.start_date,
                education.end_date
            ),
            768,
        );
    }
    for project in resume.projects.iter().take(6) {
        append_field(
            &mut prompt,
            "project",
            &format!(
                "{} | {} | {}",
                project.name, project.description, project.url
            ),
            1_024,
        );
    }
    truncate_chars(&prompt, MAX_PROMPT_CHARS)
}

fn append_field(prompt: &mut String, name: &str, value: &str, limit: usize) {
    if !value.trim().is_empty() {
        prompt.push_str(name);
        prompt.push_str(": ");
        prompt.push_str(&truncate_chars(value.trim(), limit));
        prompt.push('\n');
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn profile_validation_and_persistence_never_need_a_key() {
        let directory = tempdir().expect("tempdir");
        let store = AiProfileStore::from_paths(&DataPaths::new(directory.path()));
        let profile = store
            .add(" local ", "https://model.example/v1/", "example-model", 5)
            .expect("add");
        assert_eq!(profile.name, "local");
        assert_eq!(profile.base_url, "https://model.example/v1");
        assert_eq!(store.show("local").expect("show"), profile);
        let on_disk = std::fs::read_to_string(directory.path().join("ai_profiles.json"))
            .expect("profile data");
        assert!(!on_disk.contains("BOSS_LLM_API_KEY") && !on_disk.contains("api_key"));
        assert_eq!(store.remove("local").expect("remove"), profile);
    }

    #[test]
    fn base_url_and_endpoint_are_strict() {
        assert_eq!(
            chat_completions_endpoint("https://model.example/v1")
                .expect("endpoint")
                .as_str(),
            "https://model.example/v1/chat/completions"
        );
        for value in [
            "http://model.example/v1",
            "https://key@model.example/v1",
            "https://model.example/v1?key=no",
            "https://model.example/v1#fragment",
            "https://model.example/v1/%2e%2e",
            "https://model.example/v1/%2Fother",
            "https://model.example/v1/%252e%252e",
            "https://model.example/v1/%252fother",
        ] {
            assert!(validate_base_url(value).is_err(), "{value}");
        }
        let overlong = format!(
            "https://model.example/{}",
            "a".repeat(MAX_AI_BASE_URL_CHARS)
        );
        assert!(validate_base_url(&overlong).is_err());
    }

    #[test]
    fn score_parser_rejects_malformed_or_out_of_range_data() {
        assert_eq!(
            parse_score_response(r#"{"score":80,"reasons":["Relevant Rust work"]}"#)
                .expect("score")
                .score,
            80
        );
        for value in [
            r#"{"score":101,"reasons":["no"]}"#,
            r#"{"score":80,"reasons":[" "]}"#,
            r#"{"score":80,"reasons":[]}"#,
            r#"{"score":80,"reasons":["ok"],"extra":true}"#,
            "not json",
        ] {
            assert!(parse_score_response(value).is_err(), "{value}");
        }
    }
}
