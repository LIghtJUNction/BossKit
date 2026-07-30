//! Browserless BOSS Zhipin transport executed through a bounded Python helper.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wait_timeout::ChildExt;

use crate::BossError;

const HELPER: &str = include_str!("../scripts/zhipin_transport.py");
const HELPER_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_HELPER_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_CHAT_MESSAGE_CHARS: usize = 200;
pub(crate) const MAX_CHAT_HISTORY_MESSAGES: usize = 20;
const MAX_CHAT_HISTORY_TEXT_CHARS: usize = 2000;
const MAX_CHAT_HISTORY_RESPONSE_BYTES: usize = 60 * 1024;
pub(crate) const MAX_CHAT_INBOX_CONVERSATIONS: usize = 5;
const MAX_CHAT_INBOX_TEXT_CHARS: usize = 512;
const MAX_CHAT_INBOX_RESPONSE_BYTES: usize = 60 * 1024;
const MAX_ZHIPIN_REMOTE_ID_CHARS: usize = 2048;
const MAX_RESUME_SCALAR_CHARS: usize = 128;
const MAX_RESUME_SUMMARY_CHARS: usize = 2000;
const MAX_RESUME_EXPECTATIONS: usize = 10;
const MAX_RESUME_SECTION_ITEMS: usize = 8;
const MAX_RESUME_CONTENT_CHARS: usize = 32 * 1024;
const MAX_RESUME_RESPONSE_BYTES: usize = 60 * 1024;
const MAX_RECRUITER_REPLIES_RESPONSE_BYTES: usize = 32 * 1024;

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    action: &'static str,
    cookie: &'a str,
}

#[derive(Debug, Serialize)]
struct GreetRequest<'a> {
    action: &'static str,
    cookie: &'a str,
    title: &'a str,
    remote_id: &'a str,
}

#[derive(Debug, Serialize)]
struct SendRequest<'a> {
    action: &'static str,
    cookie: &'a str,
    remote_id: &'a str,
    message: &'a str,
}

#[derive(Debug, Serialize)]
struct HistoryRequest<'a> {
    action: &'static str,
    cookie: &'a str,
    remote_id: &'a str,
    limit: usize,
}

#[derive(Debug, Serialize)]
struct InboxRequest<'a> {
    action: &'static str,
    cookie: &'a str,
    remote_ids: &'a [&'a str],
}

#[derive(Debug, Serialize)]
struct ResumeShowRequest<'a> {
    action: &'static str,
    cookie: &'a str,
}

#[derive(Debug, Serialize)]
struct RecruiterRepliesRequest<'a> {
    action: &'static str,
    cookie: &'a str,
    limit: usize,
    page: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectRecruiterReply {
    pub(crate) direction: String,
    pub(crate) pending: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectChatMessage {
    pub(crate) direction: String,
    pub(crate) text: String,
    pub(crate) timestamp_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectInboxLatest {
    pub(crate) direction: String,
    pub(crate) text: String,
    pub(crate) timestamp_ms: u64,
    pub(crate) truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperInboxConversation {
    remote_id: String,
    latest: Option<DirectInboxLatest>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectExpectedRole {
    pub(crate) position: String,
    pub(crate) city: String,
    pub(crate) salary: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectResumeSectionCounts {
    pub(crate) basic_info: usize,
    pub(crate) expectations: usize,
    pub(crate) personal_summary: usize,
    pub(crate) work_experience: usize,
    pub(crate) project_experience: usize,
    pub(crate) education_experience: usize,
    pub(crate) certifications: usize,
    pub(crate) volunteer_experience: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectResumeSnapshot {
    pub(crate) display_name: String,
    pub(crate) work_years: String,
    pub(crate) education: String,
    pub(crate) job_status: String,
    pub(crate) expected_roles: Vec<DirectExpectedRole>,
    pub(crate) personal_summary: String,
    pub(crate) content: String,
    pub(crate) section_counts: DirectResumeSectionCounts,
    pub(crate) truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperResponse {
    ok: bool,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    verification: Option<String>,
    #[serde(default)]
    updated_cookie: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    count: Option<usize>,
    #[serde(default)]
    messages: Option<Vec<DirectChatMessage>>,
    #[serde(default)]
    conversations: Option<Vec<HelperInboxConversation>>,
    #[serde(default)]
    snapshot: Option<DirectResumeSnapshot>,
    #[serde(default)]
    replies: Option<Vec<DirectRecruiterReply>>,
}

/// A verified session refresh whose Cookie remains private to the service.
#[derive(Debug)]
pub(crate) struct DirectSessionRefresh {
    pub(crate) cookie: String,
    pub(crate) verification: String,
}

/// A verified initial-contact result whose refreshed Cookie remains private.
#[derive(Debug)]
pub(crate) struct DirectGreeting {
    pub(crate) cookie: String,
    pub(crate) state: String,
    pub(crate) verification: String,
}

/// A history-verified direct message whose refreshed Cookie remains private.
#[derive(Debug)]
pub(crate) struct DirectMessage {
    pub(crate) cookie: String,
    pub(crate) state: String,
    pub(crate) verification: String,
}

/// A bounded, sanitized snapshot of one exact direct conversation.
#[derive(Debug)]
pub(crate) struct DirectHistory {
    pub(crate) cookie: String,
    pub(crate) verification: String,
    pub(crate) messages: Vec<DirectChatMessage>,
}

/// Latest safe text for a bounded ordered set of exact conversations.
#[derive(Debug)]
pub(crate) struct DirectInbox {
    pub(crate) cookie: String,
    pub(crate) verification: String,
    pub(crate) conversations: Vec<Option<DirectInboxLatest>>,
}

/// A bounded online resume snapshot whose refreshed Cookie remains private.
#[derive(Debug)]
pub(crate) struct DirectOnlineResume {
    pub(crate) cookie: String,
    pub(crate) verification: String,
    pub(crate) snapshot: DirectResumeSnapshot,
}

/// A bounded, identifier-free recruiter reply result.
#[derive(Debug)]
pub(crate) struct DirectRecruiterReplies {
    pub(crate) cookie: String,
    pub(crate) verification: String,
    pub(crate) records: Vec<DirectRecruiterReply>,
}

/// Refreshes and verifies one stored Zhipin Cookie without a browser process.
pub(crate) fn refresh_session(cookie: &str) -> Result<DirectSessionRefresh, BossError> {
    let request = serde_json::to_vec(&RefreshRequest {
        action: "refresh",
        cookie,
    })
    .map_err(|_| transport_error("unable to encode direct transport request"))?;
    parse_refresh_response(&invoke_helper(&request)?)
}

/// Refreshes a recruiter session using only the recruiter friend-list route.
pub(crate) fn refresh_recruiter_session(cookie: &str) -> Result<DirectSessionRefresh, BossError> {
    let response = match crate::zhipin_http::recruiter_replies(cookie, 1, 1) {
        Ok(response) => response,
        Err(error) if is_recruiter_challenge(&error) => {
            let request = serde_json::to_vec(&RefreshRequest {
                action: "recruiter_refresh",
                cookie,
            })
            .map_err(|_| transport_error("unable to encode recruiter refresh request"))?;
            return parse_recruiter_refresh_response(&invoke_helper(&request)?);
        }
        Err(error) => return Err(error),
    };
    Ok(DirectSessionRefresh {
        cookie: response.cookie,
        verification: response.verification,
    })
}

/// Reads bounded recruiter reply states without candidate search or chat APIs.
pub(crate) fn recruiter_replies(
    cookie: &str,
    limit: usize,
    page: usize,
) -> Result<DirectRecruiterReplies, BossError> {
    if !(1..=20).contains(&limit) || !(1..=50).contains(&page) {
        return Err(BossError::InvalidArgument(
            "recruiter replies limit must be 1..=20 and page must be 1..=50".to_owned(),
        ));
    }
    let response = match crate::zhipin_http::recruiter_replies(cookie, limit, page) {
        Ok(response) => response,
        Err(error) if is_recruiter_challenge(&error) => {
            let request = serde_json::to_vec(&RecruiterRepliesRequest {
                action: "recruiter_replies",
                cookie,
                limit,
                page,
            })
            .map_err(|_| transport_error("unable to encode recruiter replies request"))?;
            return parse_recruiter_replies_response(&invoke_helper(&request)?, limit);
        }
        Err(error) => return Err(error),
    };
    Ok(DirectRecruiterReplies {
        cookie: response.cookie,
        verification: response.verification,
        records: response
            .records
            .into_iter()
            .map(|record| DirectRecruiterReply {
                direction: record.direction,
                pending: record.pending,
            })
            .collect(),
    })
}

/// Establishes and verifies one default Zhipin conversation without custom text.
pub(crate) fn greet(
    cookie: &str,
    title: &str,
    remote_id: &str,
) -> Result<DirectGreeting, BossError> {
    let request = serde_json::to_vec(&GreetRequest {
        action: "greet",
        cookie,
        title,
        remote_id,
    })
    .map_err(|_| transport_error("unable to encode direct transport request"))?;
    parse_greet_response(&invoke_helper(&request)?)
}

/// Normalizes one bounded printable chat message without including it in errors.
pub(crate) fn normalize_message(message: &str) -> Result<String, BossError> {
    let normalized = message.trim();
    let char_count = normalized.chars().count();
    if char_count == 0 || char_count > MAX_CHAT_MESSAGE_CHARS {
        return Err(BossError::InvalidArgument(
            "chat message must contain 1 to 200 characters".to_owned(),
        ));
    }
    if normalized.chars().any(|character| {
        character.is_control()
            || character == '\u{2028}'
            || character == '\u{2029}'
            || (character.is_whitespace() && character != ' ')
    }) {
        return Err(BossError::InvalidArgument(
            "chat message must be printable and single-line".to_owned(),
        ));
    }
    Ok(normalized.to_owned())
}

/// Sends one custom message to an existing exact Zhipin conversation.
pub(crate) fn send(
    cookie: &str,
    remote_id: &str,
    message: &str,
) -> Result<DirectMessage, BossError> {
    let request = serde_json::to_vec(&SendRequest {
        action: "send",
        cookie,
        remote_id,
        message,
    })
    .map_err(|_| transport_error("unable to encode direct transport request"))?;
    parse_send_response(&invoke_helper(&request)?)
}

/// Reads a bounded text snapshot from one existing exact Zhipin conversation.
pub(crate) fn history(
    cookie: &str,
    remote_id: &str,
    limit: usize,
) -> Result<DirectHistory, BossError> {
    if !(1..=MAX_CHAT_HISTORY_MESSAGES).contains(&limit) {
        return Err(BossError::InvalidArgument(
            "chat history limit must be between 1 and 20".to_owned(),
        ));
    }
    let request = serde_json::to_vec(&HistoryRequest {
        action: "history",
        cookie,
        remote_id,
        limit,
    })
    .map_err(|_| transport_error("unable to encode direct transport request"))?;
    parse_history_response(&invoke_helper(&request)?)
}

/// Reads the latest safe text for up to five existing exact conversations.
pub(crate) fn inbox(cookie: &str, remote_ids: &[&str]) -> Result<DirectInbox, BossError> {
    validate_inbox_remote_ids(remote_ids)?;
    let request = serde_json::to_vec(&InboxRequest {
        action: "inbox",
        cookie,
        remote_ids,
    })
    .map_err(|_| transport_error("unable to encode direct transport request"))?;
    parse_inbox_response(&invoke_helper(&request)?, remote_ids)
}

/// Reads the current BOSS Zhipin online resume without modifying it.
pub(crate) fn resume_show(cookie: &str) -> Result<DirectOnlineResume, BossError> {
    let request = serde_json::to_vec(&ResumeShowRequest {
        action: "resume_show",
        cookie,
    })
    .map_err(|_| transport_error("unable to encode direct transport request"))?;
    parse_resume_show_response(&invoke_helper(&request)?)
}

fn invoke_helper(request: &[u8]) -> Result<Vec<u8>, BossError> {
    let mut child = Command::new("uv")
        .args([
            "run",
            "--quiet",
            "--with",
            "iv8==0.1.4",
            "--with",
            "requests==2.34.2",
            "--with",
            "paho-mqtt==2.1.0",
            "python",
            "-c",
            HELPER,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| transport_error("uv is required for browserless Zhipin authentication"))?;

    child
        .stdin
        .take()
        .ok_or_else(|| transport_error("unable to open direct transport input"))?
        .write_all(request)
        .map_err(|_| transport_error("unable to write direct transport input"))?;

    if child
        .wait_timeout(HELPER_TIMEOUT)
        .map_err(|_| transport_error("unable to wait for direct transport"))?
        .is_none()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(transport_error("direct Zhipin transport timed out"));
    }

    let output = child
        .wait_with_output()
        .map_err(|_| transport_error("unable to read direct transport result"))?;
    if output.stdout.len() > MAX_HELPER_OUTPUT_BYTES {
        return Err(transport_error("direct transport result was too large"));
    }
    Ok(output.stdout)
}

fn parse_refresh_response(bytes: &[u8]) -> Result<DirectSessionRefresh, BossError> {
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok {
        return Err(transport_error(
            response
                .error
                .as_deref()
                .unwrap_or("direct Zhipin session refresh failed"),
        ));
    }
    if response.action.as_deref() != Some("refresh")
        || response.state.is_some()
        || response.count.is_some()
        || response.messages.is_some()
        || response.conversations.is_some()
        || response.snapshot.is_some()
        || response.replies.is_some()
    {
        return Err(transport_error("direct transport action mismatch"));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let verification = response
        .verification
        .filter(|value| {
            matches!(
                value.as_str(),
                "authenticated_api_code_0"
                    | "security_token_refreshed_and_authenticated_api_code_0"
            )
        })
        .ok_or_else(|| transport_error("direct transport verification was invalid"))?;
    Ok(DirectSessionRefresh {
        cookie,
        verification,
    })
}

fn is_recruiter_challenge(error: &BossError) -> bool {
    matches!(error, BossError::Authentication(message) if message.contains("API code 37"))
}

fn parse_recruiter_refresh_response(bytes: &[u8]) -> Result<DirectSessionRefresh, BossError> {
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok || response.action.as_deref() != Some("recruiter_refresh") {
        return Err(transport_error(
            response
                .error
                .as_deref()
                .unwrap_or("recruiter session refresh failed"),
        ));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let verification = response
        .verification
        .filter(|value| {
            matches!(
                value.as_str(),
                "recruiter_friend_list_api_code_0"
                    | "security_token_refreshed_and_recruiter_friend_list_api_code_0"
            )
        })
        .ok_or_else(|| transport_error("recruiter transport verification was invalid"))?;
    Ok(DirectSessionRefresh {
        cookie,
        verification,
    })
}

fn parse_recruiter_replies_response(
    bytes: &[u8],
    limit: usize,
) -> Result<DirectRecruiterReplies, BossError> {
    if bytes.len() > MAX_RECRUITER_REPLIES_RESPONSE_BYTES {
        return Err(transport_error(
            "recruiter replies result exceeded the safe output budget",
        ));
    }
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok || response.action.as_deref() != Some("recruiter_replies") {
        return Err(transport_error(
            response
                .error
                .as_deref()
                .unwrap_or("recruiter replies failed"),
        ));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let verification = response
        .verification
        .filter(|value| value == "recruiter_friend_list_api_code_0")
        .ok_or_else(|| transport_error("recruiter transport verification was invalid"))?;
    let records = response
        .replies
        .filter(|records| {
            records.len() <= limit
                && records.iter().all(|record| {
                    matches!(
                        record.direction.as_str(),
                        "candidate_to_recruiter" | "unknown"
                    ) && (record.pending == (record.direction == "candidate_to_recruiter"))
                })
        })
        .ok_or_else(|| transport_error("recruiter replies were invalid"))?;
    if response.count != Some(records.len()) {
        return Err(transport_error("recruiter reply count was invalid"));
    }
    Ok(DirectRecruiterReplies {
        cookie,
        verification,
        records,
    })
}

fn parse_greet_response(bytes: &[u8]) -> Result<DirectGreeting, BossError> {
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok {
        return Err(transport_error(
            response
                .error
                .as_deref()
                .unwrap_or("direct Zhipin greeting failed"),
        ));
    }
    if response.action.as_deref() != Some("greet")
        || response.count.is_some()
        || response.messages.is_some()
        || response.conversations.is_some()
        || response.snapshot.is_some()
        || response.replies.is_some()
    {
        return Err(transport_error("direct transport action mismatch"));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let state = response
        .state
        .filter(|value| matches!(value.as_str(), "already_connected" | "greeting_verified"))
        .ok_or_else(|| transport_error("direct greeting state was invalid"))?;
    let verification = response
        .verification
        .filter(|value| value == "exact_encrypt_job_id_in_friend_list")
        .ok_or_else(|| transport_error("direct greeting verification was invalid"))?;
    Ok(DirectGreeting {
        cookie,
        state,
        verification,
    })
}

fn parse_send_response(bytes: &[u8]) -> Result<DirectMessage, BossError> {
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok {
        return Err(transport_error(
            response
                .error
                .as_deref()
                .unwrap_or("direct Zhipin message failed"),
        ));
    }
    if response.action.as_deref() != Some("send")
        || response.count.is_some()
        || response.messages.is_some()
        || response.conversations.is_some()
        || response.snapshot.is_some()
        || response.replies.is_some()
    {
        return Err(transport_error("direct transport action mismatch"));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let state = response
        .state
        .filter(|value| matches!(value.as_str(), "already_sent" | "message_verified"))
        .ok_or_else(|| transport_error("direct message state was invalid"))?;
    let verification = response
        .verification
        .filter(|value| value == "exact_outgoing_text_in_history")
        .ok_or_else(|| transport_error("direct message verification was invalid"))?;
    Ok(DirectMessage {
        cookie,
        state,
        verification,
    })
}

fn parse_history_response(bytes: &[u8]) -> Result<DirectHistory, BossError> {
    if bytes.len() > MAX_CHAT_HISTORY_RESPONSE_BYTES {
        return Err(transport_error(
            "direct history result exceeded the safe output budget",
        ));
    }
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok {
        return Err(transport_error(
            response
                .error
                .as_deref()
                .unwrap_or("direct Zhipin history failed"),
        ));
    }
    if response.action.as_deref() != Some("history")
        || response.state.is_some()
        || response.conversations.is_some()
        || response.snapshot.is_some()
        || response.replies.is_some()
    {
        return Err(transport_error("direct transport action mismatch"));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let verification = response
        .verification
        .filter(|value| value == "exact_encrypt_job_id_and_user_id")
        .ok_or_else(|| transport_error("direct history verification was invalid"))?;
    let messages = response
        .messages
        .filter(|messages| {
            messages.len() <= MAX_CHAT_HISTORY_MESSAGES
                && messages.iter().all(|message| {
                    matches!(message.direction.as_str(), "incoming" | "outgoing")
                        && !message.text.is_empty()
                        && message.text.chars().count() <= MAX_CHAT_HISTORY_TEXT_CHARS
                        && message.text.chars().all(is_safe_history_character)
                        && message.timestamp_ms > 0
                })
                && messages
                    .windows(2)
                    .all(|pair| pair[0].timestamp_ms <= pair[1].timestamp_ms)
        })
        .ok_or_else(|| transport_error("direct history messages were invalid"))?;
    if response.count != Some(messages.len()) {
        return Err(transport_error("direct history count was invalid"));
    }
    Ok(DirectHistory {
        cookie,
        verification,
        messages,
    })
}

fn validate_inbox_remote_ids(remote_ids: &[&str]) -> Result<(), BossError> {
    if !(1..=MAX_CHAT_INBOX_CONVERSATIONS).contains(&remote_ids.len()) {
        return Err(BossError::InvalidArgument(
            "chat inbox requires between 1 and 5 jobs".to_owned(),
        ));
    }
    let mut unique = std::collections::HashSet::with_capacity(remote_ids.len());
    if remote_ids.iter().any(|remote_id| {
        remote_id.is_empty()
            || remote_id.chars().count() > MAX_ZHIPIN_REMOTE_ID_CHARS
            || !unique.insert(*remote_id)
    }) {
        return Err(BossError::InvalidArgument(
            "chat inbox requires unique valid Zhipin job identifiers".to_owned(),
        ));
    }
    Ok(())
}

fn parse_inbox_response(bytes: &[u8], remote_ids: &[&str]) -> Result<DirectInbox, BossError> {
    if bytes.len() > MAX_CHAT_INBOX_RESPONSE_BYTES {
        return Err(transport_error(
            "direct inbox result exceeded the safe output budget",
        ));
    }
    validate_inbox_remote_ids(remote_ids)?;
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok {
        return Err(transport_error(
            response
                .error
                .as_deref()
                .unwrap_or("direct Zhipin inbox failed"),
        ));
    }
    if response.action.as_deref() != Some("inbox")
        || response.state.is_some()
        || response.messages.is_some()
        || response.error.is_some()
        || response.snapshot.is_some()
        || response.replies.is_some()
    {
        return Err(transport_error("direct transport action mismatch"));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let verification = response
        .verification
        .filter(|value| value == "exact_encrypt_job_ids_and_user_id")
        .ok_or_else(|| transport_error("direct inbox verification was invalid"))?;
    let conversations = response
        .conversations
        .filter(|items| items.len() == remote_ids.len())
        .ok_or_else(|| transport_error("direct inbox conversations were invalid"))?;
    if response.count != Some(conversations.len())
        || conversations
            .iter()
            .zip(remote_ids)
            .any(|(conversation, expected)| {
                conversation.remote_id != *expected
                    || conversation.latest.as_ref().is_some_and(|latest| {
                        !matches!(latest.direction.as_str(), "incoming" | "outgoing")
                            || latest.text.is_empty()
                            || latest.text.chars().count() > MAX_CHAT_INBOX_TEXT_CHARS
                            || latest
                                .text
                                .chars()
                                .any(|character| !is_safe_history_character(character))
                            || latest.timestamp_ms == 0
                            || (latest.truncated
                                && latest.text.chars().count() != MAX_CHAT_INBOX_TEXT_CHARS)
                    })
            })
    {
        return Err(transport_error("direct inbox conversations were invalid"));
    }
    Ok(DirectInbox {
        cookie,
        verification,
        conversations: conversations
            .into_iter()
            .map(|conversation| conversation.latest)
            .collect(),
    })
}

fn parse_resume_show_response(bytes: &[u8]) -> Result<DirectOnlineResume, BossError> {
    if bytes.len() > MAX_RESUME_RESPONSE_BYTES {
        return Err(transport_error(
            "direct resume result exceeded the safe output budget",
        ));
    }
    let response: HelperResponse = serde_json::from_slice(bytes)
        .map_err(|_| transport_error("direct transport returned an invalid result"))?;
    if !response.ok {
        return Err(transport_error("direct Zhipin resume preview failed"));
    }
    if response.action.as_deref() != Some("resume_show")
        || response.state.is_some()
        || response.error.is_some()
        || response.count.is_some()
        || response.messages.is_some()
        || response.conversations.is_some()
    {
        return Err(transport_error("direct transport action mismatch"));
    }
    let cookie = response
        .updated_cookie
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error("direct transport returned no session"))?;
    crate::auth::validate_cookie(&cookie)?;
    let verification = response
        .verification
        .filter(|value| value == "resume_preview_api_code_0")
        .ok_or_else(|| transport_error("direct resume verification was invalid"))?;
    let snapshot = response
        .snapshot
        .filter(valid_resume_snapshot)
        .ok_or_else(|| transport_error("direct resume snapshot was invalid"))?;
    Ok(DirectOnlineResume {
        cookie,
        verification,
        snapshot,
    })
}

fn valid_resume_snapshot(snapshot: &DirectResumeSnapshot) -> bool {
    let scalar = |value: &str| valid_resume_text(value, MAX_RESUME_SCALAR_CHARS, false);
    !snapshot.display_name.is_empty()
        && scalar(&snapshot.display_name)
        && scalar(&snapshot.work_years)
        && scalar(&snapshot.education)
        && scalar(&snapshot.job_status)
        && snapshot.expected_roles.len() <= MAX_RESUME_EXPECTATIONS
        && snapshot.expected_roles.iter().all(|role| {
            !role.position.is_empty()
                && scalar(&role.position)
                && scalar(&role.city)
                && scalar(&role.salary)
        })
        && valid_resume_text(&snapshot.personal_summary, MAX_RESUME_SUMMARY_CHARS, false)
        && !snapshot.content.is_empty()
        && valid_resume_text(&snapshot.content, MAX_RESUME_CONTENT_CHARS, true)
        && [
            "[Basic info]",
            "[Expectations]",
            "[Personal summary]",
            "[Work experience]",
            "[Project experience]",
            "[Education experience]",
            "[Certifications]",
            "[Volunteer experience]",
        ]
        .iter()
        .all(|heading| snapshot.content.contains(heading))
        && (1..=4).contains(&snapshot.section_counts.basic_info)
        && snapshot.section_counts.expectations == snapshot.expected_roles.len()
        && snapshot.section_counts.personal_summary
            == usize::from(!snapshot.personal_summary.is_empty())
        && snapshot.section_counts.work_experience <= MAX_RESUME_SECTION_ITEMS
        && snapshot.section_counts.project_experience <= MAX_RESUME_SECTION_ITEMS
        && snapshot.section_counts.education_experience <= MAX_RESUME_SECTION_ITEMS
        && snapshot.section_counts.certifications <= MAX_RESUME_SECTION_ITEMS
        && snapshot.section_counts.volunteer_experience <= MAX_RESUME_SECTION_ITEMS
}

fn valid_resume_text(value: &str, maximum: usize, allow_newline: bool) -> bool {
    value.chars().count() <= maximum
        && value.chars().all(|character| {
            (allow_newline && character == '\n') || is_safe_history_character(character)
        })
        && !value.contains('@')
        && !contains_uri_scheme(value)
        && !contains_bare_domain(value)
        && !contains_phone_like(value)
}

fn contains_phone_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    (0..bytes.len()).any(|start| {
        if !(bytes[start].is_ascii_digit() || bytes[start] == b'+')
            || (start > 0
                && (bytes[start - 1].is_ascii_alphanumeric()
                    || matches!(bytes[start - 1], b'_' | b'+')))
        {
            return false;
        }
        let mut index = start + usize::from(bytes[start] == b'+');
        let mut digits = 0;
        let mut last_digit_end = index;
        while index < bytes.len() {
            if bytes[index].is_ascii_digit() {
                digits += 1;
                last_digit_end = index + 1;
            } else if !matches!(bytes[index], b' ' | b'-' | b'.' | b'(' | b')') {
                break;
            }
            index += 1;
        }
        (7..=15).contains(&digits)
            && (last_digit_end == bytes.len()
                || !(bytes[last_digit_end].is_ascii_alphanumeric()
                    || bytes[last_digit_end] == b'_'))
    })
}

fn contains_uri_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    (0..bytes.len()).any(|start| {
        if !bytes[start].is_ascii_alphabetic()
            || (start > 0
                && matches!(
                    bytes[start - 1],
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'-' | b'.'
                ))
        {
            return false;
        }
        let mut index = start + 1;
        while index < bytes.len()
            && matches!(
                bytes[index],
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        {
            index += 1;
        }
        index + 1 < bytes.len() && bytes[index] == b':' && !bytes[index + 1].is_ascii_whitespace()
    })
}

fn contains_bare_domain(value: &str) -> bool {
    value
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
        })
        .any(|token| {
            let token = token.trim_matches('.');
            let Some((prefix, suffix)) = token.rsplit_once('.') else {
                return false;
            };
            suffix.len() >= 2
                && suffix.bytes().all(|byte| byte.is_ascii_alphabetic())
                && prefix.split('.').all(valid_domain_label)
        })
}

fn valid_domain_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && label
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn is_safe_history_character(character: char) -> bool {
    let codepoint = u32::from(character);
    !character.is_control()
        && codepoint != 0x00AD
        && !(0x0600..=0x0605).contains(&codepoint)
        && !matches!(codepoint, 0x061C | 0x06DD | 0x070F | 0x08E2 | 0x180E)
        && !(0x0890..=0x0891).contains(&codepoint)
        && !(0x200B..=0x200F).contains(&codepoint)
        && !(0x2028..=0x202E).contains(&codepoint)
        && !(0x2060..=0x206F).contains(&codepoint)
        && codepoint != 0xFEFF
        && !(0xFFF9..=0xFFFB).contains(&codepoint)
        && !matches!(codepoint, 0x110BD | 0x110CD | 0xE0001)
        && !(0x13430..=0x1343F).contains(&codepoint)
        && !(0x1BCA0..=0x1BCA3).contains(&codepoint)
        && !(0x1D173..=0x1D17A).contains(&codepoint)
        && !(0xE0020..=0xE007F).contains(&codepoint)
}

fn transport_error(message: &str) -> BossError {
    BossError::Authentication(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_pure_helper(assertion: &str) -> String {
        let runner = format!(
            "import sys\nscope={{'__name__':'zhipin_transport'}}\nexec(sys.stdin.read(),scope)\n{assertion}\n"
        );
        let mut child = Command::new("python")
            .args(["-c", &runner])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("python");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(HELPER.as_bytes())
            .expect("helper source");
        let output = child.wait_with_output().expect("python output");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_owned()
    }

    fn valid_resume_payload() -> serde_json::Value {
        serde_json::json!({
            "ok":true,
            "action":"resume_show",
            "verification":"resume_preview_api_code_0",
            "updated_cookie":"wt2=secret; __zp_stoken__=fresh",
            "snapshot":{
                "display_name":"Candidate",
                "work_years":"5 years",
                "education":"Bachelor",
                "job_status":"Open",
                "expected_roles":[
                    {"position":"Rust Engineer","city":"Shenzhen","salary":"20-30K"}
                ],
                "personal_summary":"Builds reliable systems",
                "content":"[Basic info]\n- Display name: Candidate\n\n[Expectations]\n- Position: Rust Engineer\n\n[Personal summary]\n- Builds reliable systems\n\n[Work experience]\n(none)\n\n[Project experience]\n(none)\n\n[Education experience]\n(none)\n\n[Certifications]\n(none)\n\n[Volunteer experience]\n(none)",
                "section_counts":{
                    "basic_info":4,
                    "expectations":1,
                    "personal_summary":1,
                    "work_experience":0,
                    "project_experience":0,
                    "education_experience":0,
                    "certifications":0,
                    "volunteer_experience":0
                },
                "truncated":false
            }
        })
    }

    #[test]
    fn parses_verified_refresh_without_exposing_cookie_in_debug_fields() {
        let result = parse_refresh_response(
            br#"{"ok":true,"action":"refresh","verification":"authenticated_api_code_0","updated_cookie":"wt2=secret; __zp_stoken__=fresh"}"#,
        )
        .expect("refresh");
        assert_eq!(result.verification, "authenticated_api_code_0");
        assert!(result.cookie.contains("__zp_stoken__"));
    }

    #[test]
    fn rejects_unverified_or_ambiguous_success() {
        for payload in [
            br#"{"ok":true,"action":"refresh","verification":"publish_ack","updated_cookie":"wt2=secret"}"#
                .as_slice(),
            br#"{"ok":true,"action":"greet","verification":"authenticated_api_code_0","updated_cookie":"wt2=secret"}"#
                .as_slice(),
            br#"{"ok":true,"action":"refresh","verification":"authenticated_api_code_0"}"#.as_slice(),
        ] {
            assert!(parse_refresh_response(payload).is_err());
        }
    }

    #[test]
    fn helper_failure_does_not_echo_credential_fields() {
        let error =
            parse_refresh_response(br#"{"ok":false,"error":"stored Zhipin Cookie is invalid"}"#)
                .expect_err("failure");
        let rendered = error.to_string();
        assert!(!rendered.contains("wt2="));
        assert!(!rendered.contains("__zp_stoken__="));
    }

    #[test]
    fn resume_show_request_and_verified_snapshot_are_strictly_typed() {
        let request = serde_json::to_value(ResumeShowRequest {
            action: "resume_show",
            cookie: "wt2=secret",
        })
        .expect("request");
        assert_eq!(
            request,
            serde_json::json!({"action":"resume_show","cookie":"wt2=secret"})
        );

        let payload = serde_json::to_vec(&valid_resume_payload()).expect("payload");
        let result = parse_resume_show_response(&payload).expect("resume");
        assert_eq!(result.verification, "resume_preview_api_code_0");
        assert_eq!(result.snapshot.expected_roles.len(), 1);
        assert_eq!(result.snapshot.display_name, "Candidate");
    }

    #[test]
    fn resume_show_parser_rejects_unknown_oversized_unsafe_and_private_fields() {
        let mut invalid = Vec::new();

        let mut unknown = valid_resume_payload();
        unknown["snapshot"]["phone"] = serde_json::json!("13800000000");
        invalid.push(unknown);

        let mut oversized = valid_resume_payload();
        oversized["snapshot"]["display_name"] =
            serde_json::json!("x".repeat(MAX_RESUME_SCALAR_CHARS + 1));
        invalid.push(oversized);

        let mut unsafe_text = valid_resume_payload();
        unsafe_text["snapshot"]["personal_summary"] = serde_json::json!("\u{202e}private");
        invalid.push(unsafe_text);

        let mut email = valid_resume_payload();
        email["snapshot"]["personal_summary"] = serde_json::json!("candidate@example.test");
        invalid.push(email);

        let mut handle = valid_resume_payload();
        handle["snapshot"]["personal_summary"] = serde_json::json!("Built tools with @platform");
        invalid.push(handle);

        let mut attachment = valid_resume_payload();
        attachment["snapshot"]["content"] = serde_json::json!(
            "[Basic info]\n[Expectations]\n[Personal summary]\n[Work experience]\n[Project experience]\n[Education experience]\n[Certifications]\n[Volunteer experience]\nhttps://example.test/resume.pdf"
        );
        invalid.push(attachment);

        for private_text in [
            "Call +86 138-0000-0000",
            "Download ftp://example.test/resume",
            "Contact mailto:candidate@example.test",
            "Profile candidate.example.test",
        ] {
            let mut adversarial = valid_resume_payload();
            adversarial["snapshot"]["personal_summary"] = serde_json::json!(private_text);
            invalid.push(adversarial);
        }

        for payload in invalid {
            let encoded = serde_json::to_vec(&payload).expect("payload");
            assert!(parse_resume_show_response(&encoded).is_err());
        }
    }

    #[test]
    fn resume_show_failure_never_echoes_helper_credentials() {
        let error = parse_resume_show_response(
            br#"{"ok":false,"error":"wt2=PRIVATE_COOKIE; token=PRIVATE_TOKEN"}"#,
        )
        .expect_err("failure");
        let rendered = error.to_string();
        assert!(!rendered.contains("PRIVATE_COOKIE"));
        assert!(!rendered.contains("PRIVATE_TOKEN"));
    }

    #[test]
    fn parses_only_exact_verified_greeting_states() {
        for state in ["already_connected", "greeting_verified"] {
            let payload = format!(
                "{{\"ok\":true,\"action\":\"greet\",\"state\":\"{state}\",\"verification\":\"exact_encrypt_job_id_in_friend_list\",\"updated_cookie\":\"wt2=secret\"}}"
            );
            let result = parse_greet_response(payload.as_bytes()).expect("verified greeting");
            assert_eq!(result.state, state);
        }
    }

    #[test]
    fn rejects_unknown_or_incomplete_greeting_results() {
        for payload in [
            br#"{"ok":true,"action":"greet","state":"sent","verification":"exact_encrypt_job_id_in_friend_list","updated_cookie":"wt2=secret"}"#.as_slice(),
            br#"{"ok":true,"action":"greet","state":"greeting_verified","verification":"api_code_0","updated_cookie":"wt2=secret"}"#.as_slice(),
            br#"{"ok":true,"action":"greet","state":"greeting_verified","verification":"exact_encrypt_job_id_in_friend_list","updated_cookie":"wt2=secret","boss":"private"}"#.as_slice(),
        ] {
            assert!(parse_greet_response(payload).is_err());
        }
    }

    #[test]
    fn message_validation_trims_and_accepts_unicode_with_spaces() {
        assert_eq!(
            normalize_message("  你好，想聊聊这个职位 🙂  ").expect("message"),
            "你好，想聊聊这个职位 🙂"
        );
    }

    #[test]
    fn message_validation_rejects_empty_multiline_control_and_oversized_text() {
        assert!(normalize_message(" ").is_err());
        for message in ["private\nline", "private\ttext", &"界".repeat(201)] {
            let error = normalize_message(message).expect_err("invalid message");
            assert!(!error.to_string().contains(message));
        }
    }

    #[test]
    fn parses_only_history_verified_message_states() {
        for state in ["already_sent", "message_verified"] {
            let payload = format!(
                "{{\"ok\":true,\"action\":\"send\",\"state\":\"{state}\",\"verification\":\"exact_outgoing_text_in_history\",\"updated_cookie\":\"wt2=secret\"}}"
            );
            let result = parse_send_response(payload.as_bytes()).expect("verified message");
            assert_eq!(result.state, state);
        }
    }

    #[test]
    fn send_parser_rejects_unknown_or_unverified_results() {
        for payload in [
            br#"{"ok":true,"action":"send","state":"published","verification":"exact_outgoing_text_in_history","updated_cookie":"wt2=secret"}"#.as_slice(),
            br#"{"ok":true,"action":"send","state":"message_verified","verification":"puback","updated_cookie":"wt2=secret"}"#.as_slice(),
            br#"{"ok":true,"action":"send","state":"message_verified","verification":"exact_outgoing_text_in_history","updated_cookie":"wt2=secret","message":"private"}"#.as_slice(),
        ] {
            assert!(parse_send_response(payload).is_err());
        }
    }

    #[test]
    fn parses_only_bounded_chronological_history() {
        let result = parse_history_response(
            br#"{"ok":true,"action":"history","verification":"exact_encrypt_job_id_and_user_id","updated_cookie":"wt2=secret","count":2,"messages":[{"direction":"incoming","text":"\u4f60\u597d","timestamp_ms":10},{"direction":"outgoing","text":"\u60a8\u597d","timestamp_ms":20}]}"#,
        )
        .expect("verified history");
        assert_eq!(result.verification, "exact_encrypt_job_id_and_user_id");
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.messages[0].direction, "incoming");
        assert_eq!(result.messages[1].text, "您好");
    }

    #[test]
    fn history_parser_rejects_unverified_or_ambiguous_results() {
        for payload in [
            br#"{"ok":true,"action":"history","verification":"friend_list","updated_cookie":"wt2=secret","count":0,"messages":[]}"#.as_slice(),
            br#"{"ok":true,"action":"history","verification":"exact_encrypt_job_id_and_user_id","updated_cookie":"wt2=secret","count":2,"messages":[]}"#.as_slice(),
            br#"{"ok":true,"action":"history","verification":"exact_encrypt_job_id_and_user_id","updated_cookie":"wt2=secret","count":1,"messages":[{"direction":"unknown","text":"private","timestamp_ms":10}]}"#.as_slice(),
            br#"{"ok":true,"action":"history","verification":"exact_encrypt_job_id_and_user_id","updated_cookie":"wt2=secret","count":1,"messages":[{"direction":"incoming","text":"\u202eprivate","timestamp_ms":10}]}"#.as_slice(),
            br#"{"ok":true,"action":"history","verification":"exact_encrypt_job_id_and_user_id","updated_cookie":"wt2=secret","count":2,"messages":[{"direction":"incoming","text":"later","timestamp_ms":20},{"direction":"outgoing","text":"earlier","timestamp_ms":10}]}"#.as_slice(),
            br#"{"ok":true,"action":"history","verification":"exact_encrypt_job_id_and_user_id","updated_cookie":"wt2=secret","state":"verified","count":0,"messages":[]}"#.as_slice(),
        ] {
            assert!(parse_history_response(payload).is_err());
        }
    }

    #[test]
    fn parses_ordered_inbox_with_nullable_latest_text() {
        let truncated = "界".repeat(MAX_CHAT_INBOX_TEXT_CHARS);
        let payload = serde_json::to_vec(&serde_json::json!({
            "ok":true,
            "action":"inbox",
            "verification":"exact_encrypt_job_ids_and_user_id",
            "updated_cookie":"wt2=secret",
            "count":2,
            "conversations":[
                {"remote_id":"remote-1","latest":null},
                {"remote_id":"remote-2","latest":{
                    "direction":"incoming",
                    "text":truncated,
                    "timestamp_ms":20,
                    "truncated":true
                }}
            ]
        }))
        .expect("payload");
        let result =
            parse_inbox_response(&payload, &["remote-1", "remote-2"]).expect("verified inbox");
        assert_eq!(result.verification, "exact_encrypt_job_ids_and_user_id");
        assert!(result.conversations[0].is_none());
        let latest = result.conversations[1].as_ref().expect("latest");
        assert_eq!(latest.direction, "incoming");
        assert_eq!(latest.text.chars().count(), MAX_CHAT_INBOX_TEXT_CHARS);
        assert!(latest.truncated);
    }

    #[test]
    fn inbox_parser_rejects_invalid_identity_fields_and_latest_text() {
        for payload in [
            serde_json::json!({
                "ok":true,"action":"inbox",
                "verification":"exact_encrypt_job_ids_and_user_id",
                "updated_cookie":"wt2=secret","count":1,
                "conversations":[{"remote_id":"wrong","latest":null}]
            }),
            serde_json::json!({
                "ok":true,"action":"inbox",
                "verification":"exact_encrypt_job_ids_and_user_id",
                "updated_cookie":"wt2=secret","count":2,
                "conversations":[{"remote_id":"remote-1","latest":null}]
            }),
            serde_json::json!({
                "ok":true,"action":"inbox",
                "verification":"exact_encrypt_job_ids_and_user_id",
                "updated_cookie":"wt2=secret","count":1,"state":"verified",
                "conversations":[{"remote_id":"remote-1","latest":null}]
            }),
            serde_json::json!({
                "ok":true,"action":"inbox",
                "verification":"exact_encrypt_job_ids_and_user_id",
                "updated_cookie":"wt2=secret","count":1,"messages":[],
                "conversations":[{"remote_id":"remote-1","latest":null}]
            }),
            serde_json::json!({
                "ok":true,"action":"inbox",
                "verification":"exact_encrypt_job_ids_and_user_id",
                "updated_cookie":"wt2=secret","count":1,
                "conversations":[{"remote_id":"remote-1","latest":{
                    "direction":"incoming","text":"short",
                    "timestamp_ms":10,"truncated":true
                }}]
            }),
            serde_json::json!({
                "ok":true,"action":"inbox",
                "verification":"exact_encrypt_job_ids_and_user_id",
                "updated_cookie":"wt2=secret","count":1,
                "conversations":[{"remote_id":"remote-1","latest":{
                    "direction":"incoming","text":"\u{202e}private",
                    "timestamp_ms":10,"truncated":false
                }}]
            }),
        ] {
            let encoded = serde_json::to_vec(&payload).expect("payload");
            assert!(parse_inbox_response(&encoded, &["remote-1"]).is_err());
        }
    }

    #[test]
    fn inbox_request_rejects_empty_duplicate_oversized_or_excess_ids() {
        assert!(validate_inbox_remote_ids(&[]).is_err());
        assert!(validate_inbox_remote_ids(&["remote", "remote"]).is_err());
        assert!(validate_inbox_remote_ids(&["", "remote"]).is_err());
        let oversized = "x".repeat(MAX_ZHIPIN_REMOTE_ID_CHARS + 1);
        assert!(validate_inbox_remote_ids(&[oversized.as_str()]).is_err());
        assert!(validate_inbox_remote_ids(&["1", "2", "3", "4", "5", "6"]).is_err());
    }

    #[test]
    fn existing_action_parsers_reject_inbox_only_fields() {
        assert!(parse_refresh_response(
            br#"{"ok":true,"action":"refresh","verification":"authenticated_api_code_0","updated_cookie":"wt2=secret","conversations":[]}"#
        )
        .is_err());
        assert!(parse_history_response(
            br#"{"ok":true,"action":"history","verification":"exact_encrypt_job_id_and_user_id","updated_cookie":"wt2=secret","count":0,"messages":[],"conversations":[]}"#
        )
        .is_err());
    }

    #[test]
    fn python_exact_job_search_retries_one_challenge_and_returns_updated_pairs() {
        let result = run_pure_helper(
            r#"
class FakeResponse:
 def __init__(self,payload):
  self.payload=payload
 def json(self):
  return self.payload
class FakeSession:
 def __init__(self,payloads):
  self.payloads=list(payloads)
  self.calls=[]
 def post(self,url,headers,data,timeout):
  self.calls.append((url,dict(data),timeout))
  return FakeResponse(self.payloads.pop(0))
challenge={'code':37,'zpData':{'seed':'seed','name':'name','ts':1}}
found={'code':0,'zpData':{'jobList':[{'encryptJobId':'target','securityId':'security','lid':'lid'}]}}
challenge_calls=[]
def fake_apply(payload,pairs,session,deadline):
 challenge_calls.append((payload,list(pairs),deadline))
 return list(pairs)+[('__zp_stoken__','fresh')]
scope['apply_challenge']=fake_apply
session=FakeSession([challenge,found])
security,lid,pairs=scope['search_exact_job'](session,[('wt2','secret')],'Engineer','target')
assert (security,lid) == ('security','lid')
assert pairs == [('wt2','secret'),('__zp_stoken__','fresh')]
assert len(challenge_calls) == 1 and len(session.calls) == 2
assert session.calls[0] == session.calls[1]
session=FakeSession([challenge,challenge])
challenge_calls.clear()
try:
 scope['search_exact_job'](session,[('wt2','secret')],'Engineer','target')
 raise AssertionError('accepted repeated challenge')
except scope['SafeFailure']:
 pass
assert len(challenge_calls) == 1 and len(session.calls) == 2
print('bounded')
"#,
        );
        assert_eq!(result, "bounded");
    }

    #[test]
    fn python_resume_show_uses_exact_read_endpoint_and_one_challenge_retry() {
        let result = run_pure_helper(
            r#"
class FakeResponse:
 def __init__(self,payload):
  self.payload=payload
 def json(self):
  return self.payload
class FakeSession:
 def __init__(self,payloads):
  self.payloads=list(payloads)
  self.calls=[]
 def get(self,url,headers,params,timeout):
  self.calls.append((url,dict(headers),dict(params)))
  return FakeResponse(self.payloads.pop(0))
challenge={'code':37,'zpData':{'seed':'seed','name':'name','ts':1}}
preview={'code':0,'zpData':{
 'baseInfo':{'nickName':'Candidate','workYearDesc':'5 years','degreeCategory':'Bachelor','phone':'13800000000'},
 'applyStatusDesc':'Open',
 'expectList':[{'positionName':'Rust Engineer','cityName':'Shenzhen','salaryDesc':'20-30K'}],
 'userDesc':'Contact candidate@example.test or 13800000000 at https://example.test/cv',
 'workExpList':[{'companyName':'Example','positionName':'Engineer','startDate':'2020','endDate':'2024','workContent':'Systems','workPerformance':'Reliable'}],
 'projectExpList':[{'name':'Compiler','roleName':'Lead','startDate':'2022','endDate':'2023','projectDesc':'Built it','performance':'Shipped'}],
 'educationExpList':[{'school':'University','major':'Computer Science','degreeName':'Bachelor','startYear':'2016','endYear':'2020'}],
 'certificationList':[{'certName':'Cloud Certificate'}],
 'volunteerExpList':[{'name':'Community','serviceLength':'20 hours','volunteerDesc':'Mentoring'}]
}}
challenge_calls=[]
resume_token='PRIVATE_BST_TOKEN'
def fake_apply(payload,pairs,session,deadline):
 challenge_calls.append(payload)
 return list(pairs)+[('__zp_stoken__','fresh')]
session=FakeSession([challenge,preview])
scope['prepare_session']=lambda cookie,deadline:(session,[('wt2','secret'),('bst',resume_token)],False)
scope['apply_challenge']=fake_apply
response=scope['resume_show']('wt2=secret')
assert response['action'] == 'resume_show'
assert response['verification'] == 'resume_preview_api_code_0'
assert response['updated_cookie'].endswith('__zp_stoken__=fresh')
snapshot=response['snapshot']
assert snapshot['display_name'] == 'Candidate'
assert snapshot['work_years'] == '5 years'
assert snapshot['education'] == 'Bachelor'
assert snapshot['job_status'] == 'Open'
assert snapshot['personal_summary'].startswith('Contact')
assert snapshot['expected_roles'] == [{'position':'Rust Engineer','city':'Shenzhen','salary':'20-30K'}]
assert snapshot['section_counts'] == {
 'basic_info':4,
 'expectations':1,
 'personal_summary':1,
 'work_experience':1,
 'project_experience':1,
 'education_experience':1,
 'certifications':1,
 'volunteer_experience':1,
}
for expected in ['Reliable','Shipped','University','Cloud Certificate','20 hours','Mentoring']:
 assert expected in snapshot['content']
assert len(challenge_calls) == 1 and len(session.calls) == 2
assert session.calls[0] == session.calls[1]
assert session.calls[0][0] == scope['RESUME_PREVIEW_URL']
assert session.calls[0][1]['Referer'] == scope['RESUME_REFERER']
assert session.calls[0][1]['Zp_token'] == resume_token
assert set(session.calls[0][2]) == {'_'}
assert type(session.calls[0][2]['_']) is int and session.calls[0][2]['_'] > 0
rendered=scope['json'].dumps(snapshot,ensure_ascii=False)
assert 'candidate@example.test' not in rendered
assert '13800000000' not in rendered
assert 'https://example.test/cv' not in rendered
assert resume_token not in rendered
public_response={key:value for key,value in response.items() if key != 'updated_cookie'}
assert resume_token not in scope['json'].dumps(public_response)
assert 'Zp_token' not in response
session=FakeSession([challenge,challenge])
challenge_calls.clear()
scope['prepare_session']=lambda cookie,deadline:(session,[('wt2','secret'),('bst',resume_token)],False)
try:
 scope['resume_show']('wt2=secret')
 raise AssertionError('accepted repeated challenge')
except scope['SafeFailure'] as error:
 assert resume_token not in str(error)
assert len(challenge_calls) == 1 and len(session.calls) == 2
for invalid_pairs in [
 [],
 [('bst','')],
 [('bst','FIRST_PRIVATE'),('bst','SECOND_PRIVATE')],
 [('bst','x'*(scope['MAX_OPAQUE_VALUE_CHARS']+1))],
]:
 try:
  scope['required_cookie_value'](invalid_pairs,'bst','resume token')
  raise AssertionError('accepted invalid resume token')
 except scope['SafeFailure'] as error:
  assert 'PRIVATE' not in str(error) and 'xxxx' not in str(error)
print('bounded')
"#,
        );
        assert_eq!(result, "bounded");
    }

    #[test]
    fn python_resume_text_normalizes_common_whitespace_and_rejects_unsafe_controls() {
        let result = run_pure_helper(
            r#"
snapshot=scope['resume_snapshot']({
 'baseInfo':{'nickName':'Candidate'},
 'workExpList':[{
  'companyName':'Example',
  'workContent':'Line one\nLine two\tTabbed\rFinal'
 }]
})
work_line=next(
 line for line in snapshot['content'].splitlines()
 if 'Company: Example' in line
)
assert 'Summary: Line one Line two Tabbed Final' in work_line
assert '\t' not in work_line and '\r' not in work_line
for unsafe in ['bad\x00text','bad\u202etext']:
 try:
  scope['resume_text'](unsafe,'test',128)
  raise AssertionError('accepted unsafe resume text')
 except scope['SafeFailure']:
  pass
for phone in [
 '0755-12345678',
 '+86 138-0000-0000',
 '13800000000',
 '1234567',
 '12345678',
]:
 text,_=scope['resume_text'](f'Call {phone} now','test',128)
 assert '[redacted-phone]' in text and phone not in text
for adjacent in [
 '电话13800138000微信同号',
 '电话0755-12345678微信同号',
]:
 text,_=scope['resume_text'](adjacent,'test',128)
 assert '[redacted-phone]' in text
 assert not any(character.isdigit() for character in text)
for url in [
 'ftp://example.test/file',
 'mailto:candidate@example.test',
 'tel:+8613800000000',
 'data:text/plain,private',
 'wss://example.test/socket',
 'candidate.example.test/profile',
]:
 text,_=scope['resume_text'](f'Link {url} now','test',128)
 assert '[redacted-url]' in text and url not in text
date,_=scope['resume_text']('Worked 2020-01 to 2024-01','test',128)
assert date == 'Worked 2020-01 to 2024-01'
handle,shortened=scope['resume_text'](
 'Built useful tools with @platform team and kept ordinary context',
 'test',
 128
)
assert not shortened
assert '@' not in handle
assert '[redacted-at]platform' in handle
assert 'Built useful tools' in handle and 'kept ordinary context' in handle
bounded,shortened=scope['resume_text']('@handle '*100,'test',32)
assert shortened and len(bounded) == 32 and '@' not in bounded
print('normalized')
"#,
        );
        assert_eq!(result, "normalized");
    }

    #[test]
    fn python_helper_encodes_the_minimal_techwolf_wire_shape() {
        let encoded =
            run_pure_helper("print(scope['encode_protocol'](1,2,'boss',3,'Hi',4,5).hex())");
        assert_eq!(
            encoded,
            "08011a270a0408013800120a08021204626f737338031801200528043208080110011a0248695805a00101"
        );
    }

    #[test]
    fn python_helper_bounds_history_by_final_utf8_response_bytes() {
        let result = run_pure_helper(
            "items=[{'direction':'incoming','text':'界'*2000,'timestamp_ms':i+1} for i in range(20)]\nresponse=scope['bounded_history_response']('wt2=secret',items)\nencoded=scope['json'].dumps(response,ensure_ascii=False,separators=(',',':')).encode('utf-8')\nassert len(encoded)+1 <= scope['MAX_HISTORY_RESPONSE_BYTES']\nassert 0 < len(response['messages']) < 20\nassert response['messages'][-1]['timestamp_ms'] == 20\nprint('bounded')",
        );
        assert_eq!(result, "bounded");
    }

    #[test]
    fn python_helper_validates_inbox_ids_and_never_drops_conversations() {
        let result = run_pure_helper(
            "ids=scope['validate_inbox_remote_ids'](['one','two'])\nassert ids == ['one','two']\nitems=[{'remote_id':'one','latest':None},{'remote_id':'two','latest':{'direction':'incoming','text':'界'*512,'timestamp_ms':1,'truncated':True}}]\nresponse=scope['bounded_inbox_response']('wt2=secret',items)\nassert response['count'] == 2 and len(response['conversations']) == 2\nfor invalid in [[],['same','same'],['1','2','3','4','5','6']]:\n try:\n  scope['validate_inbox_remote_ids'](invalid)\n  raise AssertionError('accepted invalid inbox ids')\n except scope['SafeFailure']:\n  pass\nprint('bounded')",
        );
        assert_eq!(result, "bounded");
    }

    #[test]
    fn python_helper_varint_rejects_negative_values() {
        let result = run_pure_helper(
            "try:\n scope['encode_varint'](-1)\n print('accepted')\nexcept scope['SafeFailure']:\n print('rejected')",
        );
        assert_eq!(result, "rejected");
    }

    #[test]
    fn recruiter_helper_uses_only_the_fixed_get_friend_list_route() {
        assert!(HELPER.contains("getBossFriendListV2.json"));
        assert!(
            HELPER.contains("params={\"page\": str(page), \"status\": \"0\", \"jobId\": \"0\"}")
        );
        let result = run_pure_helper(
            "assert scope['recruiter_direction']({'isFromGeek': True}) == 'candidate_to_recruiter'\nassert scope['recruiter_direction']({}) == 'unknown'\nprint('safe')",
        );
        assert_eq!(result, "safe");
    }

    #[test]
    fn native_recruiter_challenge_error_routes_to_legacy_token_retry() {
        assert!(is_recruiter_challenge(&BossError::Authentication(
            "Zhipin security challenge requires native token support; API code 37 request was not retried"
                .to_owned(),
        )));
        assert!(!is_recruiter_challenge(&BossError::Authentication(
            "Zhipin request failed with API code 24".to_owned(),
        )));
    }
}
