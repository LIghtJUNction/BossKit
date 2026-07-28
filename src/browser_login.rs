//! Isolated, user-driven Chromium login fallback for the native CLI.

use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::{Builder, TempDir};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};
use url::Url;

use crate::Platform;
use crate::auth::{domain_matches, validate_cookie};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const CDP_IO_TIMEOUT: Duration = Duration::from_secs(3);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_ACTIVE_PORT_BYTES: u64 = 4 * 1024;
const MAX_CDP_MESSAGE_BYTES: usize = 128 * 1024;
const MAX_CDP_MESSAGES_PER_REQUEST: usize = 64;

#[cfg(target_os = "linux")]
const FALLBACK_BROWSERS: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
];

#[cfg(not(target_os = "linux"))]
const FALLBACK_BROWSERS: &[&str] = &[];

/// Returns the official user-facing landing URL for one supported platform.
#[must_use]
pub(crate) const fn landing_url(platform: Platform) -> &'static str {
    match platform {
        Platform::Zhipin => "https://www.zhipin.com/web/geek/job",
        Platform::Zhilian => "https://sou.zhaopin.com",
        Platform::Qiancheng => "https://we.51job.com",
    }
}

/// Runs an isolated, user-confirmed browser login and returns a validated Cookie header.
///
/// Every setup or protocol failure intentionally resolves to `None`: callers emit a
/// structured unresolved login result without exposing browser internals.
pub(crate) fn interactive_login(platform: Platform, auth_root: &Path) -> Option<String> {
    if !io::stdin().is_terminal() {
        return None;
    }
    let profile = private_temporary_profile(auth_root)?;
    let mut browser = BrowserSession::launch(profile.path())?;
    let session_id = browser.create_landing_target(platform)?;
    if !wait_for_user_confirmation(platform) {
        return None;
    }
    browser.platform_cookie(platform, &session_id)
}

fn private_temporary_profile(auth_root: &Path) -> Option<TempDir> {
    let profile = Builder::new()
        .prefix("browser-profile-")
        .rand_bytes(16)
        .tempdir_in(auth_root)
        .ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(profile.path(), fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(profile)
}

fn wait_for_user_confirmation(platform: Platform) -> bool {
    eprint!(
        "请在已打开的 {} 浏览器窗口完成登录，完成后输入 done 并回车（输入 cancel 取消）：",
        platform.display_name()
    );
    if io::stderr().flush().is_err() {
        return false;
    }
    let mut confirmation = String::new();
    io::stdin()
        .read_line(&mut confirmation)
        .is_ok_and(|_| confirmation.trim().eq_ignore_ascii_case("done"))
}

struct BrowserSession {
    child: Child,
    cdp: CdpClient,
}

impl BrowserSession {
    fn launch(profile: &Path) -> Option<Self> {
        let child = launch_browser(profile)?;
        let mut session = Self {
            child,
            cdp: CdpClient::unavailable(),
        };
        let endpoint = wait_for_devtools_endpoint(profile)?;
        session.cdp = CdpClient::connect(&endpoint)?;
        Some(session)
    }

    fn create_landing_target(&mut self, platform: Platform) -> Option<String> {
        create_landing_target(&mut self.cdp, platform)
    }

    fn platform_cookie(&mut self, platform: Platform, session_id: &str) -> Option<String> {
        platform_cookie(&mut self.cdp, platform, session_id)
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn create_landing_target<S: Read + Write>(
    cdp: &mut CdpClient<S>,
    platform: Platform,
) -> Option<String> {
    let target = cdp.request(
        "Target.createTarget",
        json!({"url":landing_url(platform)}),
        None,
    )?;
    let target_id = target.get("targetId")?.as_str()?;
    let attached = cdp.request(
        "Target.attachToTarget",
        json!({"targetId":target_id,"flatten":true}),
        None,
    )?;
    attached.get("sessionId")?.as_str().map(ToOwned::to_owned)
}

fn platform_cookie<S: Read + Write>(
    cdp: &mut CdpClient<S>,
    platform: Platform,
    session_id: &str,
) -> Option<String> {
    cdp.request("Network.enable", json!({}), Some(session_id))?;
    let response = cdp.request(
        "Network.getCookies",
        json!({"urls":[landing_url(platform)]}),
        Some(session_id),
    )?;
    cookie_header_from_protocol(platform, &response)
}

fn launch_browser(profile: &Path) -> Option<Child> {
    browser_candidates().into_iter().find_map(|browser| {
        let mut user_data_dir = std::ffi::OsString::from("--user-data-dir=");
        user_data_dir.push(profile.as_os_str());
        Command::new(browser)
            .arg(user_data_dir)
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--remote-debugging-port=0")
            .arg("--new-window")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    })
}

fn browser_candidates() -> Vec<std::ffi::OsString> {
    if let Some(browser) = env::var_os("BOSS_BROWSER").filter(|value| !value.is_empty()) {
        return vec![browser];
    }
    FALLBACK_BROWSERS
        .iter()
        .map(std::ffi::OsString::from)
        .collect()
}

fn wait_for_devtools_endpoint(profile: &Path) -> Option<String> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Some(endpoint) = devtools_endpoint_from_file(profile) {
            return Some(endpoint);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(STARTUP_POLL_INTERVAL);
    }
}

fn devtools_endpoint_from_file(profile: &Path) -> Option<String> {
    let active_port = profile.join("DevToolsActivePort");
    let metadata = fs::symlink_metadata(&active_port).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_ACTIVE_PORT_BYTES
    {
        return None;
    }
    let contents = fs::read_to_string(active_port).ok()?;
    if contents.len() as u64 > MAX_ACTIVE_PORT_BYTES {
        return None;
    }
    parse_devtools_active_port(&contents)
}

fn parse_devtools_active_port(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    let port = lines.next()?.trim().parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }
    let path = lines.next()?.trim();
    if !path.starts_with("/devtools/browser/")
        || path.len() > 512
        || path.contains(char::is_whitespace)
        || lines.any(|line| !line.trim().is_empty())
    {
        return None;
    }
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

struct CdpClient<S = TcpStream> {
    socket: Option<WebSocket<S>>,
    next_id: u64,
}

impl CdpClient<TcpStream> {
    const fn unavailable() -> Self {
        Self {
            socket: None,
            next_id: 1,
        }
    }

    fn connect(endpoint: &str) -> Option<Self> {
        let endpoint = browser_endpoint(endpoint)?;
        let port = endpoint.port()?;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = TcpStream::connect_timeout(&address, CDP_IO_TIMEOUT).ok()?;
        stream.set_read_timeout(Some(CDP_IO_TIMEOUT)).ok()?;
        stream.set_write_timeout(Some(CDP_IO_TIMEOUT)).ok()?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_CDP_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_CDP_MESSAGE_BYTES));
        let (socket, _) =
            tungstenite::client::client_with_config(endpoint.as_str(), stream, Some(config))
                .ok()?;
        Some(Self::with_socket(socket))
    }
}

impl<S: Read + Write> CdpClient<S> {
    fn with_socket(socket: WebSocket<S>) -> Self {
        Self {
            socket: Some(socket),
            next_id: 1,
        }
    }

    fn request(&mut self, method: &str, params: Value, session_id: Option<&str>) -> Option<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session_id) = session_id {
            request["sessionId"] = Value::String(session_id.to_owned());
        }
        let payload = serde_json::to_string(&request).ok()?;
        let socket = self.socket.as_mut()?;
        socket.send(Message::text(payload)).ok()?;
        for _ in 0..MAX_CDP_MESSAGES_PER_REQUEST {
            match socket.read().ok()? {
                Message::Text(message) => {
                    if message.len() > MAX_CDP_MESSAGE_BYTES {
                        return None;
                    }
                    let response: Value = serde_json::from_str(message.as_str()).ok()?;
                    if response.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if response.get("error").is_some() {
                        return None;
                    }
                    return response.get("result").cloned();
                }
                Message::Ping(_) => socket.flush().ok()?,
                Message::Close(_) => return None,
                Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        None
    }
}

fn browser_endpoint(endpoint: &str) -> Option<Url> {
    let parsed = Url::parse(endpoint).ok()?;
    if parsed.scheme() != "ws"
        || parsed.host_str() != Some("127.0.0.1")
        || parsed.port().is_none_or(|port| port == 0)
        || !parsed.path().starts_with("/devtools/browser/")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(parsed)
}

fn cookie_header_from_protocol(platform: Platform, response: &Value) -> Option<String> {
    let cookies = response.get("cookies")?.as_array()?;
    let mut pairs = Vec::new();
    for cookie in cookies {
        let domain = cookie.get("domain")?.as_str()?;
        if !domain_matches(platform, domain) {
            continue;
        }
        let name = cookie.get("name")?.as_str()?;
        let value = cookie.get("value")?.as_str()?;
        let pair = format!("{name}={value}");
        if validate_cookie(&pair).is_err() || pairs.iter().any(|existing| existing == &pair) {
            continue;
        }
        pairs.push(pair);
    }
    let cookie = pairs.join("; ");
    validate_cookie(&cookie).ok()?;
    Some(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_urls_are_the_expected_official_platform_pages() {
        assert_eq!(
            landing_url(Platform::Zhipin),
            "https://www.zhipin.com/web/geek/job"
        );
        assert_eq!(landing_url(Platform::Zhilian), "https://sou.zhaopin.com");
        assert_eq!(landing_url(Platform::Qiancheng), "https://we.51job.com");
    }

    #[test]
    fn devtools_endpoint_parser_accepts_only_loopback_browser_targets() {
        let endpoint =
            parse_devtools_active_port("9222\n/devtools/browser/fixture\n").expect("endpoint");
        assert_eq!(endpoint, "ws://127.0.0.1:9222/devtools/browser/fixture");
        assert!(parse_devtools_active_port("0\n/devtools/browser/fixture\n").is_none());
        assert!(parse_devtools_active_port("9222\n/devtools/page/fixture\n").is_none());
        assert!(browser_endpoint("ws://127.0.0.1:9222/devtools/browser/fixture").is_some());
        assert!(browser_endpoint("ws://example.test:9222/devtools/browser/fixture").is_none());
    }

    #[test]
    fn protocol_cookie_collection_filters_domains_and_invalid_pairs() {
        let response = json!({"cookies":[
            {"domain":".zhipin.com","name":"session","value":"fixture"},
            {"domain":".zhaopin.com","name":"other","value":"skip"},
            {"domain":".zhipin.com","name":"bad name","value":"skip"},
            {"domain":"jobs.zhipin.com","name":"token","value":"value"}
        ]});
        assert_eq!(
            cookie_header_from_protocol(Platform::Zhipin, &response),
            Some("session=fixture; token=value".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn cdp_client_uses_target_scoped_cookie_requests_and_ignores_unrelated_messages() {
        use std::os::unix::net::UnixStream;
        use tungstenite::protocol::Role;

        let (client_stream, server_stream) = UnixStream::pair().expect("socket pair");
        client_stream
            .set_read_timeout(Some(CDP_IO_TIMEOUT))
            .expect("client read timeout");
        client_stream
            .set_write_timeout(Some(CDP_IO_TIMEOUT))
            .expect("client write timeout");
        server_stream
            .set_read_timeout(Some(CDP_IO_TIMEOUT))
            .expect("server read timeout");
        server_stream
            .set_write_timeout(Some(CDP_IO_TIMEOUT))
            .expect("server write timeout");
        let server = thread::spawn(move || {
            let mut socket = WebSocket::from_raw_socket(server_stream, Role::Server, None);

            let create = receive_cdp_request(&mut socket);
            assert_eq!(create["method"], "Target.createTarget");
            assert_eq!(
                create["params"]["url"],
                "https://www.zhipin.com/web/geek/job"
            );
            socket
                .send(Message::text(
                    json!({"method":"Target.targetCreated","params":{}}).to_string(),
                ))
                .expect("notification");
            socket
                .send(Message::text(
                    json!({"id":999,"result":{"ignored":true}}).to_string(),
                ))
                .expect("unrelated response");
            send_cdp_result(&mut socket, &create, json!({"targetId":"target-fixture"}));

            let attach = receive_cdp_request(&mut socket);
            assert_eq!(attach["method"], "Target.attachToTarget");
            assert_eq!(attach["params"]["targetId"], "target-fixture");
            assert_eq!(attach["params"]["flatten"], true);
            send_cdp_result(&mut socket, &attach, json!({"sessionId":"session-fixture"}));

            let enable = receive_cdp_request(&mut socket);
            assert_eq!(enable["method"], "Network.enable");
            assert_eq!(enable["sessionId"], "session-fixture");
            send_cdp_result(&mut socket, &enable, json!({}));

            let cookies = receive_cdp_request(&mut socket);
            assert_eq!(cookies["method"], "Network.getCookies");
            assert_eq!(cookies["sessionId"], "session-fixture");
            assert_eq!(
                cookies["params"]["urls"],
                json!(["https://www.zhipin.com/web/geek/job"])
            );
            send_cdp_result(
                &mut socket,
                &cookies,
                json!({"cookies":[{"domain":".zhipin.com","name":"session","value":"fixture"}]}),
            );
        });

        let client_socket = WebSocket::from_raw_socket(client_stream, Role::Client, None);
        let mut client = CdpClient::with_socket(client_socket);
        let session_id = create_landing_target(&mut client, Platform::Zhipin).expect("target");
        assert_eq!(session_id, "session-fixture");
        assert_eq!(
            platform_cookie(&mut client, Platform::Zhipin, &session_id),
            Some("session=fixture".to_owned())
        );
        server.join().expect("server");
    }

    #[cfg(unix)]
    #[test]
    fn temporary_browser_profile_is_private_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let auth_root = tempdir().expect("auth root");
        let profile = private_temporary_profile(auth_root.path()).expect("profile");
        let profile_path = profile.path().to_path_buf();
        assert_eq!(profile_path.parent(), Some(auth_root.path()));
        let mode = fs::metadata(&profile_path)
            .expect("profile metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        drop(profile);
        assert!(!profile_path.exists());
    }

    fn receive_cdp_request<S: Read + Write>(socket: &mut WebSocket<S>) -> Value {
        let Message::Text(message) = socket.read().expect("request") else {
            panic!("expected text request");
        };
        serde_json::from_str(message.as_str()).expect("request json")
    }

    fn send_cdp_result<S: Read + Write>(socket: &mut WebSocket<S>, request: &Value, result: Value) {
        socket
            .send(Message::text(
                json!({"id":request["id"].clone(),"result":result}).to_string(),
            ))
            .expect("result");
    }
}
