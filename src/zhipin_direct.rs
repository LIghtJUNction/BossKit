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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectChatMessage {
    pub(crate) direction: String,
    pub(crate) text: String,
    pub(crate) timestamp_ms: u64,
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

/// Refreshes and verifies one stored Zhipin Cookie without a browser process.
pub(crate) fn refresh_session(cookie: &str) -> Result<DirectSessionRefresh, BossError> {
    let request = serde_json::to_vec(&RefreshRequest {
        action: "refresh",
        cookie,
    })
    .map_err(|_| transport_error("unable to encode direct transport request"))?;
    parse_refresh_response(&invoke_helper(&request)?)
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
    if response.action.as_deref() != Some("history") || response.state.is_some() {
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
    fn python_helper_varint_rejects_negative_values() {
        let result = run_pure_helper(
            "try:\n scope['encode_varint'](-1)\n print('accepted')\nexcept scope['SafeFailure']:\n print('rejected')",
        );
        assert_eq!(result, "rejected");
    }
}
