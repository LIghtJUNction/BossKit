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

fn invoke_helper(request: &[u8]) -> Result<Vec<u8>, BossError> {
    let mut child = Command::new("uv")
        .args([
            "run",
            "--quiet",
            "--with",
            "iv8==0.1.4",
            "--with",
            "requests==2.34.2",
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
    if response.action.as_deref() != Some("refresh") {
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
    if response.action.as_deref() != Some("greet") {
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

fn transport_error(message: &str) -> BossError {
    BossError::Authentication(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
