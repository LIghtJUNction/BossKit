//! Small Rust-native HTTP transport for the recruiter BOSS surface.
//!
//! This module deliberately owns the bounded recruiter list, detail, history,
//! and message routes. The remaining geek chat flow still depends on the
//! legacy helper while its security challenge and MQTT wire protocol are being
//! replaced.

use futures_util::{SinkExt, StreamExt};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use reqwest::redirect::Policy;
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{
    Message, client::IntoClientRequest, http::HeaderValue as WsHeaderValue,
};

use crate::BossError;
use crate::auth;

const BASE_URL: &str = "https://www.zhipin.com";
const RECRUITER_REFERER: &str = "https://www.zhipin.com/web/chat/index";
const CANDIDATE_SEARCH_REFERER: &str = "https://www.zhipin.com/web/frame/search/";
const RECRUITER_FILTER_PATH: &str = "/wapi/zprelation/friend/filterByLabel";
const RECRUITER_FRIEND_DETAIL_PATH: &str = "/wapi/zprelation/friend/getBossFriendListV2.json";
const CANDIDATE_SEARCH_PATH: &str = "/wapi/zpjob/rec/geek/list";
const CANDIDATE_DETAIL_PATH: &str = "/wapi/zpitem/web/boss/search/geek/info";
const RECRUITER_HISTORY_PATH: &str = "/wapi/zpchat/boss/historyMsg";
const RECRUITER_RESUME_PATH: &str = "/wapi/zpboss/h5/geek/detail/get";
const RECRUITER_GREET_PATH: &str = "/wapi/zpjob/chat/start";
const USER_INFO_PATH: &str = "/wapi/zpuser/wap/getUserInfo.json";
const WT_PATH: &str = "/wapi/zppassport/get/wt";
// BOSS returns up to 100 recruiter rows in one page; bound the raw payload
// above that observed size while keeping the CLI response strictly limited.
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
// Search cards carry more nested resume-preview metadata than the friend list.
// Keep this endpoint-specific allowance bounded; parsed output remains small.
const MAX_CANDIDATE_RESPONSE_BYTES: usize = 512 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const MAX_HISTORY_TEXT_CHARS: usize = 2000;
const MAX_RESUME_DESCRIPTION_CHARS: usize = 16 * 1024;
const MAX_RESUME_ITEMS: usize = 20;
pub(crate) const MAX_RECRUITER_INBOX_RECORDS: usize = 20;
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
// Keep exact resume/reply UID lookup bounded. A UID surfaced by the bounded
// inbox scan should normally be found in these same first pages; going wider
// would turn one explicit read into dozens of authenticated requests.
const MAX_FRIEND_SEARCH_PAGES: usize = 3;
// Recruiter resume screening can touch the friend list and history endpoints
// repeatedly. Serialize native requests and leave a conservative, jittered
// gap between them so bulk read-only scans do not resemble a bursty client.
const RECRUITER_REQUEST_MIN_INTERVAL: Duration = Duration::from_millis(1500);
const RECRUITER_REQUEST_MAX_JITTER: Duration = Duration::from_millis(1000);
static LAST_RECRUITER_REQUEST: OnceLock<Mutex<Instant>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecruiterReplyRecord {
    pub(crate) direction: String,
    pub(crate) pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterInboxRecord {
    pub(crate) uid: String,
    pub(crate) name: String,
    pub(crate) job: String,
    pub(crate) last_direction: String,
    pub(crate) last_message: Option<String>,
    pub(crate) pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterSendResult {
    pub(crate) state: String,
    pub(crate) verification: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterGreetResult {
    pub(crate) state: String,
    pub(crate) verification: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterResumeSnapshot {
    pub(crate) uid: String,
    pub(crate) name: String,
    pub(crate) age: String,
    pub(crate) degree: String,
    pub(crate) work_years: String,
    pub(crate) apply_status: String,
    pub(crate) expected_positions: Vec<String>,
    pub(crate) expected_salary: Vec<String>,
    pub(crate) summary: String,
    pub(crate) projects: Vec<RecruiterResumeProject>,
    pub(crate) work_experience: Vec<RecruiterResumeWork>,
    pub(crate) education: Vec<RecruiterResumeEducation>,
    pub(crate) github_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterResumeProject {
    pub(crate) name: String,
    pub(crate) role: String,
    pub(crate) description: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterResumeWork {
    pub(crate) company: String,
    pub(crate) position: String,
    pub(crate) description: String,
    pub(crate) start: String,
    pub(crate) end: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterResumeEducation {
    pub(crate) school: String,
    pub(crate) major: String,
    pub(crate) degree: String,
    pub(crate) description: String,
    pub(crate) start: String,
    pub(crate) end: String,
}

#[derive(Debug)]
pub(crate) struct RecruiterReplies {
    pub(crate) cookie: String,
    pub(crate) verification: String,
    pub(crate) records: Vec<RecruiterReplyRecord>,
}

#[derive(Debug)]
pub(crate) struct RecruiterInboxPage {
    pub(crate) records: Vec<RecruiterInboxRecord>,
    pub(crate) raw_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecruiterCandidateRecord {
    pub(crate) uid: Option<String>,
    pub(crate) encrypt_uid: Option<String>,
    pub(crate) security_id: Option<String>,
    pub(crate) encrypt_job_id: Option<String>,
    pub(crate) expect_id: Option<String>,
    pub(crate) lid: Option<String>,
    /// `Some(false)` is the only state eligible for a new greeting. Missing
    /// or malformed values are deliberately retained as ineligible.
    pub(crate) have_chatted: Option<bool>,
    pub(crate) name: String,
    pub(crate) age: String,
    pub(crate) birth_year: Option<u16>,
    pub(crate) degree: String,
    pub(crate) work_years: String,
    pub(crate) expected_positions: Vec<String>,
    pub(crate) summary: String,
    pub(crate) projects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RecruiterCandidateDetail {
    pub(crate) age: String,
    pub(crate) degree: String,
    pub(crate) work_years: String,
    pub(crate) expected_positions: Vec<String>,
    pub(crate) summary: String,
    pub(crate) projects: Vec<String>,
    pub(crate) work_experience: Vec<String>,
    pub(crate) education: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct RecruiterCandidatePage {
    pub(crate) records: Vec<RecruiterCandidateRecord>,
    pub(crate) has_more: bool,
}

/// Searches only the first recruiter candidate page. The caller performs the
/// hard degree filter and soft ranking locally; this route never opens a chat.
pub(crate) fn recruiter_candidates(
    cookie: &str,
    encrypt_job_id: &str,
    keywords: &str,
    city: Option<&str>,
    limit: usize,
) -> Result<RecruiterCandidatePage, BossError> {
    if !(1..=5).contains(&limit) {
        return Err(BossError::InvalidArgument(
            "recruiter candidate limit must be 1..=5".to_owned(),
        ));
    }
    auth::validate_cookie(cookie)?;
    let encrypt_job_id = encrypt_job_id.trim().to_owned();
    if encrypt_job_id.is_empty()
        || encrypt_job_id.chars().count() > 4096
        || !encrypt_job_id.is_ascii()
    {
        return Err(BossError::InvalidArgument(
            "recruiter candidates encrypted job id is invalid".to_owned(),
        ));
    }
    let keywords = keywords.trim().to_owned();
    let city = city.map(str::to_owned);
    let cookie = cookie.to_owned();
    std::thread::spawn(move || {
        recruiter_candidates_blocking(&cookie, &encrypt_job_id, &keywords, city.as_deref(), limit)
    })
    .join()
    .map_err(|_| transport_error("native recruiter candidate search thread panicked"))?
}

/// Reads one selected search candidate's structured resume summary. The
/// security identifier is kept internal and no contact fields are returned.
pub(crate) fn recruiter_candidate_detail(
    cookie: &str,
    security_id: &str,
    keywords: &str,
) -> Result<RecruiterCandidateDetail, BossError> {
    if security_id.trim().is_empty() || security_id.chars().count() > 4096 {
        return Err(BossError::InvalidArgument(
            "candidate detail identifier is invalid".to_owned(),
        ));
    }
    auth::validate_cookie(cookie)?;
    let cookie = cookie.to_owned();
    let security_id = security_id.to_owned();
    let keywords = keywords.to_owned();
    std::thread::spawn(move || {
        recruiter_candidate_detail_blocking(&cookie, &security_id, &keywords)
    })
    .join()
    .map_err(|_| transport_error("native recruiter candidate detail thread panicked"))?
}

fn recruiter_candidate_detail_blocking(
    cookie: &str,
    security_id: &str,
    keywords: &str,
) -> Result<RecruiterCandidateDetail, BossError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| transport_error("unable to build native Zhipin HTTP client"))?;
    let payload = native_json_get_with_referer(
        &client,
        cookie,
        CANDIDATE_DETAIL_PATH,
        &[
            ("securityId", security_id.to_owned()),
            ("query", keywords.to_owned()),
            ("encryptGeekDetailGray", "1".to_owned()),
        ],
        CANDIDATE_SEARCH_REFERER,
        MAX_CANDIDATE_RESPONSE_BYTES,
    )?;
    parse_recruiter_candidate_detail(&payload)
}

fn parse_recruiter_candidate_detail(
    payload: &Value,
) -> Result<RecruiterCandidateDetail, BossError> {
    let data = payload
        .get("zpData")
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native candidate detail returned no result data"))?;
    let detail = data
        .get("geekDetail")
        .or_else(|| data.get("geekDetailInfo"))
        .and_then(Value::as_object)
        .unwrap_or(data);
    let base = detail
        .get("geekBaseInfo")
        .or_else(|| detail.get("baseInfo"))
        .and_then(Value::as_object)
        .unwrap_or(detail);
    let expected_positions = detail_array_text(
        detail,
        &["geekExpPosList", "expectList", "expectedPositions"],
        &["positionName", "expectPositionName", "name", "position"],
    );
    let projects = detail_array_text(
        detail,
        &["geekProjExpList", "projectExpList", "projects"],
        &[
            "name",
            "projectName",
            "projectDescription",
            "projectDesc",
            "description",
            "performance",
        ],
    );
    let work_experience = detail_array_text(
        detail,
        &["geekWorkExpList", "workExpList", "workExperience"],
        &[
            "company",
            "companyName",
            "positionName",
            "position",
            "responsibility",
            "workContent",
            "description",
            "performance",
        ],
    );
    let education = detail_array_text(
        detail,
        &["geekEduExpList", "educationExpList", "education"],
        &[
            "school",
            "major",
            "degreeName",
            "degreeDesc",
            "description",
            "eduDescription",
        ],
    );
    let summary = candidate_text(
        base,
        detail,
        &["userDescription", "userDesc", "summary", "description"],
    );
    Ok(RecruiterCandidateDetail {
        age: bounded_text(&candidate_text(base, detail, &["ageDesc", "age"]), 64),
        degree: bounded_text(
            &candidate_text(
                base,
                detail,
                &[
                    "highestDegreeName",
                    "degreeCategory",
                    "degreeName",
                    "degree",
                ],
            ),
            64,
        ),
        work_years: bounded_text(
            &candidate_text(base, detail, &["workYearDesc", "workYears", "workYear"]),
            64,
        ),
        expected_positions,
        summary: clean_candidate_text(&summary, 8000),
        projects,
        work_experience,
        education,
    })
}

fn detail_array_text(
    detail: &serde_json::Map<String, Value>,
    keys: &[&str],
    item_keys: &[&str],
) -> Vec<String> {
    for key in keys {
        let Some(items) = detail.get(*key).and_then(Value::as_array) else {
            continue;
        };
        let values = items
            .iter()
            .filter_map(|item| {
                let object = item.as_object()?;
                let parts = item_keys
                    .iter()
                    .filter_map(|key| object.get(*key))
                    .filter_map(value_text)
                    .collect::<Vec<_>>();
                (!parts.is_empty()).then(|| parts.join("："))
            })
            .take(20)
            .map(|value| clean_candidate_text(&value, 2000))
            .collect::<Vec<_>>();
        if !values.is_empty() {
            return values;
        }
    }
    Vec::new()
}

fn recruiter_candidates_blocking(
    cookie: &str,
    encrypt_job_id: &str,
    keywords: &str,
    city: Option<&str>,
    limit: usize,
) -> Result<RecruiterCandidatePage, BossError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| transport_error("unable to build native Zhipin HTTP client"))?;
    let mut query = vec![
        ("page", "1".to_owned()),
        ("jobId", encrypt_job_id.to_owned()),
        ("age", "16,-1".to_owned()),
        ("school", "0".to_owned()),
        ("degree", "0".to_owned()),
        ("experience", "0".to_owned()),
        ("activation", "0".to_owned()),
        ("recentNotView", "0".to_owned()),
        ("exchangeResumeWithColleague", "0".to_owned()),
        ("gender", "0".to_owned()),
        ("major", "0".to_owned()),
        ("keyword1", keywords.to_owned()),
        ("switchJobFrequency", "0".to_owned()),
    ];
    if let Some(city) = city.filter(|value| !value.is_empty()) {
        query.push(("city", city.to_owned()));
    }
    let payload = native_json_get_with_referer(
        &client,
        cookie,
        CANDIDATE_SEARCH_PATH,
        &query,
        CANDIDATE_SEARCH_REFERER,
        MAX_CANDIDATE_RESPONSE_BYTES,
    )?;
    parse_recruiter_candidates(&payload, limit, encrypt_job_id)
}

/// Fetches one bounded recruiter friend-list page without invoking Python.
pub(crate) fn recruiter_replies(
    cookie: &str,
    limit: usize,
    page: usize,
) -> Result<RecruiterReplies, BossError> {
    validate_bounds(limit, page)?;
    auth::validate_cookie(cookie)?;
    let cookie = cookie.to_owned();
    std::thread::spawn(move || recruiter_replies_blocking(&cookie, limit, page))
        .join()
        .map_err(|_| transport_error("native Zhipin recruiter request thread panicked"))?
}

fn recruiter_replies_blocking(
    cookie: &str,
    limit: usize,
    page: usize,
) -> Result<RecruiterReplies, BossError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| transport_error("unable to build native Zhipin HTTP client"))?;
    let payload = recruiter_payload(&client, cookie, page)?;
    let bytes = serde_json::to_vec(&payload)
        .map_err(|_| transport_error("native recruiter response could not be encoded"))?;
    parse_recruiter_response(&bytes, cookie, limit)
}

/// Reads recruiter-side conversations and the latest textual message using
/// only the native HTTP client. Identifiers are returned solely so an explicit
/// `recruiter reply` command can target one exact conversation.
pub(crate) fn recruiter_inbox(
    cookie: &str,
    limit: usize,
    page: usize,
    job_filter: Option<&str>,
) -> Result<RecruiterInboxPage, BossError> {
    if !(1..=MAX_RECRUITER_INBOX_RECORDS).contains(&limit) || !(1..=50).contains(&page) {
        return Err(BossError::InvalidArgument(
            "recruiter inbox limit must be 1..=20 and page must be 1..=50".to_owned(),
        ));
    }
    auth::validate_cookie(cookie)?;
    let cookie = cookie.to_owned();
    let job_filter = job_filter.map(str::to_owned);
    std::thread::spawn(move || recruiter_inbox_blocking(&cookie, limit, page, job_filter))
        .join()
        .map_err(|_| transport_error("native Zhipin recruiter inbox thread panicked"))?
}

/// Reads one exact candidate's full recruiter-side resume detail without a browser.
pub(crate) fn recruiter_resume(
    cookie: &str,
    uid: &str,
) -> Result<RecruiterResumeSnapshot, BossError> {
    auth::validate_cookie(cookie)?;
    let uid = uid.to_owned();
    let cookie = cookie.to_owned();
    std::thread::spawn(move || recruiter_resume_blocking(&cookie, &uid))
        .join()
        .map_err(|_| transport_error("native recruiter resume thread panicked"))?
}

/// Sends one recruiter-side initial greeting after re-reading the same
/// job-scoped recommendation list and matching the exact eligible card.
pub(crate) fn recruiter_greet(
    cookie: &str,
    encrypt_geek_id: &str,
    security_id: &str,
    encrypt_job_id: &str,
    expect_id: &str,
    lid: &str,
    message: &str,
) -> Result<RecruiterGreetResult, BossError> {
    let message = normalize_message(message)?;
    validate_recruiter_greet_identifiers(
        encrypt_geek_id,
        security_id,
        encrypt_job_id,
        expect_id,
        lid,
    )?;
    auth::validate_cookie(cookie)?;
    let cookie = cookie.to_owned();
    let encrypt_geek_id = encrypt_geek_id.to_owned();
    let security_id = security_id.to_owned();
    let encrypt_job_id = encrypt_job_id.to_owned();
    let expect_id = expect_id.to_owned();
    let lid = lid.to_owned();
    std::thread::spawn(move || {
        recruiter_greet_blocking(
            &cookie,
            &encrypt_geek_id,
            &security_id,
            &encrypt_job_id,
            &expect_id,
            &lid,
            &message,
        )
    })
    .join()
    .map_err(|_| transport_error("native recruiter greeting thread panicked"))?
}

pub(crate) fn validate_recruiter_greet_identifiers(
    encrypt_geek_id: &str,
    security_id: &str,
    encrypt_job_id: &str,
    expect_id: &str,
    lid: &str,
) -> Result<(), BossError> {
    for (field, value) in [
        ("encrypted candidate id", encrypt_geek_id),
        ("security id", security_id),
        ("encrypted job id", encrypt_job_id),
        ("expectation id", expect_id),
    ] {
        if value.trim().is_empty() || value.chars().count() > 4096 || !value.is_ascii() {
            return Err(BossError::InvalidArgument(format!(
                "recruiter greet {field} is invalid"
            )));
        }
    }
    if lid.chars().count() > 4096 || !lid.is_ascii() {
        return Err(BossError::InvalidArgument(
            "recruiter greet lid is invalid".to_owned(),
        ));
    }
    Ok(())
}

fn recruiter_greet_blocking(
    cookie: &str,
    encrypt_geek_id: &str,
    security_id: &str,
    encrypt_job_id: &str,
    expect_id: &str,
    lid: &str,
    message: &str,
) -> Result<RecruiterGreetResult, BossError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| transport_error("unable to build native Zhipin HTTP client"))?;
    let form = recruiter_greet_form(
        encrypt_geek_id,
        encrypt_job_id,
        expect_id,
        lid,
        message,
        security_id,
    );
    recruiter_greet_after_preflight(
        || recruiter_greeting_preflight_candidates(&client, cookie, encrypt_job_id),
        || native_recruiter_greet_post(&client, cookie, &form),
        encrypt_geek_id,
        security_id,
        encrypt_job_id,
        expect_id,
        lid,
    )
}

fn recruiter_greet_after_preflight<F, W>(
    preflight: F,
    write: W,
    encrypt_geek_id: &str,
    security_id: &str,
    encrypt_job_id: &str,
    expect_id: &str,
    lid: &str,
) -> Result<RecruiterGreetResult, BossError>
where
    F: FnOnce() -> Result<RecruiterCandidatePage, BossError>,
    W: FnOnce() -> Result<RecruiterGreetWriteOutcome, BossError>,
{
    let page = preflight()?;
    ensure_recruiter_greet_preflight(
        &page.records,
        encrypt_geek_id,
        security_id,
        encrypt_job_id,
        expect_id,
        lid,
    )?;
    match write()? {
        RecruiterGreetWriteOutcome::Accepted => Ok(RecruiterGreetResult {
            state: "api_accepted".to_owned(),
            verification: "chat_start_api_status_1".to_owned(),
        }),
        RecruiterGreetWriteOutcome::Rejected => Ok(RecruiterGreetResult {
            state: "rejected".to_owned(),
            verification: "chat_start_rejected".to_owned(),
        }),
        RecruiterGreetWriteOutcome::Unknown => Ok(RecruiterGreetResult {
            state: "unverified".to_owned(),
            verification: "chat_start_write_outcome_unknown".to_owned(),
        }),
    }
}

fn recruiter_greeting_preflight_candidates(
    client: &Client,
    cookie: &str,
    encrypt_job_id: &str,
) -> Result<RecruiterCandidatePage, BossError> {
    let payload = native_json_get_with_referer(
        client,
        cookie,
        CANDIDATE_SEARCH_PATH,
        &[
            ("page", "1".to_owned()),
            ("jobId", encrypt_job_id.to_owned()),
            ("age", "16,-1".to_owned()),
            ("school", "0".to_owned()),
            ("degree", "0".to_owned()),
            ("experience", "0".to_owned()),
            ("activation", "0".to_owned()),
            ("recentNotView", "0".to_owned()),
            ("exchangeResumeWithColleague", "0".to_owned()),
            ("gender", "0".to_owned()),
            ("major", "0".to_owned()),
            ("keyword1", "-1".to_owned()),
            ("switchJobFrequency", "0".to_owned()),
        ],
        CANDIDATE_SEARCH_REFERER,
        MAX_CANDIDATE_RESPONSE_BYTES,
    )?;
    parse_recruiter_candidates(&payload, MAX_RECRUITER_INBOX_RECORDS, encrypt_job_id)
}

fn ensure_recruiter_greet_preflight(
    candidates: &[RecruiterCandidateRecord],
    encrypt_geek_id: &str,
    security_id: &str,
    encrypt_job_id: &str,
    expect_id: &str,
    lid: &str,
) -> Result<(), BossError> {
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.encrypt_uid.as_deref() == Some(encrypt_geek_id))
        .ok_or_else(|| {
            BossError::InvalidArgument(
                "recruiter greet candidate was not found in the fresh recommendation list"
                    .to_owned(),
            )
        })?;
    if candidate.have_chatted != Some(false) {
        return Err(BossError::InvalidArgument(
            "recruiter greet candidate is not eligible in the fresh recommendation list".to_owned(),
        ));
    }
    if candidate.security_id.as_deref() != Some(security_id)
        || candidate.encrypt_job_id.as_deref() != Some(encrypt_job_id)
        || candidate.expect_id.as_deref() != Some(expect_id)
        || candidate.lid.as_deref() != Some(lid)
    {
        return Err(BossError::InvalidArgument(
            "recruiter greet candidate metadata did not match the fresh recommendation card"
                .to_owned(),
        ));
    }
    Ok(())
}

fn recruiter_greet_form(
    encrypt_geek_id: &str,
    encrypt_job_id: &str,
    expect_id: &str,
    lid: &str,
    message: &str,
    security_id: &str,
) -> [(&'static str, String); 8] {
    [
        ("gid", encrypt_geek_id.to_owned()),
        ("suid", String::new()),
        ("jid", encrypt_job_id.to_owned()),
        ("expectId", expect_id.to_owned()),
        ("lid", lid.to_owned()),
        ("greet", message.to_owned()),
        ("from", String::new()),
        ("securityId", security_id.to_owned()),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecruiterGreetWriteOutcome {
    Accepted,
    Rejected,
    Unknown,
}

fn native_recruiter_greet_post(
    client: &Client,
    cookie: &str,
    form: &[(&str, String)],
) -> Result<RecruiterGreetWriteOutcome, BossError> {
    let request_headers = headers(cookie)?;
    wait_for_recruiter_request_slot();
    let response = match client
        .post(format!("{BASE_URL}{RECRUITER_GREET_PATH}"))
        .headers(request_headers)
        .form(form)
        .send()
    {
        Ok(response) => response,
        Err(_) => return Ok(RecruiterGreetWriteOutcome::Unknown),
    };
    let status = response.status().as_u16();
    if !response.status().is_success() {
        if recruiter_greet_hard_stop_status(status) {
            return Err(BossError::Http {
                status,
                message: "native Zhipin recruiter greeting was rejected".to_owned(),
            });
        }
        return Ok(RecruiterGreetWriteOutcome::Unknown);
    }
    let bytes = match read_bounded_response(response, MAX_RESPONSE_BYTES) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(RecruiterGreetWriteOutcome::Unknown),
    };
    parse_recruiter_greet_write_response(&bytes)
}

fn recruiter_greet_hard_stop_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 429)
}

fn parse_recruiter_greet_write_response(
    bytes: &[u8],
) -> Result<RecruiterGreetWriteOutcome, BossError> {
    let Ok(payload) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(RecruiterGreetWriteOutcome::Unknown);
    };
    let Some(code) = payload.get("code").and_then(Value::as_i64) else {
        return Ok(RecruiterGreetWriteOutcome::Unknown);
    };
    if matches!(code, 9 | 36 | 37) {
        return Err(transport_error(&format!(
            "Zhipin recruiter greeting stopped for risk-control API code {code}; request was not retried"
        )));
    }
    if code != 0 {
        return Ok(RecruiterGreetWriteOutcome::Rejected);
    }
    let status = payload
        .get("zpData")
        .and_then(Value::as_object)
        .and_then(|data| data.get("status"))
        .and_then(Value::as_i64);
    Ok(match status {
        Some(1) => RecruiterGreetWriteOutcome::Accepted,
        Some(_) => RecruiterGreetWriteOutcome::Rejected,
        None => RecruiterGreetWriteOutcome::Unknown,
    })
}

fn recruiter_resume_blocking(
    cookie: &str,
    uid: &str,
) -> Result<RecruiterResumeSnapshot, BossError> {
    let target_uid = uid
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            BossError::InvalidArgument("recruiter resume uid must be a positive integer".to_owned())
        })?;
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| transport_error("unable to build native Zhipin HTTP client"))?;
    let friend = find_recruiter_friend(&client, cookie, target_uid)?;
    let payload = native_json_get(
        &client,
        cookie,
        RECRUITER_RESUME_PATH,
        &[
            ("entrance", "7".to_owned()),
            ("uid", target_uid.to_string()),
            ("encryptJid", friend.encrypt_job_id),
            ("securityId", friend.security_id),
            ("source", "1".to_owned()),
            ("encryptExpId", friend.encrypt_expect_id.unwrap_or_default()),
        ],
    )?;
    parse_recruiter_resume(&payload, target_uid)
}

fn parse_recruiter_resume(
    payload: &Value,
    target_uid: i64,
) -> Result<RecruiterResumeSnapshot, BossError> {
    let detail = payload
        .get("zpData")
        .and_then(Value::as_object)
        .and_then(|data| data.get("geekDetailInfo"))
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native recruiter resume returned no detail"))?;
    let base = detail
        .get("geekBaseInfo")
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native recruiter resume returned no base info"))?;
    let expected = detail
        .get("geekExpPosList")
        .and_then(Value::as_array)
        .map(|items| items.iter().take(MAX_RESUME_ITEMS).collect::<Vec<_>>())
        .unwrap_or_default();
    let projects = detail
        .get("geekProjExpList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(MAX_RESUME_ITEMS)
                .filter_map(parse_resume_project)
                .collect()
        })
        .unwrap_or_default();
    let work_experience = detail
        .get("geekWorkExpList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(MAX_RESUME_ITEMS)
                .filter_map(parse_resume_work)
                .collect()
        })
        .unwrap_or_default();
    let education = detail
        .get("geekEduExpList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(MAX_RESUME_ITEMS)
                .filter_map(parse_resume_education)
                .collect()
        })
        .unwrap_or_default();
    let summary = mask_contact_markup_bounded(
        base.get("userDescription")
            .and_then(Value::as_str)
            .unwrap_or(""),
        MAX_RESUME_DESCRIPTION_CHARS,
    );
    Ok(RecruiterResumeSnapshot {
        uid: target_uid.to_string(),
        name: bounded_text(
            base.get("name").and_then(Value::as_str).unwrap_or("候选人"),
            128,
        ),
        age: bounded_text(
            base.get("ageDesc").and_then(Value::as_str).unwrap_or(""),
            64,
        ),
        degree: bounded_text(
            base.get("degreeCategory")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
        work_years: bounded_text(
            base.get("workYearDesc")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
        apply_status: bounded_text(
            base.get("applyStatusContent")
                .and_then(Value::as_str)
                .unwrap_or(""),
            128,
        ),
        expected_positions: expected
            .iter()
            .filter_map(|item| item.get("positionName").and_then(Value::as_str))
            .map(|value| bounded_text(value, 128))
            .collect(),
        expected_salary: expected
            .iter()
            .filter_map(|item| item.get("salaryDesc").and_then(Value::as_str))
            .map(|value| bounded_text(value, 64))
            .collect(),
        github_refs: extract_github_refs(&summary),
        summary,
        projects,
        work_experience,
        education,
    })
}

fn parse_resume_project(value: &Value) -> Option<RecruiterResumeProject> {
    let object = value.as_object()?;
    Some(RecruiterResumeProject {
        name: bounded_text(
            object.get("name").and_then(Value::as_str).unwrap_or(""),
            128,
        ),
        role: bounded_text(
            object.get("roleName").and_then(Value::as_str).unwrap_or(""),
            128,
        ),
        description: mask_contact_markup_bounded(
            object
                .get("projectDescription")
                .and_then(Value::as_str)
                .unwrap_or(""),
            2000,
        ),
        start: bounded_text(
            object
                .get("startDateDesc")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
        end: bounded_text(
            object
                .get("endDateDesc")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
        url: mask_contact_markup_bounded(
            object.get("url").and_then(Value::as_str).unwrap_or(""),
            512,
        ),
    })
}

fn parse_resume_work(value: &Value) -> Option<RecruiterResumeWork> {
    let object = value.as_object()?;
    Some(RecruiterResumeWork {
        company: bounded_text(
            object.get("company").and_then(Value::as_str).unwrap_or(""),
            128,
        ),
        position: bounded_text(
            object.get("position").and_then(Value::as_str).unwrap_or(""),
            128,
        ),
        description: mask_contact_markup_bounded(
            object
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or(""),
            2000,
        ),
        start: bounded_text(
            object
                .get("startDateDesc")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
        end: bounded_text(
            object
                .get("endDateDesc")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
    })
}

fn parse_resume_education(value: &Value) -> Option<RecruiterResumeEducation> {
    let object = value.as_object()?;
    Some(RecruiterResumeEducation {
        school: bounded_text(
            object.get("school").and_then(Value::as_str).unwrap_or(""),
            128,
        ),
        major: bounded_text(
            object.get("major").and_then(Value::as_str).unwrap_or(""),
            128,
        ),
        degree: bounded_text(
            object
                .get("degreeName")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
        description: mask_contact_markup_bounded(
            object
                .get("eduDescription")
                .and_then(Value::as_str)
                .unwrap_or(""),
            1000,
        ),
        start: bounded_text(
            object
                .get("startDateDesc")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
        end: bounded_text(
            object
                .get("endDateDesc")
                .and_then(Value::as_str)
                .unwrap_or(""),
            64,
        ),
    })
}

fn extract_github_refs(value: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for token in value.split_whitespace() {
        let Some(start) = token.to_ascii_lowercase().find("github.com/") else {
            continue;
        };
        let reference = token[start..]
            .trim_matches(|character: char| "()[]{}<>.,;\"'".contains(character))
            .to_owned();
        if !reference.is_empty() && !refs.contains(&reference) && refs.len() < 20 {
            refs.push(reference);
        }
    }
    refs
}

fn parse_recruiter_candidates(
    payload: &Value,
    limit: usize,
    encrypt_job_id: &str,
) -> Result<RecruiterCandidatePage, BossError> {
    if !(1..=MAX_RECRUITER_INBOX_RECORDS).contains(&limit) {
        return Err(BossError::InvalidArgument(
            "recruiter candidate parse limit must be 1..=20".to_owned(),
        ));
    }
    let data = payload
        .get("zpData")
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native candidate response returned no result data"))?;
    let has_more = data
        .get("hasMore")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let items: &[Value] = match data.get("geekList").and_then(Value::as_array) {
        Some(items) => items.as_slice(),
        None if data.is_empty() || !has_more => &[],
        None => {
            return Err(transport_error(
                "native candidate response returned no compatible result list",
            ));
        }
    };
    let records = items
        .iter()
        .take(MAX_RECRUITER_INBOX_RECORDS)
        .filter_map(|item| parse_recruiter_candidate(item, encrypt_job_id))
        .collect();
    Ok(RecruiterCandidatePage { records, has_more })
}

fn parse_recruiter_candidate(
    value: &Value,
    encrypt_job_id: &str,
) -> Option<RecruiterCandidateRecord> {
    let item = value.as_object()?;
    let card = item
        .get("geekCard")
        .and_then(Value::as_object)
        .unwrap_or(item);
    let uid = candidate_text(card, item, &["uid", "geekId"]);
    let encrypt_uid = candidate_text(card, item, &["encryptGeekId", "encryptUid"]);
    let security_id = candidate_text(card, item, &["securityId"]);
    let expect_id = candidate_text(card, item, &["expectId", "encryptExpectId"]);
    let lid = candidate_text(card, item, &["lid"]);
    let have_chatted = item
        .get("haveChatted")
        .or_else(|| card.get("haveChatted"))
        .map(Value::as_bool)
        .unwrap_or(None);
    let name = candidate_text(card, item, &["name", "geekName"]);
    let age = candidate_text(card, item, &["ageDesc", "age"]);
    let degree = candidate_text(
        card,
        item,
        &[
            "highestDegreeName",
            "degreeCategory",
            "degreeName",
            "degree",
        ],
    );
    let work_years = candidate_text(card, item, &["workYearDesc", "workYear"]);
    let mut expected_positions = candidate_list(
        card,
        item,
        &[
            "expectedPositions",
            "expectPositionName",
            "expectPosition",
            "expectedPosition",
            "positionName",
        ],
    );
    if expected_positions.is_empty() {
        let expected = candidate_text(card, item, &["highlightExpectName"]);
        let expected = if expected.is_empty() {
            candidate_nested_text(card, item, "expect", "name")
        } else {
            expected
        };
        if !expected.is_empty() {
            expected_positions.push(clean_candidate_text(&expected, 1000));
        }
    }
    let mut summary = candidate_text(
        card,
        item,
        &[
            "userDescription",
            "geekDescription",
            "selfIntroduction",
            "introduce",
            "description",
            "advantage",
        ],
    );
    if summary.is_empty() {
        summary = candidate_text(card, item, &["highlightGeekDescName"]);
        if summary.is_empty() {
            summary = candidate_nested_text(card, item, "geekDesc", "name");
        }
    }
    let mut projects = candidate_list(
        card,
        item,
        &[
            "projectExperience",
            "projectDesc",
            "projects",
            "projectList",
        ],
    );
    if projects.is_empty() {
        projects = candidate_card_fields(card, &["PROJECT", "WORK", "EXPERIENCE"]);
    }
    if name.is_empty()
        && age.is_empty()
        && degree.is_empty()
        && work_years.is_empty()
        && expected_positions.is_empty()
        && summary.is_empty()
        && projects.is_empty()
    {
        return None;
    }
    let birth_value = candidate_text(
        card,
        item,
        &["birthYear", "birthday", "birthdayDesc", "birthDate"],
    );
    let birth_year = parse_birth_year(&birth_value);
    Some(RecruiterCandidateRecord {
        uid: (!uid.is_empty()).then_some(uid),
        encrypt_uid: (!encrypt_uid.is_empty()).then_some(encrypt_uid),
        security_id: (!security_id.is_empty()).then_some(security_id),
        encrypt_job_id: Some(encrypt_job_id.to_owned()),
        expect_id: (!expect_id.is_empty()).then_some(expect_id),
        lid: Some(lid),
        have_chatted,
        name: bounded_text(if name.is_empty() { "候选人" } else { &name }, 128),
        age: bounded_text(&age, 64),
        birth_year,
        degree: bounded_text(&degree, 64),
        work_years: bounded_text(&work_years, 64),
        expected_positions,
        summary: clean_candidate_text(&summary, 4000),
        projects,
    })
}

fn candidate_text(
    card: &serde_json::Map<String, Value>,
    item: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> String {
    [card, item]
        .into_iter()
        .flat_map(|object| keys.iter().filter_map(|key| object.get(*key)))
        .find_map(value_text)
        .unwrap_or_default()
}

fn candidate_nested_text(
    card: &serde_json::Map<String, Value>,
    item: &serde_json::Map<String, Value>,
    outer_key: &str,
    inner_key: &str,
) -> String {
    [card, item]
        .into_iter()
        .filter_map(|object| object.get(outer_key))
        .filter_map(Value::as_object)
        .filter_map(|object| object.get(inner_key))
        .find_map(value_text)
        .unwrap_or_default()
}

fn candidate_card_fields(card: &serde_json::Map<String, Value>, kinds: &[&str]) -> Vec<String> {
    card.get("cardFields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|field| {
            field
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kinds.iter().any(|expected| kind.contains(expected)))
        })
        .filter_map(candidate_entry_text)
        .take(10)
        .map(|value| clean_candidate_text(&value, 1000))
        .collect()
}

fn candidate_list(
    card: &serde_json::Map<String, Value>,
    item: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Vec<String> {
    for object in [card, item] {
        for key in keys {
            let Some(value) = object.get(*key) else {
                continue;
            };
            let mut values: Vec<String> = match value {
                Value::Array(items) => items.iter().filter_map(candidate_entry_text).collect(),
                _ => candidate_entry_text(value).into_iter().collect(),
            };
            values.retain(|value| !value.is_empty());
            if !values.is_empty() {
                return values
                    .into_iter()
                    .take(10)
                    .map(|value| clean_candidate_text(&value, 1000))
                    .collect();
            }
        }
    }
    Vec::new()
}

fn candidate_entry_text(value: &Value) -> Option<String> {
    if let Some(text) = value_text(value) {
        return Some(text);
    }
    let object = value.as_object()?;
    let name = [
        "projectName",
        "name",
        "positionName",
        "title",
        "projectDescription",
        "description",
        "content",
    ]
    .into_iter()
    .filter_map(|key| object.get(key))
    .filter_map(value_text)
    .chain(
        object
            .get("text")
            .and_then(Value::as_object)
            .into_iter()
            .filter_map(|text| text.get("content"))
            .filter_map(value_text),
    )
    .collect::<Vec<_>>();
    (!name.is_empty()).then(|| name.join("："))
}

fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn clean_candidate_text(value: &str, max_chars: usize) -> String {
    let mut plain = String::with_capacity(value.len());
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => plain.push(character),
            _ => {}
        }
    }
    mask_contact_markup_bounded(&plain, max_chars)
}

fn parse_birth_year(value: &str) -> Option<u16> {
    let digits = value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| part.len() == 4)
        .and_then(|part| part.parse::<u16>().ok())?;
    (1900..=2100).contains(&digits).then_some(digits)
}

fn recruiter_inbox_blocking(
    cookie: &str,
    limit: usize,
    page: usize,
    job_filter: Option<String>,
) -> Result<RecruiterInboxPage, BossError> {
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| transport_error("unable to build native Zhipin HTTP client"))?;
    let payload = recruiter_payload(&client, cookie, page)?;
    let data = payload
        .get("zpData")
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native recruiter response returned no result data"))?;
    let items = recruiter_items(data)?;
    let raw_count = items.len();
    let list_objects = items
        .iter()
        .take(limit)
        .filter_map(Value::as_object)
        .filter(|object| recruiter_uid(object).is_some_and(|uid| uid > 0))
        .filter(|object| {
            job_filter.as_ref().is_none_or(|filter| {
                object
                    .get("jobName")
                    .and_then(Value::as_str)
                    .is_some_and(|job| job.contains(filter.as_str()))
            })
        })
        .collect::<Vec<_>>();
    if list_objects.is_empty() {
        return Ok(RecruiterInboxPage {
            records: Vec::new(),
            raw_count,
        });
    }

    let friend_ids = list_objects
        .iter()
        .filter_map(|object| recruiter_uid(object))
        .collect::<Vec<_>>();
    let detail_payload = recruiter_friend_detail_payload(&client, cookie, &friend_ids)?;
    let detail_data = detail_payload
        .get("zpData")
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native recruiter detail returned no result data"))?;
    let detail_items = recruiter_items(detail_data)?;
    let detail_by_uid = detail_items
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|object| recruiter_uid(object).map(|uid| (uid, object)))
        .collect::<std::collections::HashMap<_, _>>();
    let user_id = current_user_id(&client, cookie)?;
    let mut records = Vec::with_capacity(list_objects.len());
    for list_object in list_objects {
        let Some(uid) = recruiter_uid(list_object).filter(|value| *value > 0) else {
            continue;
        };
        let object = detail_by_uid.get(&uid).copied().unwrap_or(list_object);
        let job = object
            .get("jobName")
            .and_then(Value::as_str)
            .or_else(|| list_object.get("jobName").and_then(Value::as_str))
            .unwrap_or("");
        let security_id = object
            .get("securityId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                list_object
                    .get("securityId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            })
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let Some(security_id) = security_id else {
            continue;
        };
        let messages = recruiter_history(&client, cookie, uid, &security_id)?;
        let latest = messages
            .iter()
            .filter_map(|message| {
                let text = message.get("text").and_then(Value::as_str).or_else(|| {
                    message
                        .get("body")
                        .and_then(Value::as_object)
                        .and_then(|body| body.get("text"))
                        .and_then(Value::as_str)
                })?;
                let sender = message
                    .get("from")
                    .and_then(Value::as_object)
                    .and_then(|from| from.get("uid"))
                    .and_then(Value::as_i64)?;
                Some((sender, text.to_owned()))
            })
            .next_back();
        let (last_direction, last_message) = match latest {
            Some((sender, text)) if sender != user_id => (
                "candidate_to_recruiter".to_owned(),
                Some(mask_contact_markup(&text)),
            ),
            Some((_, text)) => (
                "recruiter_to_candidate".to_owned(),
                Some(mask_contact_markup(&text)),
            ),
            None => ("unknown".to_owned(), None),
        };
        records.push(RecruiterInboxRecord {
            uid: uid.to_string(),
            name: bounded_text(
                object
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| list_object.get("name").and_then(Value::as_str))
                    .unwrap_or("候选人"),
                128,
            ),
            job: bounded_text(job, 256),
            pending: last_direction == "candidate_to_recruiter",
            last_direction,
            last_message,
        });
    }
    Ok(RecruiterInboxPage { records, raw_count })
}

/// Sends one recruiter message through the native MQTT-over-WebSocket path
/// and verifies it through the recruiter history endpoint.
pub(crate) fn recruiter_send(
    cookie: &str,
    uid: &str,
    message: &str,
) -> Result<RecruiterSendResult, BossError> {
    auth::validate_cookie(cookie)?;
    let uid = uid.to_owned();
    let message = message.to_owned();
    let cookie = cookie.to_owned();
    std::thread::spawn(move || recruiter_send_blocking(&cookie, &uid, &message))
        .join()
        .map_err(|_| transport_error("native Zhipin recruiter send thread panicked"))?
}

fn recruiter_send_blocking(
    cookie: &str,
    uid: &str,
    message: &str,
) -> Result<RecruiterSendResult, BossError> {
    let message = normalize_message(message)?;
    let target_uid = uid
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            BossError::InvalidArgument("recruiter reply uid must be a positive integer".to_owned())
        })?;
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| transport_error("unable to build native Zhipin HTTP client"))?;
    let friend = find_recruiter_friend(&client, cookie, target_uid)?;
    let user_id = current_user_id(&client, cookie)?;
    let user_token = current_user_token(&client, cookie)?;
    let before = recruiter_history(&client, cookie, target_uid, &friend.security_id)?;
    if has_outgoing_text(&before, user_id, target_uid, &message) {
        return Ok(RecruiterSendResult {
            state: "already_sent".to_owned(),
            verification: "exact_outgoing_text_in_recruiter_history".to_owned(),
        });
    }
    let wt_payload = native_json_get(&client, cookie, WT_PATH, &[])?;
    let wt2 = wt_payload
        .get("zpData")
        .and_then(Value::as_object)
        .and_then(|data| data.get("wt2"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("native recruiter websocket credential was invalid"))?;
    let payload = encode_protocol(
        user_id,
        target_uid,
        &friend.encrypt_uid,
        friend.friend_source,
        &message,
        now_millis()?,
    )?;
    let publication_failed = publish_mqtt(cookie, &user_token, wt2, payload).is_err();
    for attempt in 0..3 {
        let after = recruiter_history(&client, cookie, target_uid, &friend.security_id)?;
        if has_outgoing_text(&after, user_id, target_uid, &message) {
            return Ok(RecruiterSendResult {
                state: "message_verified".to_owned(),
                verification: "exact_outgoing_text_in_recruiter_history".to_owned(),
            });
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    Ok(RecruiterSendResult {
        state: "unverified".to_owned(),
        verification: if publication_failed {
            "recruiter_publish_failed_without_exact_history"
        } else {
            "recruiter_publish_acknowledged_without_exact_history"
        }
        .to_owned(),
    })
}

fn headers(cookie: &str) -> Result<HeaderMap, BossError> {
    headers_with_referer(cookie, RECRUITER_REFERER)
}

fn headers_with_referer(cookie: &str, referer: &str) -> Result<HeaderMap, BossError> {
    let mut map = HeaderMap::new();
    map.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    map.insert(
        REFERER,
        HeaderValue::from_str(referer)
            .map_err(|_| transport_error("native Zhipin referer header was invalid"))?,
    );
    map.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    map.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    map.insert(
        COOKIE,
        HeaderValue::from_str(cookie).map_err(|_| {
            transport_error("stored Zhipin Cookie cannot be used by native transport")
        })?,
    );
    Ok(map)
}

fn recruiter_payload(client: &Client, cookie: &str, page: usize) -> Result<Value, BossError> {
    native_json_post(
        client,
        cookie,
        RECRUITER_FILTER_PATH,
        &[("labelId", "0".to_owned()), ("page", page.to_string())],
    )
}

fn recruiter_friend_detail_payload(
    client: &Client,
    cookie: &str,
    friend_ids: &[i64],
) -> Result<Value, BossError> {
    if friend_ids.is_empty() {
        return Err(transport_error(
            "native recruiter detail requires at least one friend id",
        ));
    }
    native_json_post(
        client,
        cookie,
        RECRUITER_FRIEND_DETAIL_PATH,
        &[(
            "friendIds",
            friend_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )],
    )
}

fn recruiter_items(data: &serde_json::Map<String, Value>) -> Result<&[Value], BossError> {
    for key in ["friendList", "result", "list", "items"] {
        if let Some(items) = data.get(key).and_then(Value::as_array) {
            return Ok(items);
        }
    }
    if data.is_empty()
        || data
            .get("hasMore")
            .and_then(Value::as_bool)
            .is_some_and(|has_more| !has_more)
    {
        return Ok(&[]);
    }
    Err(transport_error(
        "native recruiter response returned no compatible result list",
    ))
}

fn recruiter_uid(object: &serde_json::Map<String, Value>) -> Option<i64> {
    ["uid", "friendId", "friend_id", "gid"]
        .into_iter()
        .find_map(|key| {
            object
                .get(key)
                .and_then(Value::as_i64)
                .filter(|value| *value > 0)
        })
}

fn native_json_get(
    client: &Client,
    cookie: &str,
    path: &str,
    query: &[(&str, String)],
) -> Result<Value, BossError> {
    native_json_get_with_referer(
        client,
        cookie,
        path,
        query,
        RECRUITER_REFERER,
        MAX_RESPONSE_BYTES,
    )
}

fn native_json_get_with_referer(
    client: &Client,
    cookie: &str,
    path: &str,
    query: &[(&str, String)],
    referer: &str,
    max_bytes: usize,
) -> Result<Value, BossError> {
    wait_for_recruiter_request_slot();
    let response = client
        .get(format!("{BASE_URL}{path}"))
        .headers(headers_with_referer(cookie, referer)?)
        .query(query)
        .send()
        .map_err(|_| transport_error("native Zhipin request failed"))?;
    parse_native_json_response_bounded(response, max_bytes)
}

fn native_json_post(
    client: &Client,
    cookie: &str,
    path: &str,
    form: &[(&str, String)],
) -> Result<Value, BossError> {
    wait_for_recruiter_request_slot();
    let response = client
        .post(format!("{BASE_URL}{path}"))
        .headers(headers(cookie)?)
        .form(form)
        .send()
        .map_err(|_| transport_error("native Zhipin request failed"))?;
    parse_native_json_response(response)
}

fn parse_native_json_response(response: reqwest::blocking::Response) -> Result<Value, BossError> {
    parse_native_json_response_bounded(response, MAX_RESPONSE_BYTES)
}

fn parse_native_json_response_bounded(
    response: reqwest::blocking::Response,
    max_bytes: usize,
) -> Result<Value, BossError> {
    let status = response.status();
    if !status.is_success() {
        return Err(BossError::Http {
            status: status.as_u16(),
            message: "native Zhipin request was rejected".to_owned(),
        });
    }
    let bytes = read_bounded_response(response, max_bytes)?;
    let payload: Value = serde_json::from_slice(&bytes)
        .map_err(|_| transport_error("native Zhipin response was not valid JSON"))?;
    let code = payload
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| transport_error("native Zhipin response omitted API code"))?;
    if matches!(code, 9 | 36 | 37) {
        return Err(transport_error(&format!(
            "Zhipin request stopped for risk-control API code {code}; request was not retried"
        )));
    }
    if code != 0 {
        return Err(transport_error(&format!(
            "Zhipin request failed with API code {code}"
        )));
    }
    Ok(payload)
}

fn wait_for_recruiter_request_slot() {
    let initial = Instant::now()
        .checked_sub(RECRUITER_REQUEST_MIN_INTERVAL + RECRUITER_REQUEST_MAX_JITTER)
        .unwrap_or_else(Instant::now);
    let last_request = LAST_RECRUITER_REQUEST.get_or_init(|| Mutex::new(initial));
    let mut last_request = last_request
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let interval = RECRUITER_REQUEST_MIN_INTERVAL + request_jitter();
    let next_allowed = *last_request + interval;
    if let Some(wait) = next_allowed.checked_duration_since(Instant::now()) {
        std::thread::sleep(wait);
    }
    *last_request = Instant::now();
}

fn request_jitter() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(nanos % (RECRUITER_REQUEST_MAX_JITTER.as_millis() as u64 + 1))
}

fn read_bounded_response(
    response: reqwest::blocking::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, BossError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(transport_error(
            "native Zhipin response exceeded the safe output budget",
        ));
    }
    let mut reader = response.take((max_bytes + 1) as u64);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|_| transport_error("native Zhipin response could not be read"))?;
    if bytes.len() > max_bytes {
        return Err(transport_error(
            "native Zhipin response exceeded the safe output budget",
        ));
    }
    Ok(bytes)
}

#[derive(Debug)]
struct RecruiterFriend {
    security_id: String,
    encrypt_uid: String,
    encrypt_job_id: String,
    encrypt_expect_id: Option<String>,
    friend_source: i64,
}

fn find_recruiter_friend(
    client: &Client,
    cookie: &str,
    target_uid: i64,
) -> Result<RecruiterFriend, BossError> {
    for page in 1..=MAX_FRIEND_SEARCH_PAGES {
        let payload = recruiter_payload(client, cookie, page)?;
        let items = payload
            .get("zpData")
            .and_then(Value::as_object)
            .ok_or_else(|| transport_error("native recruiter response returned no result data"))
            .and_then(recruiter_items)?;
        for item in items {
            let Some(list_object) = item.as_object() else {
                continue;
            };
            if recruiter_uid(list_object) != Some(target_uid) {
                continue;
            }
            let detail_payload = recruiter_friend_detail_payload(client, cookie, &[target_uid])?;
            let detail_data = detail_payload
                .get("zpData")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    transport_error("native recruiter detail returned no result data")
                })?;
            let object = recruiter_items(detail_data)?
                .iter()
                .filter_map(Value::as_object)
                .find(|object| recruiter_uid(object) == Some(target_uid))
                .unwrap_or(list_object);
            let security_id = object
                .get("securityId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    transport_error("native recruiter conversation has no security id")
                })?;
            let encrypt_uid = object
                .get("encryptUid")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    transport_error("native recruiter conversation has no encrypted uid")
                })?;
            let encrypt_job_id = object
                .get("encryptJobId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    transport_error("native recruiter conversation has no encrypted job id")
                })?;
            let encrypt_expect_id = object
                .get("encryptExpectId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let friend_source = object
                .get("friendSource")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            return Ok(RecruiterFriend {
                security_id: security_id.to_owned(),
                encrypt_uid: encrypt_uid.to_owned(),
                encrypt_job_id: encrypt_job_id.to_owned(),
                encrypt_expect_id,
                friend_source,
            });
        }
        if items.is_empty() {
            break;
        }
    }
    Err(BossError::InvalidArgument(
        "recruiter reply uid was not found in the exact friend list".to_owned(),
    ))
}

fn recruiter_history(
    client: &Client,
    cookie: &str,
    uid: i64,
    security_id: &str,
) -> Result<Vec<Value>, BossError> {
    let payload = native_json_get(
        client,
        cookie,
        RECRUITER_HISTORY_PATH,
        &[
            ("gid", uid.to_string()),
            ("securityId", security_id.to_owned()),
            ("page", "1".to_owned()),
            ("c", "20".to_owned()),
            ("src", "0".to_owned()),
        ],
    )?;
    let data = payload
        .get("zpData")
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native recruiter history returned no result data"))?;
    let messages = data
        .get("messages")
        .or_else(|| data.get("historyMsgList"))
        .and_then(Value::as_array)
        .ok_or_else(|| transport_error("native recruiter history returned invalid messages"))?;
    Ok(messages.clone())
}

fn current_user_id(client: &Client, cookie: &str) -> Result<i64, BossError> {
    let payload = native_json_get(client, cookie, USER_INFO_PATH, &[])?;
    payload
        .get("zpData")
        .and_then(Value::as_object)
        .and_then(|data| data.get("userId"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| transport_error("native recruiter user identity was invalid"))
}

fn current_user_token(client: &Client, cookie: &str) -> Result<String, BossError> {
    let payload = native_json_get(client, cookie, USER_INFO_PATH, &[])?;
    payload
        .get("zpData")
        .and_then(Value::as_object)
        .and_then(|data| data.get("token"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| transport_error("native recruiter user token was invalid"))
}

fn history_message_text(message: &Value) -> Option<&str> {
    message.get("text").and_then(Value::as_str).or_else(|| {
        message
            .get("body")
            .and_then(Value::as_object)
            .and_then(|body| body.get("text"))
            .and_then(Value::as_str)
    })
}

fn history_participant_uid(message: &Value, field: &str) -> Option<i64> {
    message
        .get(field)
        .and_then(Value::as_object)
        .and_then(|participant| participant.get("uid"))
        .and_then(Value::as_i64)
}

fn has_outgoing_text(messages: &[Value], user_id: i64, target_uid: i64, expected: &str) -> bool {
    messages.iter().any(|message| {
        history_participant_uid(message, "from") == Some(user_id)
            && history_participant_uid(message, "to") == Some(target_uid)
            && history_message_text(message) == Some(expected)
    })
}

fn bounded_text(value: &str, max: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(max)
        .collect()
}

fn mask_contact_markup(value: &str) -> String {
    mask_contact_markup_bounded(value, MAX_HISTORY_TEXT_CHARS)
}

fn mask_contact_markup_bounded(value: &str, max_chars: usize) -> String {
    let mut output = value.to_owned();
    for tag in ["phone", "copy"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if let (Some(start), Some(end)) = (output.find(&open), output.find(&close))
            && end > start
        {
            let after = end + close.len();
            output.replace_range(start..after, &format!("{open}[redacted]{close}"));
        }
    }
    let mut redacted = String::with_capacity(output.len());
    let mut digits = String::new();
    for character in output.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }
        if !digits.is_empty() {
            if digits.len() >= 7 {
                redacted.push_str("[redacted]");
            } else {
                redacted.push_str(&digits);
            }
            digits.clear();
        }
        redacted.push(character);
    }
    if !digits.is_empty() {
        if digits.len() >= 7 {
            redacted.push_str("[redacted]");
        } else {
            redacted.push_str(&digits);
        }
    }
    let redacted = redacted
        .split_inclusive(char::is_whitespace)
        .map(|part| {
            let trimmed = part.trim_end_matches(char::is_whitespace);
            if trimmed.contains('@') && trimmed.rsplit_once('.').is_some() {
                format!("[redacted]{}", &part[trimmed.len()..])
            } else {
                part.to_owned()
            }
        })
        .collect::<String>();
    redacted
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(max_chars)
        .collect()
}

fn normalize_message(message: &str) -> Result<String, BossError> {
    crate::zhipin_direct::normalize_message(message)
}

fn now_millis() -> Result<i64, BossError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .map_err(|_| transport_error("system clock is before the Unix epoch"))
}

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn encode_varint_field(field: u32, value: u64, output: &mut Vec<u8>) {
    encode_varint((field as u64) << 3, output);
    encode_varint(value, output);
}

fn encode_bytes_field(field: u32, value: &[u8], output: &mut Vec<u8>) {
    encode_varint(((field as u64) << 3) | 2, output);
    encode_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn encode_text_field(field: u32, value: &str, output: &mut Vec<u8>) {
    encode_bytes_field(field, value.as_bytes(), output);
}

fn encode_user(uid: i64, name: Option<&str>, source: i64) -> Vec<u8> {
    let mut output = Vec::new();
    encode_varint_field(1, uid as u64, &mut output);
    if let Some(name) = name.filter(|value| !value.is_empty()) {
        encode_text_field(2, name, &mut output);
    }
    encode_varint_field(7, source as u64, &mut output);
    output
}

fn encode_protocol(
    user_id: i64,
    target_uid: i64,
    target_name: &str,
    target_source: i64,
    message: &str,
    timestamp_ms: i64,
) -> Result<Vec<u8>, BossError> {
    let message_id = user_id
        .checked_add(timestamp_ms)
        .ok_or_else(|| transport_error("native recruiter message identifier overflowed"))?;
    let mut body = Vec::new();
    encode_varint_field(1, 1, &mut body);
    encode_varint_field(2, 1, &mut body);
    encode_text_field(3, message, &mut body);

    let mut chat_message = Vec::new();
    encode_bytes_field(1, &encode_user(user_id, None, 0), &mut chat_message);
    encode_bytes_field(
        2,
        &encode_user(target_uid, Some(target_name), target_source),
        &mut chat_message,
    );
    encode_varint_field(3, 1, &mut chat_message);
    encode_varint_field(4, message_id as u64, &mut chat_message);
    encode_varint_field(5, timestamp_ms as u64, &mut chat_message);
    encode_bytes_field(6, &body, &mut chat_message);
    encode_varint_field(11, message_id as u64, &mut chat_message);
    encode_varint_field(20, 1, &mut chat_message);

    let mut payload = Vec::new();
    encode_varint_field(1, 1, &mut payload);
    encode_bytes_field(3, &chat_message, &mut payload);
    Ok(payload)
}

fn publish_mqtt(cookie: &str, token: &str, wt2: &str, payload: Vec<u8>) -> Result<(), BossError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cookie = cookie.to_owned();
    let wt2 = wt2.to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| transport_error("native recruiter MQTT runtime could not start"))?;
    runtime.block_on(async move {
        let mut request = "wss://ws.zhipin.com/chatws"
            .into_client_request()
            .map_err(|_| transport_error("native recruiter WebSocket request was invalid"))?;
        request.headers_mut().insert(
            "Cookie",
            WsHeaderValue::from_str(&cookie)
                .map_err(|_| transport_error("native recruiter Cookie header was invalid"))?,
        );
        request.headers_mut().insert(
            "Origin",
            WsHeaderValue::from_static("https://www.zhipin.com"),
        );
        request
            .headers_mut()
            .insert("User-Agent", WsHeaderValue::from_static(USER_AGENT_VALUE));
        request
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", WsHeaderValue::from_static("mqtt"));
        let (mut socket, _) = connect_async(request).await.map_err(|error| {
            transport_error(&format!(
                "native recruiter WebSocket connection failed: {error}"
            ))
        })?;
        let client_id = format!("ws-{:016x}", now_millis().unwrap_or_default());
        socket
            .send(Message::Binary(
                mqtt_connect_packet(&client_id, token, &wt2).into(),
            ))
            .await
            .map_err(|_| transport_error("native recruiter MQTT connect failed"))?;
        wait_for_connack(&mut socket).await.map_err(|error| {
            transport_error(&format!(
                "{error}; cookie_bytes={} token_bytes={} wt2_bytes={}",
                cookie.len(),
                token.len(),
                wt2.len()
            ))
        })?;
        socket
            .send(Message::Binary(
                mqtt_publish_packet("chat", &payload).into(),
            ))
            .await
            .map_err(|_| transport_error("native recruiter MQTT publish failed"))?;
        wait_for_puback(&mut socket, 1).await.map_err(|error| {
            transport_error(&format!(
                "{error}; cookie_bytes={} token_bytes={} wt2_bytes={}",
                cookie.len(),
                token.len(),
                wt2.len()
            ))
        })?;
        let _ = socket.close(None).await;
        Ok::<(), BossError>(())
    })
}

fn mqtt_remaining_length(mut length: usize, output: &mut Vec<u8>) {
    loop {
        let mut encoded = (length % 128) as u8;
        length /= 128;
        if length > 0 {
            encoded |= 128;
        }
        output.push(encoded);
        if length == 0 {
            break;
        }
    }
}

fn mqtt_utf8(value: &str, output: &mut Vec<u8>) {
    output.extend_from_slice(&(value.len() as u16).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

fn mqtt_connect_packet(client_id: &str, token: &str, wt2: &str) -> Vec<u8> {
    let mut body = Vec::new();
    mqtt_utf8("MQTT", &mut body);
    body.push(4);
    body.push(0b1100_0010);
    body.extend_from_slice(&30_u16.to_be_bytes());
    mqtt_utf8(client_id, &mut body);
    mqtt_utf8(token, &mut body);
    mqtt_utf8(wt2, &mut body);
    let mut packet = vec![0x10];
    mqtt_remaining_length(body.len(), &mut packet);
    packet.extend_from_slice(&body);
    packet
}

fn mqtt_publish_packet(topic: &str, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    mqtt_utf8(topic, &mut body);
    body.extend_from_slice(&1_u16.to_be_bytes());
    body.extend_from_slice(payload);
    let mut packet = vec![0x32];
    mqtt_remaining_length(body.len(), &mut packet);
    packet.extend_from_slice(&body);
    packet
}

async fn wait_for_connack<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<(), BossError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let deadline = tokio::time::sleep(SEND_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return Err(transport_error("native recruiter MQTT acknowledgement timed out")),
            frame = socket.next() => {
                let Some(frame) = frame else { return Err(transport_error("native recruiter MQTT socket closed")); };
                let frame = frame.map_err(|_| transport_error("native recruiter MQTT frame failed"))?;
                if let Message::Close(details) = &frame {
                    return Err(transport_error(&format!("native recruiter MQTT socket closed: {details:?}")));
                }
                let Message::Binary(bytes) = frame else { continue };
                if bytes.len() >= 4 && bytes[0] >> 4 == 2 && bytes[3] == 0 {
                    return Ok(());
                }
            }
        }
    }
}

async fn wait_for_puback<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    packet_id: u16,
) -> Result<(), BossError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let deadline = tokio::time::sleep(SEND_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return Err(transport_error("native recruiter MQTT acknowledgement timed out")),
            frame = socket.next() => {
                let Some(frame) = frame else { return Err(transport_error("native recruiter MQTT socket closed")); };
                let frame = frame.map_err(|_| transport_error("native recruiter MQTT frame failed"))?;
                if let Message::Close(details) = &frame {
                    return Err(transport_error(&format!("native recruiter MQTT socket closed: {details:?}")));
                }
                let Message::Binary(bytes) = frame else { continue };
                if bytes.len() >= 4 && bytes[0] >> 4 == 4 {
                    let acknowledged = u16::from_be_bytes([bytes[2], bytes[3]]);
                    if acknowledged == packet_id {
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn validate_bounds(limit: usize, page: usize) -> Result<(), BossError> {
    if !(1..=20).contains(&limit) || !(1..=50).contains(&page) {
        return Err(BossError::InvalidArgument(
            "recruiter replies limit must be 1..=20 and page must be 1..=50".to_owned(),
        ));
    }
    Ok(())
}

fn parse_recruiter_response(
    bytes: &[u8],
    cookie: &str,
    limit: usize,
) -> Result<RecruiterReplies, BossError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(transport_error(
            "native recruiter response exceeded the safe output budget",
        ));
    }
    let payload: Value = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("native recruiter response was not valid JSON"))?;
    let code = payload
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| transport_error("native recruiter response omitted API code"))?;
    if code == 37 {
        return Err(transport_error(
            "Zhipin security challenge requires native token support; recruiter request was not retried",
        ));
    }
    if code != 0 {
        return Err(transport_error(&format!(
            "Zhipin recruiter friend list failed with API code {code}"
        )));
    }
    let data = payload
        .get("zpData")
        .and_then(Value::as_object)
        .ok_or_else(|| transport_error("native recruiter response returned no result data"))?;
    let items = recruiter_items(data)?;
    let mut records = Vec::with_capacity(items.len().min(limit));
    for item in items.iter().take(limit) {
        let object = item.as_object().ok_or_else(|| {
            transport_error("native recruiter response returned invalid result entries")
        })?;
        let direction = recruiter_direction(object);
        records.push(RecruiterReplyRecord {
            pending: direction == "candidate_to_recruiter",
            direction: direction.to_owned(),
        });
    }
    Ok(RecruiterReplies {
        cookie: cookie.to_owned(),
        verification: "recruiter_friend_list_api_code_0".to_owned(),
        records,
    })
}

fn recruiter_direction(item: &serde_json::Map<String, Value>) -> &'static str {
    if item.get("isFromGeek") == Some(&Value::Bool(true)) {
        return "candidate_to_recruiter";
    }
    if item
        .get("lastMessage")
        .and_then(Value::as_object)
        .and_then(|message| message.get("fromGeek"))
        == Some(&Value::Bool(true))
    {
        return "candidate_to_recruiter";
    }
    "unknown"
}

fn transport_error(message: &str) -> BossError {
    BossError::Authentication(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preflight_candidate(have_chatted: Option<bool>) -> RecruiterCandidateRecord {
        RecruiterCandidateRecord {
            uid: Some("42".to_owned()),
            encrypt_uid: Some("geek".to_owned()),
            security_id: Some("security".to_owned()),
            encrypt_job_id: Some("job".to_owned()),
            expect_id: Some("expect".to_owned()),
            lid: Some("lid".to_owned()),
            have_chatted,
            name: "Candidate".to_owned(),
            age: String::new(),
            birth_year: None,
            degree: "本科".to_owned(),
            work_years: String::new(),
            expected_positions: Vec::new(),
            summary: String::new(),
            projects: Vec::new(),
        }
    }

    fn preflight_page(candidate: RecruiterCandidateRecord) -> RecruiterCandidatePage {
        RecruiterCandidatePage {
            records: vec![candidate],
            has_more: false,
        }
    }

    #[test]
    fn parses_only_bounded_and_identifier_free_recruiter_records() {
        let payload = br#"{"code":0,"zpData":{"friendList":[{"isFromGeek":true,"uid":"private"},{"lastMessage":{"fromGeek":false}}]}}"#;
        let result = parse_recruiter_response(payload, "wt2=secret", 2).expect("records");
        assert_eq!(result.verification, "recruiter_friend_list_api_code_0");
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.records[0].direction, "candidate_to_recruiter");
        assert_eq!(result.records[1].direction, "unknown");
        assert_eq!(result.cookie, "wt2=secret");
    }

    #[test]
    fn recruiter_result_falls_back_when_friend_list_is_null() {
        let payload = br#"{"code":0,"zpData":{"friendList":null,"result":[{"isFromGeek":true}]}}"#;
        let result = parse_recruiter_response(payload, "wt2=secret", 1).expect("records");
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].direction, "candidate_to_recruiter");
    }

    #[test]
    fn recruiter_items_accepts_list_alias_and_empty_terminal_pages() {
        let list = serde_json::json!({"list":[{"uid":42}]});
        let list_data = list.as_object().expect("object");
        assert_eq!(recruiter_items(list_data).expect("list").len(), 1);
        let friend_id = serde_json::json!({"friendId":43});
        assert_eq!(
            recruiter_uid(friend_id.as_object().expect("object")),
            Some(43)
        );

        let terminal = serde_json::json!({"hasMore":false});
        let terminal_data = terminal.as_object().expect("object");
        assert!(
            recruiter_items(terminal_data)
                .expect("terminal page")
                .is_empty()
        );
    }

    #[test]
    fn recruiter_greeting_requires_all_card_identifiers() {
        assert!(
            validate_recruiter_greet_identifiers(
                "encrypted-geek",
                "security",
                "encrypted-job",
                "expect",
                "lid"
            )
            .is_ok()
        );
        assert!(
            validate_recruiter_greet_identifiers("候选人", "security", "job", "expect", "lid")
                .is_err()
        );
        assert!(validate_recruiter_greet_identifiers("geek", "", "job", "expect", "lid").is_err());
        assert!(
            validate_recruiter_greet_identifiers("geek", "security", "job", "", "lid").is_err()
        );
    }

    #[test]
    fn recruiter_greeting_response_distinguishes_acceptance_and_rejection() {
        assert_eq!(
            parse_recruiter_greet_write_response(br#"{"code":0,"zpData":{"status":1}}"#)
                .expect("accepted"),
            RecruiterGreetWriteOutcome::Accepted
        );
        assert_eq!(
            parse_recruiter_greet_write_response(br#"{"code":5}"#).expect("rejected"),
            RecruiterGreetWriteOutcome::Rejected
        );
    }

    #[test]
    fn recruiter_greeting_api_acceptance_is_not_delivery_verification() {
        let response = parse_recruiter_greet_write_response(br#"{"code":0,"zpData":{"status":1}}"#)
            .expect("accepted");
        assert_eq!(response, RecruiterGreetWriteOutcome::Accepted);
        let result = RecruiterGreetResult {
            state: "api_accepted".to_owned(),
            verification: "chat_start_api_status_1".to_owned(),
        };
        assert_eq!(result.state, "api_accepted");
        assert!(!result.verification.contains("verified"));
    }

    #[test]
    fn recruiter_greeting_ambiguous_response_is_unknown() {
        for body in [
            b"not-json".as_slice(),
            br#"{}"#.as_slice(),
            br#"{"code":"0"}"#.as_slice(),
        ] {
            assert_eq!(
                parse_recruiter_greet_write_response(body).expect("unknown outcome"),
                RecruiterGreetWriteOutcome::Unknown
            );
        }
    }

    #[test]
    fn recruiter_greeting_auth_and_rate_limit_statuses_are_hard_stops() {
        for status in [401, 403, 429] {
            assert!(recruiter_greet_hard_stop_status(status));
        }
        for status in [400, 404, 500, 503] {
            assert!(!recruiter_greet_hard_stop_status(status));
        }
    }

    #[test]
    fn recruiter_greeting_risk_codes_are_hard_stops() {
        for code in [9, 36, 37] {
            let body = format!(r#"{{"code":{code}}}"#);
            let error = parse_recruiter_greet_write_response(body.as_bytes())
                .expect_err("risk code must stop");
            assert!(error.to_string().contains("risk-control"));
        }
    }

    #[test]
    fn recruiter_greeting_form_matches_chat_start_contract() {
        assert_eq!(
            recruiter_greet_form("geek", "job", "expect", "lid", "hello", "security"),
            [
                ("gid", "geek".to_owned()),
                ("suid", String::new()),
                ("jid", "job".to_owned()),
                ("expectId", "expect".to_owned()),
                ("lid", "lid".to_owned()),
                ("greet", "hello".to_owned()),
                ("from", String::new()),
                ("securityId", "security".to_owned()),
            ]
        );
    }

    #[test]
    fn recruiter_greeting_preflight_reads_before_one_exact_write() {
        let events = std::cell::RefCell::new(Vec::new());
        let result = recruiter_greet_after_preflight(
            || {
                events.borrow_mut().push("get");
                Ok(preflight_page(preflight_candidate(Some(false))))
            },
            || {
                events.borrow_mut().push("post");
                Ok(RecruiterGreetWriteOutcome::Accepted)
            },
            "geek",
            "security",
            "job",
            "expect",
            "lid",
        )
        .expect("accepted greeting");
        assert_eq!(*events.borrow(), ["get", "post"]);
        assert_eq!(result.state, "api_accepted");
    }

    #[test]
    fn recruiter_greeting_preflight_blocks_ineligible_missing_or_malformed_candidates() {
        for candidate in [
            preflight_candidate(Some(true)),
            preflight_candidate(None),
            RecruiterCandidateRecord {
                encrypt_uid: Some("other".to_owned()),
                ..preflight_candidate(Some(false))
            },
            RecruiterCandidateRecord {
                security_id: Some("other-security".to_owned()),
                ..preflight_candidate(Some(false))
            },
        ] {
            let mut writes = 0;
            let result = recruiter_greet_after_preflight(
                || Ok(preflight_page(candidate)),
                || {
                    writes += 1;
                    Ok(RecruiterGreetWriteOutcome::Accepted)
                },
                "geek",
                "security",
                "job",
                "expect",
                "lid",
            );
            assert!(result.is_err());
            assert_eq!(writes, 0);
        }
    }

    #[test]
    fn recruiter_greeting_preflight_read_error_prevents_write() {
        let mut writes = 0;
        let result = recruiter_greet_after_preflight(
            || Err(transport_error("preflight read failed")),
            || {
                writes += 1;
                Ok(RecruiterGreetWriteOutcome::Accepted)
            },
            "geek",
            "security",
            "job",
            "expect",
            "lid",
        );
        assert!(result.is_err());
        assert_eq!(writes, 0);
    }

    #[test]
    fn rejects_challenge_and_oversized_or_malformed_payloads() {
        for payload in [
            br#"{"code":37,"zpData":{"seed":"private"}}"#.as_slice(),
            br#"{"code":0}"#.as_slice(),
            br#"{"code":0,"zpData":{"friendList":["private"]}}"#.as_slice(),
        ] {
            assert!(parse_recruiter_response(payload, "wt2=secret", 1).is_err());
        }
        assert!(
            parse_recruiter_response(&vec![b'x'; MAX_RESPONSE_BYTES + 1], "wt2=secret", 1).is_err()
        );
    }

    #[test]
    fn native_request_bounds_match_cli_contract() {
        assert!(validate_bounds(1, 1).is_ok());
        assert!(validate_bounds(20, 50).is_ok());
        assert!(validate_bounds(0, 1).is_err());
        assert!(validate_bounds(1, 51).is_err());
    }

    #[test]
    fn recruiter_request_pacing_has_a_conservative_jittered_gap() {
        assert!(RECRUITER_REQUEST_MIN_INTERVAL >= Duration::from_secs(1));
        assert!(RECRUITER_REQUEST_MAX_JITTER <= Duration::from_secs(2));
        for _ in 0..32 {
            let interval = RECRUITER_REQUEST_MIN_INTERVAL + request_jitter();
            assert!(interval >= RECRUITER_REQUEST_MIN_INTERVAL);
            assert!(interval <= RECRUITER_REQUEST_MIN_INTERVAL + RECRUITER_REQUEST_MAX_JITTER);
        }
    }

    #[test]
    fn history_text_redacts_markup_phone_and_email_contacts() {
        let text = "电话 <phone>13800138000</phone> 邮箱 candidate@example.com";
        assert_eq!(
            mask_contact_markup(text),
            "电话 <phone>[redacted]</phone> 邮箱 [redacted]"
        );
    }

    #[test]
    fn mqtt_publish_uses_qos_one_without_retain_and_fixed_packet_id() {
        let packet = mqtt_publish_packet("chat", b"payload");
        assert_eq!(packet[0], 0x32);
        assert_eq!(&packet[2..8], b"\0\x04chat");
        assert_eq!(&packet[8..10], &[0, 1]);
        assert_eq!(&packet[10..], b"payload");
    }

    #[test]
    fn outgoing_history_matches_body_or_top_level_text_with_exact_participants() {
        let body = serde_json::json!({
            "from":{"uid":7},"to":{"uid":9},"body":{"text":"hello"},
            "quote":{"body":{"text":"earlier"}}
        });
        let top = serde_json::json!({
            "from":{"uid":7},"to":{"uid":9},"text":"hello",
            "quote":{"invalid":true}
        });
        assert!(has_outgoing_text(&[body, top], 7, 9, "hello"));
    }

    #[test]
    fn outgoing_history_rejects_same_text_from_wrong_sender_or_target() {
        let messages = serde_json::json!([
            {"from":{"uid":8},"to":{"uid":9},"text":"hello"},
            {"from":{"uid":7},"to":{"uid":10},"body":{"text":"hello"}},
            {"from":"malformed","to":{"uid":9},"text":"hello"}
        ]);
        assert!(!has_outgoing_text(
            messages.as_array().expect("messages"),
            7,
            9,
            "hello"
        ));
    }

    #[test]
    fn parses_bounded_full_recruiter_resume_and_extracts_github_refs() {
        let payload = serde_json::json!({
            "code": 0,
            "zpData": {
                "geekDetailInfo": {
                    "geekBaseInfo": {
                        "name": "Candidate",
                        "ageDesc": "20岁",
                        "degreeCategory": "本科",
                        "workYearDesc": "应届生",
                        "applyStatusContent": "考虑机会",
                        "userDescription": "项目 https://github.com/example/repo\n电话 13800138000"
                    },
                    "geekExpPosList": [{"positionName":"Python","salaryDesc":"15-30K"}],
                    "geekProjExpList": [{"name":"Demo","roleName":"后端","projectDescription":"Agent 电话 13800138000","url":"mailto:candidate@example.com","startDateDesc":"2024","endDateDesc":"至今"}],
                    "geekWorkExpList": [],
                    "geekEduExpList": [{"school":"Example","major":"安全工程","degreeName":"本科","eduDescription":"协会","startDateDesc":"2023","endDateDesc":"2027"}]
                }
            }
        });
        let resume = parse_recruiter_resume(&payload, 42).expect("resume");
        assert_eq!(resume.uid, "42");
        assert_eq!(resume.expected_positions, vec!["Python"]);
        assert_eq!(resume.github_refs, vec!["github.com/example/repo"]);
        assert!(resume.summary.contains("[redacted]"));
        assert_eq!(resume.projects.len(), 1);
        assert!(resume.projects[0].description.contains("[redacted]"));
        assert_eq!(resume.projects[0].url, "[redacted]");
        assert_eq!(resume.education.len(), 1);
    }

    #[test]
    fn recruiter_resume_rejects_missing_detail() {
        let payload = serde_json::json!({"code":0,"zpData":{}});
        assert!(parse_recruiter_resume(&payload, 42).is_err());
    }

    #[test]
    fn parses_job_scoped_recommendation_cards_without_contact_fields() {
        let payload = serde_json::json!({
            "code": 0,
            "zpData": {
                "page": 1,
                "hasMore": true,
                "geekList": [{
                    "haveChatted": false,
                    "geekCard": {
                        "uid": 42,
                        "encryptGeekId": "encrypted-geek",
                        "name": "S**",
                        "ageDesc": "28岁",
                        "birthYear": 1998,
                        "degreeName": "本科",
                        "workYearDesc": "3年",
                        "securityId": "security",
                        "expectId": "expect",
                        "lid": "lid",
                        "expectPositionName": "视频剪辑",
                        "userDescription": "自学 PR，作品集和项目经历。电话 13800138000",
                        "projectExperience": [{
                            "projectName": "短视频项目",
                            "projectDescription": "负责剪辑和交付"
                        }],
                        "phone": "13800138000"
                    }
                }]
            }
        });
        let page = parse_recruiter_candidates(&payload, 5, "job").expect("candidates");
        assert!(page.has_more);
        assert_eq!(page.records.len(), 1);
        let candidate = &page.records[0];
        assert_eq!(candidate.uid.as_deref(), Some("42"));
        assert_eq!(candidate.encrypt_uid.as_deref(), Some("encrypted-geek"));
        assert_eq!(candidate.security_id.as_deref(), Some("security"));
        assert_eq!(candidate.encrypt_job_id.as_deref(), Some("job"));
        assert_eq!(candidate.expect_id.as_deref(), Some("expect"));
        assert_eq!(candidate.lid.as_deref(), Some("lid"));
        assert_eq!(candidate.have_chatted, Some(false));
        assert_eq!(candidate.degree, "本科");
        assert_eq!(candidate.birth_year, Some(1998));
        assert_eq!(candidate.expected_positions, vec!["视频剪辑"]);
        assert!(candidate.summary.contains("[redacted]"));
        assert!(!candidate.summary.contains("13800138000"));
        assert_eq!(candidate.projects.len(), 1);
    }

    #[test]
    fn recommendation_parser_uses_the_requested_encrypted_job_id() {
        let payload = serde_json::json!({
            "code": 0,
            "zpData": {
                "geekList": [{
                    "geekCard": {
                        "uid": 42,
                        "encryptGeekId": "encrypted-geek",
                        "securityId": "security",
                        "expectId": "expect",
                        "lid": "lid",
                        "name": "Candidate"
                    }
                }]
            }
        });

        let page = parse_recruiter_candidates(&payload, 1, "encrypted-job").expect("candidates");
        assert_eq!(page.records.len(), 1);
        assert_eq!(
            page.records[0].encrypt_job_id.as_deref(),
            Some("encrypted-job")
        );
        assert_eq!(page.records[0].have_chatted, None);
    }

    #[test]
    fn recommendation_parser_preserves_explicit_conversation_eligibility() {
        let payload = serde_json::json!({
            "code": 0,
            "zpData": {"geekList": [
                {"haveChatted": false, "geekCard": {"name": "Eligible"}},
                {"haveChatted": true, "geekCard": {"name": "Existing"}},
                {"haveChatted": "false", "geekCard": {"name": "Malformed"}},
                {"geekCard": {"name": "Missing"}}
            ]}
        });
        let page = parse_recruiter_candidates(&payload, 5, "job").expect("candidates");
        assert_eq!(page.records.len(), 4);
        assert_eq!(page.records[0].have_chatted, Some(false));
        assert_eq!(page.records[1].have_chatted, Some(true));
        assert_eq!(page.records[2].have_chatted, None);
        assert_eq!(page.records[3].have_chatted, None);
    }

    #[test]
    fn candidate_parser_allows_the_bounded_preflight_page() {
        let payload = serde_json::json!({"code":0,"zpData":{"geekList":[]}});
        assert!(parse_recruiter_candidates(&payload, 1, "job").is_ok());
        assert!(parse_recruiter_candidates(&payload, MAX_RECRUITER_INBOX_RECORDS, "job").is_ok());
        assert!(
            parse_recruiter_candidates(&payload, MAX_RECRUITER_INBOX_RECORDS + 1, "job").is_err()
        );
    }

    #[test]
    fn parses_search_card_highest_degree_and_nested_highlights() {
        let payload = serde_json::json!({
            "code": 0,
            "zpData": {"geekList": [{
                "ageDesc": "29岁",
                "highlightExpectName": "视频剪辑",
                "highlightGeekDescName": "自学<em class='h'>剪辑</em>并持续复盘",
                "geekCard": {
                    "name": "L**",
                    "highestDegreeName": "本科",
                    "workYear": "2年",
                    "expect": {"name": "后期剪辑"},
                    "geekDesc": {"name": "作品集和项目经历"},
                    "cardFields": [{
                        "kind": "PROJECT",
                        "text": {"content": "短视频项目：负责交付"}
                    }]
                }
            }]}
        });
        let page = parse_recruiter_candidates(&payload, 5, "job").expect("candidates");
        let candidate = &page.records[0];
        assert_eq!(candidate.degree, "本科");
        assert_eq!(candidate.work_years, "2年");
        assert_eq!(candidate.expected_positions, vec!["视频剪辑"]);
        assert_eq!(candidate.summary, "自学剪辑并持续复盘");
        assert_eq!(candidate.projects, vec!["短视频项目：负责交付"]);
    }

    #[test]
    fn parses_candidate_detail_without_contact_fields() {
        let payload = serde_json::json!({
            "code": 0,
            "zpData": {
                "geekDetail": {
                    "geekBaseInfo": {
                        "ageDesc": "28岁",
                        "degreeCategory": "本科",
                        "workYearDesc": "3年",
                        "userDescription": "自学剪辑并复盘 电话 13800138000"
                    },
                    "geekExpPosList": [{"positionName": "视频剪辑"}],
                    "geekProjExpList": [{
                        "name": "短视频项目",
                        "projectDescription": "负责后期交付"
                    }],
                    "geekWorkExpList": [{
                        "company": "Example",
                        "positionName": "剪辑师",
                        "performance": "完成项目"
                    }],
                    "geekEduExpList": [{
                        "school": "Example大学",
                        "degreeName": "本科"
                    }]
                }
            }
        });
        let detail = parse_recruiter_candidate_detail(&payload).expect("detail");
        assert_eq!(detail.age, "28岁");
        assert_eq!(detail.degree, "本科");
        assert_eq!(detail.expected_positions, vec!["视频剪辑"]);
        assert_eq!(detail.projects.len(), 1);
        assert_eq!(detail.work_experience.len(), 1);
        assert_eq!(detail.education.len(), 1);
        assert!(detail.summary.contains("[redacted]"));
        assert!(!detail.summary.contains("13800138000"));
    }
}
