//! Terminal-only QR login fallback for the native CLI.
//!
//! This module deliberately owns no persistent state.  It creates a fresh in-memory
//! Cookie jar for one attempt and returns a validated header only after the platform's
//! documented QR completion signal.  The service layer is the sole owner of persistence.

use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use image::{ImageReader, Limits};
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use rqrr::PreparedImage;
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use url::Url;

use crate::Platform;
use crate::auth::validate_cookie;

const FLOW_TIMEOUT: Duration = Duration::from_secs(120);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_ATTEMPTS: usize = 60;
const MAX_JSON_BYTES: usize = 128 * 1024;
const MAX_HTML_BYTES: usize = 128 * 1024;
const MAX_IMAGE_BYTES: usize = 1024 * 1024;
const MAX_QR_IMAGE_DIMENSION: u32 = 2048;
const MAX_QR_IMAGE_ALLOC: u64 = 16 * 1024 * 1024;
const MAX_QR_PAYLOAD_BYTES: usize = 4096;
const MAX_OPAQUE_VALUE_BYTES: usize = 4096;
const MAX_REDIRECTS: usize = 2;

const ZHIPIN_SCOPE: &str = "https://www.zhipin.com/";
const ZHIPIN_RANDKEY: &str = "https://www.zhipin.com/wapi/zppassport/captcha/randkey";
const ZHIPIN_SCAN: &str = "https://www.zhipin.com/wapi/zppassport/captcha/scan";
const ZHIPIN_GET_SECOND_KEY: &str = "https://www.zhipin.com/wapi/zppassport/captcha/getSecondKey";
const ZHIPIN_SCAN_SECOND: &str = "https://www.zhipin.com/wapi/zppassport/captcha/scanSecond";
const ZHIPIN_SCAN_LOGIN: &str = "https://www.zhipin.com/wapi/zppassport/captcha/scanLogin";
const ZHIPIN_DISPATCHER: &str = "https://www.zhipin.com/wapi/zppassport/qrlogin/dispatcher";

const ZHILIAN_SCOPE: &str = "https://sou.zhaopin.com/";
const ZHILIAN_QR_START: &str = "https://passport.zhaopin.com/napi/wechat-qrcode";
const ZHILIAN_QR_POLL: &str = "https://passport.zhaopin.com/napi/wechat-qrcode-login";
// Public application identifier used by the official web QR flow.  It is intentionally
// crate-private and never appears in CLI output or documentation.
const ZHILIAN_PASSPORT_APP_ID: &str = "9f69dd17cf834693b04e06dd5e9b5728";

const QIANCHENG_LOGIN_PAGE: &str = "https://login.51job.com/login.php";
const QIANCHENG_QR_START: &str = "https://login.51job.com/ajax/qrcodelogin.php";
const QIANCHENG_QR_POLL: &str = "https://login.51job.com/ajax/pcqr_scanlogin_poll.php";
const QIANCHENG_SCOPE: &str = "https://we.51job.com/";

/// Tries the platform's user-driven QR protocol in a fresh private in-memory session.
///
/// All failure modes intentionally resolve to `None`, allowing the caller to use the
/// existing browser/manual fallback without revealing remote protocol details.
pub(crate) async fn interactive_login(platform: Platform) -> Option<String> {
    if !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        return None;
    }
    let jar = Arc::new(Jar::default());
    let transport = ReqwestQrTransport::new(Arc::clone(&jar))?;
    let config = FlowConfig::interactive();
    timeout(
        FLOW_TIMEOUT,
        attempt_with_transport(platform, &transport, config, render_terminal_qr),
    )
    .await
    .ok()
    .flatten()
}

#[derive(Clone, Copy)]
struct FlowConfig {
    poll_attempts: usize,
    poll_interval: Duration,
}

impl FlowConfig {
    const fn interactive() -> Self {
        Self {
            poll_attempts: POLL_ATTEMPTS,
            poll_interval: POLL_INTERVAL,
        }
    }

    #[cfg(test)]
    const fn immediate(poll_attempts: usize) -> Self {
        Self {
            poll_attempts,
            poll_interval: Duration::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QrOperation {
    ZhipinRandkey,
    ZhipinScan,
    ZhipinSecondKey,
    ZhipinScanSecond,
    ZhipinScanLogin,
    ZhipinDispatcher,
    ZhilianStart,
    ZhilianImage,
    ZhilianPoll,
    QianchengPage,
    QianchengStart,
    QianchengImage,
    QianchengPoll,
    QianchengRedirect,
}

#[derive(Clone, Copy)]
enum QrMethod {
    Get,
    Post,
}

enum RequestBody {
    Empty,
    Form(Vec<(String, String)>),
    Json(Value),
}

struct QrRequest {
    operation: QrOperation,
    method: QrMethod,
    url: Url,
    query: Vec<(String, String)>,
    body: RequestBody,
    max_bytes: usize,
    requires_image_content_type: bool,
}

impl QrRequest {
    fn get(operation: QrOperation, url: Url, max_bytes: usize) -> Self {
        Self {
            operation,
            method: QrMethod::Get,
            url,
            query: Vec::new(),
            body: RequestBody::Empty,
            max_bytes,
            requires_image_content_type: false,
        }
    }

    fn image_get(operation: QrOperation, url: Url) -> Self {
        Self {
            operation,
            method: QrMethod::Get,
            url,
            query: Vec::new(),
            body: RequestBody::Empty,
            max_bytes: MAX_IMAGE_BYTES,
            requires_image_content_type: true,
        }
    }

    fn post_form(operation: QrOperation, url: Url, form: Vec<(String, String)>) -> Self {
        Self {
            operation,
            method: QrMethod::Post,
            url,
            query: Vec::new(),
            body: RequestBody::Form(form),
            max_bytes: MAX_JSON_BYTES,
            requires_image_content_type: false,
        }
    }

    fn post_json(operation: QrOperation, url: Url, body: Value) -> Self {
        Self {
            operation,
            method: QrMethod::Post,
            url,
            query: Vec::new(),
            body: RequestBody::Json(body),
            max_bytes: MAX_JSON_BYTES,
            requires_image_content_type: false,
        }
    }

    fn query(mut self, key: &str, value: impl Into<String>) -> Self {
        self.query.push((key.to_owned(), value.into()));
        self
    }
}

struct QrResponse {
    status: StatusCode,
    location: Option<String>,
    body: Vec<u8>,
}

#[async_trait(?Send)]
trait QrTransport {
    async fn send(&self, request: QrRequest) -> Option<QrResponse>;

    fn cookie_header(&self, scope: &Url) -> Option<String>;
}

struct ReqwestQrTransport {
    client: Client,
    jar: Arc<Jar>,
}

impl ReqwestQrTransport {
    fn new(jar: Arc<Jar>) -> Option<Self> {
        let client = Client::builder()
            .cookie_provider(Arc::clone(&jar))
            .redirect(Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self { client, jar })
    }
}

#[async_trait(?Send)]
impl QrTransport for ReqwestQrTransport {
    async fn send(&self, request: QrRequest) -> Option<QrResponse> {
        // The operation tag is intentionally carried by every request so mock transports can
        // assert the exact state-machine sequence without recording endpoint values.
        let _ = request.operation;
        let mut builder = match request.method {
            QrMethod::Get => self.client.get(request.url),
            QrMethod::Post => self.client.post(request.url),
        };
        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        builder = match request.body {
            RequestBody::Empty => builder,
            RequestBody::Form(form) => builder.form(&form),
            RequestBody::Json(value) => builder.json(&value),
        };
        let response = builder.send().await.ok()?;
        let status = response.status();
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > request.max_bytes)
        {
            return None;
        }
        if request.requires_image_content_type
            && !is_image_content_type(response.headers().get(CONTENT_TYPE))
        {
            return None;
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body = bounded_body(response, request.max_bytes).await?;
        Some(QrResponse {
            status,
            location,
            body,
        })
    }

    fn cookie_header(&self, scope: &Url) -> Option<String> {
        let cookie = self.jar.cookies(scope)?.to_str().ok()?.to_owned();
        validate_cookie(&cookie).ok()?;
        Some(cookie)
    }
}

async fn bounded_body(mut response: reqwest::Response, max_bytes: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len().checked_add(chunk.len())? > max_bytes {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

fn is_image_content_type(value: Option<&reqwest::header::HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("image/png")
                    || media_type.trim().eq_ignore_ascii_case("image/jpeg")
            })
        })
}

async fn attempt_with_transport<T, R>(
    platform: Platform,
    transport: &T,
    config: FlowConfig,
    render: R,
) -> Option<String>
where
    T: QrTransport + ?Sized,
    R: Fn(&str) -> bool,
{
    match platform {
        Platform::Zhipin => zhipin_login(transport, config, &render).await,
        Platform::Zhilian => zhilian_login(transport, config, &render).await,
        Platform::Qiancheng => qiancheng_login(transport, config, &render).await,
    }
}

async fn zhipin_login<T, R>(transport: &T, config: FlowConfig, render: &R) -> Option<String>
where
    T: QrTransport + ?Sized,
    R: Fn(&str) -> bool,
{
    let first = json_response(
        transport,
        QrRequest::post_form(
            QrOperation::ZhipinRandkey,
            fixed_url(ZHIPIN_RANDKEY)?,
            Vec::new(),
        ),
    )
    .await?;
    let first_qr_id = zhipin_qr_id(&first)?;
    if !render(&first_qr_id) {
        return None;
    }
    wait_for_zhipin_scan(
        transport,
        config,
        QrOperation::ZhipinScan,
        ZHIPIN_SCAN,
        &first_qr_id,
    )
    .await?;

    let second = json_response(
        transport,
        QrRequest::get(
            QrOperation::ZhipinSecondKey,
            fixed_url(ZHIPIN_GET_SECOND_KEY)?,
            MAX_JSON_BYTES,
        )
        .query("qrId", &first_qr_id),
    )
    .await?;
    let second_qr_id = zhipin_qr_id(&second)?;
    if !render(&second_qr_id) {
        return None;
    }
    wait_for_zhipin_scan(
        transport,
        config,
        QrOperation::ZhipinScanSecond,
        ZHIPIN_SCAN_SECOND,
        &second_qr_id,
    )
    .await?;

    let pk = wait_for_zhipin_login(transport, config, &second_qr_id).await?;
    let scope = fixed_url(ZHIPIN_SCOPE)?;
    let cookie_before_dispatcher = transport.cookie_header(&scope);
    let dispatcher = json_response(
        transport,
        QrRequest::post_form(
            QrOperation::ZhipinDispatcher,
            fixed_url(ZHIPIN_DISPATCHER)?,
            vec![("qrId".to_owned(), second_qr_id), ("pk".to_owned(), pk)],
        ),
    )
    .await?;
    if response_code(&dispatcher) != Some(0) {
        return None;
    }
    let cookie = session_cookie(transport, ZHIPIN_SCOPE)?;
    (cookie_before_dispatcher.as_deref() != Some(cookie.as_str())).then_some(cookie)
}

async fn wait_for_zhipin_scan<T>(
    transport: &T,
    config: FlowConfig,
    operation: QrOperation,
    endpoint: &str,
    qr_id: &str,
) -> Option<()>
where
    T: QrTransport + ?Sized,
{
    for attempt in 0..config.poll_attempts {
        let response = json_response(
            transport,
            QrRequest::get(operation, fixed_url(endpoint)?, MAX_JSON_BYTES).query("qrId", qr_id),
        )
        .await?;
        match response.get("scaned").and_then(Value::as_bool) {
            Some(true) => return Some(()),
            Some(false) if attempt + 1 < config.poll_attempts => pause(config).await,
            Some(false) => return None,
            None => return None,
        }
    }
    None
}

async fn wait_for_zhipin_login<T>(transport: &T, config: FlowConfig, qr_id: &str) -> Option<String>
where
    T: QrTransport + ?Sized,
{
    for attempt in 0..config.poll_attempts {
        let response = json_response(
            transport,
            QrRequest::get(
                QrOperation::ZhipinScanLogin,
                fixed_url(ZHIPIN_SCAN_LOGIN)?,
                MAX_JSON_BYTES,
            )
            .query("qrId", qr_id),
        )
        .await?;
        match response.get("scaned").and_then(Value::as_bool) {
            Some(false) if attempt + 1 < config.poll_attempts => pause(config).await,
            Some(false) => return None,
            Some(true) => return zhipin_pk(&response),
            None => return None,
        }
    }
    None
}

fn zhipin_qr_id(value: &Value) -> Option<String> {
    (response_code(value) == Some(0))
        .then(|| value.get("zpData")?.get("qrId")?.as_str())
        .flatten()
        .and_then(opaque_identifier)
}

fn zhipin_pk(value: &Value) -> Option<String> {
    match value.get("login")? {
        Value::String(value) => opaque_value(value),
        Value::Object(value) => value.get("pk")?.as_str().and_then(opaque_value),
        _ => None,
    }
}

async fn zhilian_login<T, R>(transport: &T, config: FlowConfig, render: &R) -> Option<String>
where
    T: QrTransport + ?Sized,
    R: Fn(&str) -> bool,
{
    let started = json_response(
        transport,
        QrRequest::post_json(
            QrOperation::ZhilianStart,
            fixed_url(ZHILIAN_QR_START)?,
            json!({"appID":ZHILIAN_PASSPORT_APP_ID}),
        ),
    )
    .await?;
    let data = started.get("data")?;
    let image_path = data.get("path")?.as_str()?;
    let validate_id = data.get("validateId")?.as_str().and_then(opaque_value)?;
    let image_url = trusted_image_url(image_path, ImageOrigin::Zhilian)?;
    let image = transport
        .send(QrRequest::image_get(QrOperation::ZhilianImage, image_url))
        .await?;
    if !image.status.is_success() {
        return None;
    }
    let payload = decode_qr_image(&image.body)?;
    if !render(&payload) {
        return None;
    }
    let cookie_before_poll = zhilian_session(transport);

    for attempt in 0..config.poll_attempts {
        let response = json_response(
            transport,
            QrRequest::get(
                QrOperation::ZhilianPoll,
                fixed_url(ZHILIAN_QR_POLL)?,
                MAX_JSON_BYTES,
            )
            .query("appID", ZHILIAN_PASSPORT_APP_ID)
            .query("rememberMe", "true")
            .query("validateId", &validate_id)
            .query("timestamp", unix_timestamp_millis()?),
        )
        .await?;
        if response_code(&response) != Some(0) {
            return None;
        }
        if let Some(cookie) = zhilian_session(transport)
            && cookie_before_poll.as_deref() != Some(cookie.as_str())
        {
            return Some(cookie);
        }
        if attempt + 1 < config.poll_attempts {
            pause(config).await;
        }
    }
    None
}

fn zhilian_session<T>(transport: &T) -> Option<String>
where
    T: QrTransport + ?Sized,
{
    let scope = fixed_url(ZHILIAN_SCOPE)?;
    let header = transport.cookie_header(&scope)?;
    let at = named_cookie(&header, "at")?;
    let rt = named_cookie(&header, "rt")?;
    let cookie = format!("at={at}; rt={rt}");
    validate_cookie(&cookie).ok()?;
    Some(cookie)
}

async fn qiancheng_login<T, R>(transport: &T, config: FlowConfig, render: &R) -> Option<String>
where
    T: QrTransport + ?Sized,
    R: Fn(&str) -> bool,
{
    let page = transport
        .send(QrRequest::get(
            QrOperation::QianchengPage,
            fixed_url(QIANCHENG_LOGIN_PAGE)?,
            MAX_HTML_BYTES,
        ))
        .await?;
    if !page.status.is_success() {
        return None;
    }
    let html = std::str::from_utf8(&page.body).ok()?;
    let guid = qiancheng_guid(html)?;
    let destination = qiancheng_destination(html)?;
    let started = json_response(
        transport,
        QrRequest::get(
            QrOperation::QianchengStart,
            fixed_url(QIANCHENG_QR_START)?,
            MAX_JSON_BYTES,
        )
        .query("guid", &guid)
        .query("partner", "pc_scanner_login")
        .query("from", "pc")
        .query("type", "refresh"),
    )
    .await?;
    if numeric_field(&started, "status") != Some(1) {
        return None;
    }
    let image_path = started.get("result")?.as_str()?;
    let image_url = trusted_image_url(image_path, ImageOrigin::Qiancheng)?;
    let image = transport
        .send(QrRequest::image_get(QrOperation::QianchengImage, image_url))
        .await?;
    if !image.status.is_success() {
        return None;
    }
    let payload = decode_qr_image(&image.body)?;
    if !render(&payload) {
        return None;
    }

    for attempt in 0..config.poll_attempts {
        let response = json_response(
            transport,
            QrRequest::get(
                QrOperation::QianchengPoll,
                fixed_url(QIANCHENG_QR_POLL)?,
                MAX_JSON_BYTES,
            )
            .query("guid", &guid),
        )
        .await?;
        match numeric_field(&response, "result") {
            Some(0) if qiancheng_pending_state(&response) => {
                if attempt + 1 < config.poll_attempts {
                    pause(config).await;
                }
            }
            Some(1) => {
                let zdparam = response.get("zdparam")?.as_str().and_then(opaque_value)?;
                // The login page and QR setup may legitimately set ordinary cookies.  Snapshot
                // the relevant domain only after the platform has reported QR completion, then
                // require the guarded redirect to add or change that state.
                let cookie_before_completion = session_cookie(transport, QIANCHENG_SCOPE);
                return qiancheng_complete(
                    transport,
                    destination,
                    &zdparam,
                    cookie_before_completion.as_deref(),
                )
                .await;
            }
            _ => return None,
        }
    }
    None
}

async fn qiancheng_complete<T>(
    transport: &T,
    destination: Url,
    zdparam: &str,
    cookie_before_completion: Option<&str>,
) -> Option<String>
where
    T: QrTransport + ?Sized,
{
    let mut next = destination;
    next.query_pairs_mut().append_pair("zdparam", zdparam);
    for _ in 0..=MAX_REDIRECTS {
        let response = transport
            .send(QrRequest::get(
                QrOperation::QianchengRedirect,
                next.clone(),
                MAX_JSON_BYTES,
            ))
            .await?;
        if response.status.is_success() {
            let cookie = session_cookie(transport, QIANCHENG_SCOPE)?;
            return (cookie_before_completion != Some(cookie.as_str())).then_some(cookie);
        }
        if !response.status.is_redirection() {
            return None;
        }
        next = trusted_qiancheng_redirect(&next, response.location.as_deref()?)?;
    }
    None
}

fn qiancheng_pending_state(value: &Value) -> bool {
    let Some(code) = value.get("error_code") else {
        return false;
    };
    matches!(
        code,
        Value::String(value) if matches!(value.as_str(), "0" | "1" | "waiting" | "scanned")
    ) || matches!(code, Value::Number(value) if matches!(value.as_i64(), Some(0 | 1)))
}

fn qiancheng_guid(html: &str) -> Option<String> {
    javascript_property(html, "trackConfig", "guid")
        .as_deref()
        .and_then(opaque_identifier)
}

fn qiancheng_destination(html: &str) -> Option<Url> {
    let raw = javascript_property(html, "cfg.domain", "www")?;
    trusted_qiancheng_destination(&raw)
}

fn javascript_property(html: &str, object: &str, property: &str) -> Option<String> {
    let object_offset = html.find(object)?;
    let search = html.get(object_offset..)?;
    let bounded = search.get(..search.len().min(8 * 1024))?;
    let property_offset = bounded.find(property)?;
    let value = bounded.get(property_offset + property.len()..)?;
    let separator_offset = value.find([':', '='])?;
    let value = value.get(separator_offset + 1..)?.trim_start();
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let value = value.get(quote.len_utf8()..)?;
    let closing = value.find(quote)?;
    let extracted = value.get(..closing)?;
    if extracted.is_empty() || extracted.len() > MAX_OPAQUE_VALUE_BYTES || extracted.contains('\\')
    {
        return None;
    }
    Some(extracted.to_owned())
}

fn trusted_qiancheng_destination(raw: &str) -> Option<Url> {
    let base = fixed_url(QIANCHENG_SCOPE)?;
    let url = base.join(raw).ok()?;
    trusted_https_host(url, &["we.51job.com"])
}

fn trusted_qiancheng_redirect(current: &Url, raw: &str) -> Option<Url> {
    let url = current.join(raw).ok()?;
    trusted_https_host(url, &["we.51job.com"])
}

#[derive(Clone, Copy)]
enum ImageOrigin {
    Zhilian,
    Qiancheng,
}

fn trusted_image_url(raw: &str, origin: ImageOrigin) -> Option<Url> {
    let (base, hosts) = match origin {
        ImageOrigin::Zhilian => (ZHILIAN_QR_START, &["passport.zhaopin.com"][..]),
        ImageOrigin::Qiancheng => (
            QIANCHENG_QR_START,
            &["login.51job.com", "we.51job.com", "img.51jobcdn.com"][..],
        ),
    };
    let base = fixed_url(base)?;
    let url = base.join(raw).ok()?;
    trusted_https_host(url, hosts)
}

fn trusted_https_host(url: Url, allowed_hosts: &[&str]) -> Option<Url> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    allowed_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
        .then_some(url)
}

fn fixed_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    trusted_https_host(
        url,
        &[
            "www.zhipin.com",
            "passport.zhaopin.com",
            "sou.zhaopin.com",
            "login.51job.com",
            "we.51job.com",
        ],
    )
}

fn decode_qr_image(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes));
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_QR_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_QR_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_QR_IMAGE_ALLOC);
    reader.limits(limits);
    let image = reader.with_guessed_format().ok()?.decode().ok()?;
    let mut prepared = PreparedImage::prepare(image.to_luma8());
    let grids = prepared.detect_grids();
    if grids.len() != 1 {
        return None;
    }
    let (_, payload) = grids.into_iter().next()?.decode().ok()?;
    qr_payload(&payload).map(ToOwned::to_owned)
}

fn qr_payload(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= MAX_QR_PAYLOAD_BYTES
        && !value.chars().any(char::is_control))
    .then_some(value)
}

fn opaque_identifier(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_OPAQUE_VALUE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(value.to_owned())
}

fn opaque_value(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_OPAQUE_VALUE_BYTES
        || !value.is_ascii()
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_owned())
}

fn response_code(value: &Value) -> Option<i64> {
    value.get("code")?.as_i64()
}

fn numeric_field(value: &Value, name: &str) -> Option<i64> {
    match value.get(name)? {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.parse::<i64>().ok(),
        _ => None,
    }
}

async fn json_response<T>(transport: &T, request: QrRequest) -> Option<Value>
where
    T: QrTransport + ?Sized,
{
    let response = transport.send(request).await?;
    response.status.is_success().then_some(())?;
    serde_json::from_slice(&response.body).ok()
}

fn session_cookie<T>(transport: &T, scope: &str) -> Option<String>
where
    T: QrTransport + ?Sized,
{
    let scope = fixed_url(scope)?;
    let cookie = transport.cookie_header(&scope)?;
    validate_cookie(&cookie).ok()?;
    Some(cookie)
}

fn named_cookie<'a>(header: &'a str, expected: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == expected && !value.is_empty()).then_some(value)
    })
}

fn unix_timestamp_millis() -> Option<String> {
    Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis()
            .to_string(),
    )
}

async fn pause(config: FlowConfig) {
    if !config.poll_interval.is_zero() {
        sleep(config.poll_interval).await;
    }
}

fn render_terminal_qr(payload: &str) -> bool {
    let Some(rendered) = render_qr(payload) else {
        return false;
    };
    eprintln!(
        "请使用手机扫描终端中的二维码，并在手机上完成确认；过期、取消或拒绝会改用浏览器登录。"
    );
    eprintln!("{rendered}");
    true
}

fn render_qr(payload: &str) -> Option<String> {
    let payload = qr_payload(payload)?;
    let code = QrCode::new(payload.as_bytes()).ok()?;
    Some(
        code.render::<Dense1x2>()
            .quiet_zone(true)
            .module_dimensions(1, 1)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::sync::Mutex;

    use image::{DynamicImage, ImageFormat, Luma};

    use super::*;

    struct MockTransport {
        responses: Mutex<VecDeque<QrResponse>>,
        initial_cookies: Vec<(&'static str, &'static str)>,
        completion_cookies: Vec<(&'static str, &'static str)>,
        cookie_change_after: Option<QrOperation>,
        operations: Mutex<Vec<QrOperation>>,
    }

    impl MockTransport {
        fn new(responses: Vec<QrResponse>, cookies: Vec<(&'static str, &'static str)>) -> Self {
            Self::new_changed_after(responses, cookies.clone(), cookies, None)
        }

        fn new_after(
            responses: Vec<QrResponse>,
            cookies: Vec<(&'static str, &'static str)>,
            cookie_available_after: Option<QrOperation>,
        ) -> Self {
            Self::new_changed_after(responses, Vec::new(), cookies, cookie_available_after)
        }

        fn new_changed_after(
            responses: Vec<QrResponse>,
            initial_cookies: Vec<(&'static str, &'static str)>,
            completion_cookies: Vec<(&'static str, &'static str)>,
            cookie_change_after: Option<QrOperation>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                initial_cookies,
                completion_cookies,
                cookie_change_after,
                operations: Mutex::new(Vec::new()),
            }
        }

        fn operations(&self) -> Vec<QrOperation> {
            self.operations.lock().expect("operation log").clone()
        }
    }

    #[async_trait(?Send)]
    impl QrTransport for MockTransport {
        async fn send(&self, request: QrRequest) -> Option<QrResponse> {
            self.operations
                .lock()
                .expect("operation log")
                .push(request.operation);
            self.responses.lock().expect("response queue").pop_front()
        }

        fn cookie_header(&self, scope: &Url) -> Option<String> {
            let host = scope.host_str()?;
            let completion_cookie_is_available = self.cookie_change_after.is_none_or(|operation| {
                self.operations
                    .lock()
                    .expect("operation log")
                    .contains(&operation)
            });
            let cookies = if completion_cookie_is_available {
                &self.completion_cookies
            } else {
                &self.initial_cookies
            };
            cookies
                .iter()
                .find(|(expected, _)| host == *expected)
                .map(|(_, cookie)| (*cookie).to_owned())
        }
    }

    fn json_response(value: Value) -> QrResponse {
        QrResponse {
            status: StatusCode::OK,
            location: None,
            body: serde_json::to_vec(&value).expect("fixture JSON"),
        }
    }

    fn image_response() -> QrResponse {
        QrResponse {
            status: StatusCode::OK,
            location: None,
            body: qr_png(),
        }
    }

    fn redirect_response(location: &str) -> QrResponse {
        QrResponse {
            status: StatusCode::FOUND,
            location: Some(location.to_owned()),
            body: Vec::new(),
        }
    }

    fn qr_png() -> Vec<u8> {
        let code = QrCode::new(b"fixture-terminal-qr").expect("fixture QR");
        let image = code.render::<Luma<u8>>().min_dimensions(256, 256).build();
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("fixture image");
        bytes.into_inner()
    }

    #[test]
    fn unicode_renderer_never_includes_the_payload_as_terminal_text() {
        let rendered = render_qr("fixture-terminal-qr").expect("rendered QR");
        assert!(!rendered.contains("fixture-terminal-qr"));
        assert!(rendered.contains(['█', '▀', '▄']));
    }

    #[test]
    fn image_qr_decoder_accepts_one_bounded_qr_and_rejects_malformed_bytes() {
        assert!(decode_qr_image(&qr_png()).is_some());
        assert!(decode_qr_image(b"not an image").is_none());
    }

    #[test]
    fn image_urls_are_limited_to_expected_https_hosts() {
        assert!(trusted_image_url("/qr.png", ImageOrigin::Zhilian).is_some());
        assert!(
            trusted_image_url("https://img.51jobcdn.com/qr.png", ImageOrigin::Qiancheng).is_some()
        );
        for raw in [
            "data:image/png;base64,AA==",
            "javascript:alert(1)",
            "file:///tmp/qr.png",
            "http://passport.zhaopin.com/qr.png",
            "https://127.0.0.1/qr.png",
            "https://[::1]/qr.png",
            "https://example.invalid/qr.png",
        ] {
            assert!(trusted_image_url(raw, ImageOrigin::Zhilian).is_none());
        }
    }

    #[test]
    fn image_content_type_accepts_only_supported_qr_image_formats() {
        assert!(is_image_content_type(Some(
            &reqwest::header::HeaderValue::from_static("image/png")
        )));
        assert!(is_image_content_type(Some(
            &reqwest::header::HeaderValue::from_static("image/jpeg; charset=binary")
        )));
        assert!(!is_image_content_type(Some(
            &reqwest::header::HeaderValue::from_static("text/html")
        )));
    }

    #[tokio::test]
    async fn zhipin_requires_both_confirmations_dispatcher_and_a_session_cookie() {
        let transport = MockTransport::new_after(
            vec![
                json_response(json!({"code":0,"zpData":{"qrId":"fixture-first"}})),
                json_response(json!({"scaned":false})),
                json_response(json!({"scaned":true})),
                json_response(json!({"code":0,"zpData":{"qrId":"fixture-second"}})),
                json_response(json!({"scaned":true})),
                json_response(json!({"scaned":false})),
                json_response(json!({"scaned":true,"login":{"pk":"fixture-pk"}})),
                json_response(json!({"code":0})),
            ],
            vec![("www.zhipin.com", "session=fixture")],
            Some(QrOperation::ZhipinDispatcher),
        );
        let result = attempt_with_transport(
            Platform::Zhipin,
            &transport,
            FlowConfig::immediate(2),
            |_| true,
        )
        .await;
        assert!(result.is_some());
        assert_eq!(
            transport.operations(),
            vec![
                QrOperation::ZhipinRandkey,
                QrOperation::ZhipinScan,
                QrOperation::ZhipinScan,
                QrOperation::ZhipinSecondKey,
                QrOperation::ZhipinScanSecond,
                QrOperation::ZhipinScanLogin,
                QrOperation::ZhipinScanLogin,
                QrOperation::ZhipinDispatcher,
            ]
        );
    }

    #[tokio::test]
    async fn zhipin_never_returns_a_session_without_the_post_dispatcher_cookie() {
        let transport = MockTransport::new_after(
            vec![
                json_response(json!({"code":0,"zpData":{"qrId":"fixture-first"}})),
                json_response(json!({"scaned":true})),
                json_response(json!({"code":0,"zpData":{"qrId":"fixture-second"}})),
                json_response(json!({"scaned":true})),
                json_response(json!({"scaned":true,"login":"fixture-pk"})),
                json_response(json!({"code":0})),
            ],
            Vec::new(),
            None,
        );
        let result = attempt_with_transport(
            Platform::Zhipin,
            &transport,
            FlowConfig::immediate(1),
            |_| true,
        )
        .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn zhilian_requires_a_decoded_qr_poll_completion_and_both_cookies() {
        let transport = MockTransport::new_after(
            vec![
                json_response(json!({"data":{"path":"/qr.png","validateId":"fixture-id"}})),
                image_response(),
                json_response(json!({"code":0})),
            ],
            vec![("sou.zhaopin.com", "at=fixture-at; rt=fixture-rt")],
            Some(QrOperation::ZhilianPoll),
        );
        let result = attempt_with_transport(
            Platform::Zhilian,
            &transport,
            FlowConfig::immediate(1),
            |_| true,
        )
        .await;
        assert!(result.is_some());
        assert_eq!(
            transport.operations(),
            vec![
                QrOperation::ZhilianStart,
                QrOperation::ZhilianImage,
                QrOperation::ZhilianPoll,
            ]
        );
    }

    #[tokio::test]
    async fn qiancheng_accepts_a_changed_session_only_after_the_guarded_redirect() {
        let transport = MockTransport::new_changed_after(
            vec![
                QrResponse {
                    status: StatusCode::OK,
                    location: None,
                    body: b"trackConfig.guid = 'fixture-guid'; cfg.domain.www = 'https://we.51job.com/';"
                        .to_vec(),
                },
                json_response(json!({"status":1,"result":"/qr.png"})),
                image_response(),
                json_response(json!({"result":0,"error_code":"waiting"})),
                json_response(json!({"result":1,"zdparam":"fixture-zdparam"})),
                redirect_response("/next"),
                QrResponse {
                    status: StatusCode::OK,
                    location: None,
                    body: Vec::new(),
                },
            ],
            vec![("we.51job.com", "session=fixture-before")],
            vec![("we.51job.com", "session=fixture-after")],
            Some(QrOperation::QianchengRedirect),
        );
        let result = attempt_with_transport(
            Platform::Qiancheng,
            &transport,
            FlowConfig::immediate(2),
            |_| true,
        )
        .await;
        assert!(result.is_some());
        assert_eq!(
            transport.operations(),
            vec![
                QrOperation::QianchengPage,
                QrOperation::QianchengStart,
                QrOperation::QianchengImage,
                QrOperation::QianchengPoll,
                QrOperation::QianchengPoll,
                QrOperation::QianchengRedirect,
                QrOperation::QianchengRedirect,
            ]
        );
    }

    #[tokio::test]
    async fn qiancheng_never_returns_an_unchanged_preexisting_cookie_after_qr_completion() {
        let transport = MockTransport::new(
            vec![
                QrResponse {
                    status: StatusCode::OK,
                    location: None,
                    body: b"trackConfig.guid = 'fixture-guid'; cfg.domain.www = 'https://we.51job.com/';"
                        .to_vec(),
                },
                json_response(json!({"status":1,"result":"/qr.png"})),
                image_response(),
                json_response(json!({"result":1,"zdparam":"fixture-zdparam"})),
                QrResponse {
                    status: StatusCode::OK,
                    location: None,
                    body: Vec::new(),
                },
            ],
            vec![("we.51job.com", "session=fixture-unchanged")],
        );
        let result = attempt_with_transport(
            Platform::Qiancheng,
            &transport,
            FlowConfig::immediate(1),
            |_| true,
        )
        .await;
        assert!(result.is_none());
        assert_eq!(
            transport.operations(),
            vec![
                QrOperation::QianchengPage,
                QrOperation::QianchengStart,
                QrOperation::QianchengImage,
                QrOperation::QianchengPoll,
                QrOperation::QianchengRedirect,
            ]
        );
    }

    #[tokio::test]
    async fn qiancheng_unknown_poll_state_fails_closed_without_a_redirect() {
        let transport = MockTransport::new(
            vec![
                QrResponse {
                    status: StatusCode::OK,
                    location: None,
                    body: b"trackConfig.guid = 'fixture-guid'; cfg.domain.www = 'https://we.51job.com/';"
                        .to_vec(),
                },
                json_response(json!({"status":1,"result":"/qr.png"})),
                image_response(),
                json_response(json!({"result":0,"error_code":"unknown"})),
            ],
            Vec::new(),
        );
        let result = attempt_with_transport(
            Platform::Qiancheng,
            &transport,
            FlowConfig::immediate(1),
            |_| true,
        )
        .await;
        assert!(result.is_none());
        assert_eq!(
            transport.operations(),
            vec![
                QrOperation::QianchengPage,
                QrOperation::QianchengStart,
                QrOperation::QianchengImage,
                QrOperation::QianchengPoll,
            ]
        );
    }
}
