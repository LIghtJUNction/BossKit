//! Small Rust-native HTTP transport for the recruiter BOSS surface.
//!
//! This module deliberately owns only the fixed recruiter friend-list route.
//! The remaining geek chat flow still depends on the legacy helper while its
//! security challenge and MQTT wire protocol are being replaced.

use futures_util::{SinkExt, StreamExt};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, USER_AGENT};
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
const RECRUITER_FRIEND_LIST_PATH: &str = "/wapi/zprelation/friend/getBossFriendListV2.json";
const RECRUITER_HISTORY_PATH: &str = "/wapi/zpchat/boss/historyMsg";
const RECRUITER_RESUME_PATH: &str = "/wapi/zpboss/h5/geek/detail/get";
const USER_INFO_PATH: &str = "/wapi/zpuser/wap/getUserInfo.json";
const WT_PATH: &str = "/wapi/zppassport/get/wt";
// BOSS returns up to 100 recruiter rows in one page; bound the raw payload
// above that observed size while keeping the CLI response strictly limited.
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const MAX_HISTORY_TEXT_CHARS: usize = 2000;
const MAX_RESUME_DESCRIPTION_CHARS: usize = 16 * 1024;
const MAX_RESUME_ITEMS: usize = 20;
const MAX_RECRUITER_INBOX_RECORDS: usize = 20;
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
// Keep exact resume/reply UID lookup aligned with recruiter inbox pagination.
// The inbox can expose up to 50 pages, so stopping at page five silently made
// later candidates look nonexistent.
const MAX_FRIEND_SEARCH_PAGES: usize = 50;
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
    wait_for_recruiter_request_slot();
    let response = client
        .get(format!("{BASE_URL}{RECRUITER_FRIEND_LIST_PATH}"))
        .headers(headers(cookie)?)
        .query(&[
            ("page", page.to_string()),
            ("status", "0".to_owned()),
            ("jobId", "0".to_owned()),
        ])
        .send()
        .map_err(|_| transport_error("native Zhipin recruiter request failed"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(BossError::Http {
            status: status.as_u16(),
            message: "native Zhipin recruiter request was rejected".to_owned(),
        });
    }
    let bytes = read_bounded_response(response)?;
    parse_recruiter_response(&bytes, cookie, limit)
}

/// Reads recruiter-side conversations and the latest textual message using
/// only the native HTTP client. Identifiers are returned solely so an explicit
/// `recruiter reply` command can target one exact conversation.
pub(crate) fn recruiter_inbox(
    cookie: &str,
    limit: usize,
    page: usize,
) -> Result<Vec<RecruiterInboxRecord>, BossError> {
    if !(1..=MAX_RECRUITER_INBOX_RECORDS).contains(&limit) || !(1..=50).contains(&page) {
        return Err(BossError::InvalidArgument(
            "recruiter inbox limit must be 1..=20 and page must be 1..=50".to_owned(),
        ));
    }
    auth::validate_cookie(cookie)?;
    let cookie = cookie.to_owned();
    std::thread::spawn(move || recruiter_inbox_blocking(&cookie, limit, page))
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

fn recruiter_inbox_blocking(
    cookie: &str,
    limit: usize,
    page: usize,
) -> Result<Vec<RecruiterInboxRecord>, BossError> {
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
    let items = data
        .get("friendList")
        .or_else(|| data.get("result"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            transport_error("native recruiter response returned invalid result entries")
        })?;
    let user_id = current_user_id(&client, cookie)?;
    let mut records = Vec::with_capacity(items.len().min(limit));
    for item in items.iter().take(limit) {
        let object = item.as_object().ok_or_else(|| {
            transport_error("native recruiter response returned invalid result entries")
        })?;
        let uid = object
            .get("uid")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or_else(|| transport_error("native recruiter conversation has invalid uid"))?;
        let security_id = object
            .get("securityId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| transport_error("native recruiter conversation has no security id"))?;
        let messages = recruiter_history(&client, cookie, uid, security_id)?;
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
                    .unwrap_or("候选人"),
                128,
            ),
            job: bounded_text(
                object.get("jobName").and_then(Value::as_str).unwrap_or(""),
                256,
            ),
            pending: last_direction == "candidate_to_recruiter",
            last_direction,
            last_message,
        });
    }
    Ok(records)
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
    if has_outgoing_text(&before, user_id, &message) {
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
    publish_mqtt(cookie, &user_token, wt2, payload)?;
    for attempt in 0..3 {
        let after = recruiter_history(&client, cookie, target_uid, &friend.security_id)?;
        if has_outgoing_text(&after, user_id, &message) {
            return Ok(RecruiterSendResult {
                state: "message_verified".to_owned(),
                verification: "exact_outgoing_text_in_recruiter_history".to_owned(),
            });
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    Err(transport_error(
        "native recruiter message could not be verified",
    ))
}

fn headers(cookie: &str) -> Result<HeaderMap, BossError> {
    let mut map = HeaderMap::new();
    map.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
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
    native_json_get(
        client,
        cookie,
        RECRUITER_FRIEND_LIST_PATH,
        &[
            ("page", page.to_string()),
            ("status", "0".to_owned()),
            ("jobId", "0".to_owned()),
        ],
    )
}

fn native_json_get(
    client: &Client,
    cookie: &str,
    path: &str,
    query: &[(&str, String)],
) -> Result<Value, BossError> {
    wait_for_recruiter_request_slot();
    let response = client
        .get(format!("{BASE_URL}{path}"))
        .headers(headers(cookie)?)
        .query(query)
        .send()
        .map_err(|_| transport_error("native Zhipin request failed"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(BossError::Http {
            status: status.as_u16(),
            message: "native Zhipin request was rejected".to_owned(),
        });
    }
    let bytes = read_bounded_response(response)?;
    let payload: Value = serde_json::from_slice(&bytes)
        .map_err(|_| transport_error("native Zhipin response was not valid JSON"))?;
    let code = payload
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| transport_error("native Zhipin response omitted API code"))?;
    if code == 37 {
        return Err(transport_error(
            "Zhipin security challenge requires native token support; API code 37 request was not retried",
        ));
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

fn read_bounded_response(response: reqwest::blocking::Response) -> Result<Vec<u8>, BossError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(transport_error(
            "native Zhipin response exceeded the safe output budget",
        ));
    }
    let mut reader = response.take((MAX_RESPONSE_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|_| transport_error("native Zhipin response could not be read"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
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
            .and_then(|data| data.get("friendList").or_else(|| data.get("result")))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                transport_error("native recruiter response returned invalid result entries")
            })?;
        for item in items {
            let Some(object) = item.as_object() else {
                continue;
            };
            if object.get("uid").and_then(Value::as_i64) != Some(target_uid) {
                continue;
            }
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

fn has_outgoing_text(messages: &[Value], user_id: i64, expected: &str) -> bool {
    messages.iter().any(|message| {
        message
            .get("from")
            .and_then(Value::as_object)
            .and_then(|from| from.get("uid"))
            .and_then(Value::as_i64)
            == Some(user_id)
            && message
                .get("body")
                .and_then(Value::as_object)
                .and_then(|body| body.get("text"))
                .and_then(Value::as_str)
                == Some(expected)
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
    let normalized = message.trim();
    if normalized.is_empty()
        || normalized.chars().count() > 200
        || normalized.chars().any(|character| {
            character.is_control()
                || character == '\u{2028}'
                || character == '\u{2029}'
                || (character.is_whitespace() && character != ' ')
        })
    {
        return Err(BossError::InvalidArgument(
            "recruiter reply must contain 1 to 200 printable single-line characters".to_owned(),
        ));
    }
    Ok(normalized.to_owned())
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
    let items = data
        .get("friendList")
        .or_else(|| data.get("result"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            transport_error("native recruiter response returned invalid result entries")
        })?;
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
}
