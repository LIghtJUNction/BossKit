//! Browser-backed BOSS 直聘 actions that are not exposed by the browserless API.
//!
//! This module deliberately talks only to a local ChromeDriver instance.  The
//! BOSS session Cookie is kept in memory, injected into a fresh browser
//! session, and never returned or logged.  The exchange result is verified by
//! observing the platform's exchange card, without extracting the WeChat ID.

use reqwest::Method;
use reqwest::blocking::{Client, RequestBuilder};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

use crate::BossError;
use crate::auth::{read_manual_sms_code, validate_phone};

const DEFAULT_DRIVER_URL: &str = "http://127.0.0.1:9515";
const BOSS_HOME: &str = "https://www.zhipin.com/";
const BOSS_USER: &str = "https://www.zhipin.com/web/user/";
const BOSS_JOBS: &str = "https://www.zhipin.com/web/geek/jobs";
const BOSS_CHAT: &str = "https://www.zhipin.com/web/geek/chat";
const SECURITY_SEED: &str = "ttttZij2JIIK+xUw73+6ZmzsaYKTbDQuIH6OR6Bm54o=";
const SECURITY_NAME: &str = "e331459e";
const MAX_COOKIE_PAIRS: usize = 64;
const MAX_RESPONSE_CHARS: usize = 64 * 1024;
const WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const REPAIR_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub(crate) struct BrowserExchangeResult {
    pub(crate) state: &'static str,
    pub(crate) verification: &'static str,
}

#[derive(Debug)]
pub(crate) struct BrowserPhoneLoginResult {
    pub(crate) cookie: String,
    pub(crate) verification: &'static str,
}

#[derive(Debug)]
pub(crate) struct BrowserSessionRepairResult {
    pub(crate) cookie: String,
    pub(crate) verification: &'static str,
}

struct Driver {
    client: Client,
    base_url: Url,
    session_id: String,
}

impl Driver {
    fn connect() -> Result<Self, BossError> {
        Self::connect_inner(false)
    }

    fn connect_attached() -> Result<Self, BossError> {
        if std::env::var_os("BOSS_CHROMEDRIVER_DEBUGGER_ADDRESS").is_none() {
            return Err(BossError::Authentication(
                "BOSS session repair requires an already logged-in local Chrome; set BOSS_CHROMEDRIVER_DEBUGGER_ADDRESS"
                    .to_owned(),
            ));
        }
        Self::connect_inner(true)
    }

    fn connect_inner(require_attached: bool) -> Result<Self, BossError> {
        let raw = std::env::var("BOSS_CHROMEDRIVER_URL")
            .unwrap_or_else(|_| DEFAULT_DRIVER_URL.to_owned());
        let base_url = validate_driver_url(&raw)?;
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| {
                BossError::Network("unable to initialize ChromeDriver client".to_owned())
            })?;

        let headless = std::env::var("BOSS_CHROMEDRIVER_HEADLESS")
            .map(|value| value != "0")
            .unwrap_or(false);
        let args = vec![
            "--disable-gpu".to_owned(),
            "--window-size=1440,1000".to_owned(),
        ];
        let mut chrome_options = json!({"args": args});
        if let Ok(raw_address) = std::env::var("BOSS_CHROMEDRIVER_DEBUGGER_ADDRESS") {
            let address = validate_debugger_address(&raw_address)?;
            chrome_options["debuggerAddress"] = Value::String(address);
        } else if require_attached {
            return Err(BossError::Authentication(
                "BOSS session repair requires an attached local Chrome debugger".to_owned(),
            ));
        }
        if let Ok(raw_dir) = std::env::var("BOSS_CHROMEDRIVER_USER_DATA_DIR") {
            let user_data_dir = validate_user_data_dir(&raw_dir)?;
            chrome_options["args"]
                .as_array_mut()
                .expect("Chrome options args array")
                .push(Value::String(format!(
                    "--user-data-dir={}",
                    user_data_dir.display()
                )));
        }
        if headless {
            chrome_options["args"]
                .as_array_mut()
                .expect("Chrome options args array")
                .push(Value::String("--headless=new".to_owned()));
        }
        let response = request_json(
            &client,
            &base_url,
            Method::POST,
            "/session",
            json!({
                "capabilities": {
                    "alwaysMatch": {
                        "browserName": "chrome",
                        "goog:chromeOptions": chrome_options
                    }
                }
            }),
        )?;
        let session_id = response
            .get("sessionId")
            .and_then(Value::as_str)
            .or_else(|| {
                response
                    .get("value")
                    .and_then(|value| value.get("sessionId"))
                    .and_then(Value::as_str)
            })
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BossError::Authentication("ChromeDriver did not return a session".to_owned())
            })?
            .to_owned();
        Ok(Self {
            client,
            base_url,
            session_id,
        })
    }

    fn path(&self, suffix: &str) -> String {
        format!("/session/{}{suffix}", self.session_id)
    }

    fn command(&self, method: Method, suffix: &str, body: Value) -> Result<Value, BossError> {
        request_json(
            &self.client,
            &self.base_url,
            method,
            &self.path(suffix),
            body,
        )
    }

    fn navigate(&self, url: &str) -> Result<(), BossError> {
        self.command(Method::POST, "/url", json!({"url": url}))?;
        Ok(())
    }

    fn add_cookie(&self, name: &str, value: &str) -> Result<(), BossError> {
        self.command(
            Method::POST,
            "/cookie",
            json!({"cookie":{"name":name,"value":value,"domain":".zhipin.com","path":"/","secure":true}}),
        )?;
        Ok(())
    }

    fn script(&self, script: &str, args: &[Value]) -> Result<Value, BossError> {
        let response = self.command(
            Method::POST,
            "/execute/sync",
            json!({"script":script,"args":args}),
        )?;
        Ok(response.get("value").cloned().unwrap_or(Value::Null))
    }

    fn cookies(&self) -> Result<String, BossError> {
        let response = self.command(Method::GET, "/cookie", Value::Null)?;
        let values = response
            .get("value")
            .and_then(Value::as_array)
            .ok_or_else(|| BossError::Parse("ChromeDriver returned invalid cookies".to_owned()))?;
        let mut pairs = Vec::new();
        for value in values {
            let Some(name) = value.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(cookie) = value.get("value").and_then(Value::as_str) else {
                continue;
            };
            if name.is_empty()
                || cookie.is_empty()
                || name.chars().any(char::is_control)
                || cookie.chars().any(char::is_control)
            {
                continue;
            }
            pairs.push(format!("{name}={cookie}"));
            if pairs.len() > MAX_COOKIE_PAIRS {
                return Err(BossError::Authentication(
                    "BOSS browser returned too many cookies".to_owned(),
                ));
            }
        }
        let cookie = pairs.join("; ");
        parse_cookie(&cookie).map(|_| cookie)
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _ = request_json(
            &self.client,
            &self.base_url,
            Method::DELETE,
            &self.path(""),
            Value::Null,
        );
    }
}

/// Completes the BOSS phone/SMS login flow through the visible login page.
///
/// The phone and SMS code stay in this process and are never returned to the
/// caller. The page's own JavaScript performs current encryption and security
/// checks; this function does not emulate or bypass those checks.
pub(crate) fn phone_login(phone: &str) -> Result<BrowserPhoneLoginResult, BossError> {
    let phone = validate_phone(phone)?;
    let driver = Driver::connect()?;
    driver.navigate(BOSS_USER)?;

    let loaded = wait_until(&driver, |driver| {
        driver.script(
            r#"const input = document.querySelector('input[name="phone"], input[type="tel"], input[placeholder*="手机号"]');
const button = Array.from(document.querySelectorAll("button, a")).find((element) => {
  const text = (element.textContent || "").replace(/\s+/g, "");
  return /获取验证码|发送验证码/.test(text);
});
return {ready: document.readyState === "complete" && Boolean(input) && Boolean(button)};"#,
            &[],
        )
    })?;
    if !loaded
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS phone login page did not load a phone input".to_owned(),
        ));
    }

    let sent = driver.script(
        r#"const input = document.querySelector('input[name="phone"], input[type="tel"], input[placeholder*="手机号"]');
const setValue = (element, value) => {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (setter) setter.call(element, value); else element.value = value;
  element.dispatchEvent(new Event("input", {bubbles: true}));
  element.dispatchEvent(new Event("change", {bubbles: true}));
};
if (!input) return {sent:false, reason:"phone_input_missing"};
setValue(input, arguments[0]);
const button = Array.from(document.querySelectorAll("button, a")).find((element) => {
  const text = (element.textContent || "").replace(/\s+/g, "");
  return /获取验证码|发送验证码/.test(text);
});
if (!button) return {sent:false, reason:"sms_button_missing"};
if (button.disabled || /disabled|forbid/i.test(String(button.className || ""))) {
  return {sent:false, reason:"sms_button_disabled"};
}

button.click();
return {sent:true};"#,
        &[Value::String(phone)],
    )?;
    if !sent.get("sent").and_then(Value::as_bool).unwrap_or(false) {
        return Err(BossError::Authentication(
            "BOSS did not expose an enabled SMS-code action".to_owned(),
        ));
    }

    let code = read_manual_sms_code()?.ok_or_else(|| {
        BossError::Authentication(
            "terminal SMS-code input is required; rerun login --phone from a TTY".to_owned(),
        )
    })?;
    let submitted = driver.script(
        r#"const input = document.querySelector('input[name="phoneCode"], input[placeholder*="验证码"]');
const setValue = (element, value) => {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (setter) setter.call(element, value); else element.value = value;
  element.dispatchEvent(new Event("input", {bubbles: true}));
  element.dispatchEvent(new Event("change", {bubbles: true}));
};
if (!input) return {submitted:false};
setValue(input, arguments[0]);
const button = Array.from(document.querySelectorAll("button, a")).find((element) => {
  const text = (element.textContent || "").replace(/\s+/g, "");
  return /登录|注册/.test(text) && !/验证码/.test(text);
});
if (!button || button.disabled) return {submitted:false};
button.click();
return {submitted:true};"#,
        &[Value::String(code)],
    )?;
    if !submitted
        .get("submitted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS login page did not accept the SMS code".to_owned(),
        ));
    }

    let logged_in = wait_until(&driver, |driver| {
        driver.script(
            r#"const path = location.pathname || "";
const pageText = (document.body?.innerText || "").replace(/\s+/g, "");
const loggedIn = !/\/web\/user\/?$/.test(path) && !/验证码错误|登录失败|安全验证/.test(pageText);
return {logged_in: loggedIn, pathname: path};"#,
            &[],
        )
    })?;
    if !logged_in
        .get("logged_in")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS phone login was not verified; complete any visible security check and retry"
                .to_owned(),
        ));
    }
    let cookie = driver.cookies()?;
    crate::auth::validate_cookie(&cookie)?;
    Ok(BrowserPhoneLoginResult {
        cookie,
        verification: "visible_phone_sms_login",
    })
}

/// Refreshes the session-bound security cookie in a local browser context.
///
/// The browser performs the current BOSS security-check JavaScript. The
/// returned cookie is only accepted after the page leaves the login/security
/// routes and contains a freshly minted `__zp_stoken__`; the caller still
/// performs the browserless authenticated API probe before persisting it.
pub(crate) fn repair_session(cookie: &str) -> Result<BrowserSessionRepairResult, BossError> {
    let pairs = parse_cookie(cookie)?;
    let stored_wt2 = cookie_value(&pairs, "wt2").ok_or_else(|| {
        BossError::Authentication(
            "stored BOSS session is missing its primary session cookie".to_owned(),
        )
    })?;
    let driver = Driver::connect_attached()?;
    driver.navigate(BOSS_HOME)?;

    let attached_pairs = parse_cookie(&driver.cookies()?)?;
    if cookie_value(&attached_pairs, "wt2") != Some(stored_wt2) {
        return Err(BossError::Authentication(
            "attached Chrome session does not match the stored BOSS account".to_owned(),
        ));
    }

    driver.navigate(&security_check_url()?)?;
    let loaded = wait_until_for(
        &driver,
        |driver| {
            driver.script(
                r#"return {ready: document.readyState === "complete"};"#,
                &[],
            )
        },
        REPAIR_WAIT_TIMEOUT,
    )?;
    if !loaded
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS security-check page did not finish loading".to_owned(),
        ));
    }
    let token_ready = wait_until_for(
        &driver,
        |driver| {
            let browser_cookie = driver.cookies()?;
            let has_stoken = parse_cookie(&browser_cookie).ok().is_some_and(|pairs| {
                cookie_value(&pairs, "__zp_stoken__").is_some_and(|value| !value.is_empty())
            });
            Ok(json!({"ready":has_stoken}))
        },
        REPAIR_WAIT_TIMEOUT,
    )?;
    if !token_ready
        .get("ready")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS security-check did not complete; finish the visible browser verification and retry"
                .to_owned(),
        ));
    }

    driver.navigate(BOSS_JOBS)?;
    let logged_in = wait_until_for(
        &driver,
        |driver| {
            driver.script(
                r#"const path = location.pathname || "";
const blocked = /\/web\/user|\/login|\/passport|安全验证|验证码错误/.test(path + " " + (document.body?.innerText || ""));
return {logged_in: !blocked && document.readyState === "complete", pathname: path};"#,
                &[],
            )
        },
        REPAIR_WAIT_TIMEOUT,
    )?;
    if !logged_in
        .get("logged_in")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS browser session still requires login or visible verification".to_owned(),
        ));
    }

    let refreshed_cookie = driver.cookies()?;
    let refreshed_pairs = parse_cookie(&refreshed_cookie)?;
    if !refreshed_pairs
        .iter()
        .any(|(name, value)| name == "__zp_stoken__" && !value.is_empty())
    {
        return Err(BossError::Authentication(
            "BOSS browser did not mint a session security token".to_owned(),
        ));
    }
    crate::auth::validate_cookie(&refreshed_cookie)?;
    Ok(BrowserSessionRepairResult {
        cookie: refreshed_cookie,
        verification: "browser_security_check_stoken",
    })
}

fn security_check_url() -> Result<String, BossError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| BossError::Authentication("system clock is before UNIX epoch".to_owned()))?
        .as_millis();
    Ok(format!(
        "https://www.zhipin.com/web/common/security-check.html?seed={SECURITY_SEED}&name={SECURITY_NAME}&ts={timestamp}&callbackUrl=https%3A%2F%2Fwww.zhipin.com%2Fweb%2Fgeek%2Fjobs"
    ))
}

fn cookie_value<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(cookie_name, _)| cookie_name == name)
        .map(|(_, value)| value.as_str())
}

/// Exchanges WeChat through the platform's visible chat action.
pub(crate) fn exchange_wechat(
    cookie: &str,
    title: &str,
    company: &str,
) -> Result<BrowserExchangeResult, BossError> {
    validate_text(title, "job title")?;
    validate_text(company, "company")?;
    let pairs = parse_cookie(cookie)?;
    let driver = Driver::connect()?;
    driver.navigate(BOSS_HOME)?;
    for (name, value) in pairs {
        driver.add_cookie(&name, &value)?;
    }
    driver.navigate(BOSS_CHAT)?;

    wait_until(&driver, |driver| {
        driver.script(r#"return document.readyState === "complete";"#, &[])
    })?;
    let opened = wait_until(&driver, |driver| {
        driver.script(
            r#"const title = arguments[0];
const company = arguments[1];
const items = Array.from(document.querySelectorAll('.chat-item, .geek-item, .geek-item-wrap, .chat-item-content, [role="listitem"]'));
const target = items.find((item) => {
  const text = item.textContent || '';
  return title && text.includes(title);
}) || items.find((item) => {
  const text = item.textContent || '';
  return company && text.includes(company);
});
if (!target) return {"found":false,"count":items.length,"pathname":location.pathname,"body_length":(document.body && document.body.innerText || '').length};
target.scrollIntoView({block:'center'});
target.click();
return {"found":true};"#,
            &[
                Value::String(title.to_owned()),
                Value::String(company.to_owned()),
            ],
        )
    })?;
    if !opened
        .get("found")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if opened
            .get("pathname")
            .and_then(Value::as_str)
            .is_some_and(|path| path.contains("/passport/zp/verify"))
        {
            return Err(BossError::Authentication(
                "BOSS browser session requires verification; attach an already logged-in Chrome session with BOSS_CHROMEDRIVER_DEBUGGER_ADDRESS".to_owned(),
            ));
        }
        return Err(BossError::Authentication(format!(
            "BOSS chat target was not found in the browser session (path={}, candidates={}, body_length={})",
            opened
                .get("pathname")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            opened.get("count").and_then(Value::as_u64).unwrap_or(0),
            opened
                .get("body_length")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )));
    }

    wait_until(&driver, |driver| {
        driver.script(
            r#"const conversation = document.querySelector('.base-info-single-container, .chat-conversation, .conversation-box');
return {"ready":Boolean(conversation)};"#,
            &[],
        )
    })?;

    let before = exchange_card_count(&driver)?;
    let clicked = wait_until(&driver, |driver| {
        driver.script(
            r#"const norm = (value) => (value || '').replace(/\s+/g, '');
const items = Array.from(document.querySelectorAll('.operate-exchange-left .operate-icon-item, .operate-icon-item'));
const target = items.find((item) => norm(item.querySelector('.operate-btn')?.textContent || item.textContent).includes('换微信'));
if (!target) return {"clicked":false,"found":false,"button_count":items.length,"pathname":location.pathname};
const button = target.querySelector('.operate-btn') || target;
const className = `${target.className || ''} ${button.className || ''}`;
const disabled = /disabled|forbid|ban/i.test(className) || button.hasAttribute('disabled');
if (disabled) return {"clicked":false,"found":true,"disabled":true,"button_count":items.length,"pathname":location.pathname};
target.scrollIntoView({block:'center',inline:'nearest'});
button.click();
return {"clicked":true,"found":true};"#,
            &[],
        )
    })?;
    if !clicked
        .get("clicked")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        if before > 0 {
            return Ok(BrowserExchangeResult {
                state: "already_exchanged",
                verification: "visible_exchange_card",
            });
        }
        if clicked
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(BossError::Authentication(
                "BOSS exchange-WeChat action is currently disabled for this conversation"
                    .to_owned(),
            ));
        }
        return Err(BossError::Authentication(format!(
            "BOSS chat did not expose the exchange-WeChat action (path={}, buttons={})",
            clicked
                .get("pathname")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            clicked
                .get("button_count")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )));
    }

    let confirmed = wait_until(&driver, |driver| {
        driver.script(
            r#"const visible = (element) => {
  if (!element || element.offsetParent === null) return false;
  const rect = element.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0;
};
const tooltip = Array.from(document.querySelectorAll('.exchange-tooltip'))
  .find((element) => visible(element) && (element.textContent || '').replace(/\s+/g, '').includes('交换微信'));
if (!tooltip) return {"confirmed":false};
const button = Array.from(tooltip.querySelectorAll('.boss-btn-primary, .boss-btn'))
  .find((element) => (element.textContent || '').includes('确定'));
if (!button) return {"confirmed":false};
button.click();
return {"confirmed":true};"#,
            &[],
        )
    })?;
    if !confirmed
        .get("confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS exchange-WeChat confirmation dialog was not verified".to_owned(),
        ));
    }

    let after = wait_until(&driver, |driver| {
        let count = exchange_card_count(driver)?;
        Ok(json!({"verified": count > before}))
    })?;
    if !after
        .get("verified")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(BossError::Authentication(
            "BOSS exchange-WeChat result was not verified in chat".to_owned(),
        ));
    }
    Ok(BrowserExchangeResult {
        state: "exchange_verified",
        verification: "visible_exchange_card_after_confirm",
    })
}

fn exchange_card_count(driver: &Driver) -> Result<usize, BossError> {
    let value = driver.script(
        r#"return document.querySelectorAll('.message-card-top-wrap, [class*="d-top-text"]').length;"#,
        &[],
    )?;
    value
        .as_u64()
        .map(|count| count as usize)
        .ok_or_else(|| BossError::Parse("BOSS exchange card count was invalid".to_owned()))
}

fn wait_until<F>(driver: &Driver, mut operation: F) -> Result<Value, BossError>
where
    F: FnMut(&Driver) -> Result<Value, BossError>,
{
    wait_until_for(driver, &mut operation, WAIT_TIMEOUT)
}

fn wait_until_for<F>(
    driver: &Driver,
    mut operation: F,
    timeout: Duration,
) -> Result<Value, BossError>
where
    F: FnMut(&Driver) -> Result<Value, BossError>,
{
    let deadline = Instant::now() + timeout;
    loop {
        let value = operation(driver)?;
        if value.get("found").and_then(Value::as_bool).unwrap_or(false)
            || value
                .get("clicked")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            || value
                .get("confirmed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            || value.get("ready").and_then(Value::as_bool).unwrap_or(false)
            || value
                .get("verified")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            || value.get("sent").and_then(Value::as_bool).unwrap_or(false)
            || value
                .get("logged_in")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            || value.as_bool().unwrap_or(false)
        {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Ok(value);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn validate_driver_url(raw: &str) -> Result<Url, BossError> {
    let url = Url::parse(raw)
        .map_err(|_| BossError::InvalidArgument("BOSS_CHROMEDRIVER_URL is invalid".to_owned()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
    {
        return Err(BossError::InvalidArgument(
            "BOSS_CHROMEDRIVER_URL must point to localhost".to_owned(),
        ));
    }
    Ok(url)
}

fn validate_user_data_dir(raw: &str) -> Result<PathBuf, BossError> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() || raw.chars().any(char::is_control) {
        return Err(BossError::InvalidArgument(
            "BOSS_CHROMEDRIVER_USER_DATA_DIR must be an absolute local path".to_owned(),
        ));
    }
    Ok(path)
}

fn validate_debugger_address(raw: &str) -> Result<String, BossError> {
    let address = raw.trim();
    let (host, port) = address.rsplit_once(':').ok_or_else(|| {
        BossError::InvalidArgument(
            "BOSS_CHROMEDRIVER_DEBUGGER_ADDRESS must be localhost:port".to_owned(),
        )
    })?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
        || port.parse::<u16>().ok().filter(|port| *port > 0).is_none()
    {
        return Err(BossError::InvalidArgument(
            "BOSS_CHROMEDRIVER_DEBUGGER_ADDRESS must be localhost:port".to_owned(),
        ));
    }
    Ok(address.to_owned())
}

fn parse_cookie(cookie: &str) -> Result<Vec<(String, String)>, BossError> {
    let mut pairs = Vec::new();
    for raw in cookie.split(';') {
        let Some((name, value)) = raw.trim().split_once('=') else {
            continue;
        };
        if name.is_empty() || value.is_empty() || name.chars().any(|c| c.is_control()) {
            continue;
        }
        if value.chars().any(|c| c.is_control()) {
            return Err(BossError::Authentication(
                "stored BOSS session contains invalid cookie data".to_owned(),
            ));
        }
        pairs.push((name.to_owned(), value.to_owned()));
        if pairs.len() > MAX_COOKIE_PAIRS {
            return Err(BossError::Authentication(
                "stored BOSS session contains too many cookies".to_owned(),
            ));
        }
    }
    if pairs.is_empty() {
        return Err(BossError::Authentication(
            "stored BOSS session contains no usable cookies".to_owned(),
        ));
    }
    Ok(pairs)
}

fn validate_text(value: &str, label: &str) -> Result<(), BossError> {
    if value.trim().is_empty() || value.chars().count() > 200 || value.chars().any(char::is_control)
    {
        return Err(BossError::InvalidArgument(format!("{label} is invalid")));
    }
    Ok(())
}

fn request_json(
    client: &Client,
    base_url: &Url,
    method: Method,
    path: &str,
    body: Value,
) -> Result<Value, BossError> {
    let url = base_url
        .join(path.trim_start_matches('/'))
        .map_err(|_| BossError::Network("ChromeDriver URL could not be resolved".to_owned()))?;
    let request: RequestBuilder = client
        .request(method, url)
        .header("content-type", "application/json");
    let response = request.json(&body).send().map_err(|_| {
        BossError::Network(format!(
            "ChromeDriver request failed at {}",
            endpoint_label(path)
        ))
    })?;
    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|_| BossError::Network("ChromeDriver response could not be read".to_owned()))?;
    if bytes.len() > MAX_RESPONSE_CHARS {
        return Err(BossError::Parse(
            "ChromeDriver response was too large".to_owned(),
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| BossError::Parse("ChromeDriver response was not JSON".to_owned()))?;
    if !status.is_success() {
        return Err(BossError::Network(
            "ChromeDriver rejected the request".to_owned(),
        ));
    }
    if value
        .get("value")
        .and_then(|value| value.get("error"))
        .is_some_and(|error| error.is_string() || error.is_object())
    {
        return Err(BossError::Authentication(
            "ChromeDriver reported a browser action failure".to_owned(),
        ));
    }
    Ok(value)
}

fn endpoint_label(path: &str) -> &'static str {
    if path == "/session" {
        "session"
    } else if path.ends_with("/url") {
        "navigation"
    } else if path.ends_with("/cookie") {
        "cookie-injection"
    } else if path.ends_with("/execute/sync") {
        "script"
    } else {
        "session-cleanup"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_url_is_local_only() {
        assert!(validate_driver_url("http://127.0.0.1:9515").is_ok());
        assert!(validate_driver_url("http://10.0.0.2:9515").is_err());
        assert!(validate_driver_url("https://example.test").is_err());
    }

    #[test]
    fn user_data_dir_must_be_absolute_and_local() {
        assert!(validate_user_data_dir("/tmp/bosskit-profile").is_ok());
        assert!(validate_user_data_dir("relative-profile").is_err());
    }

    #[test]
    fn debugger_address_must_be_local() {
        assert!(validate_debugger_address("127.0.0.1:9222").is_ok());
        assert!(validate_debugger_address("example.test:9222").is_err());
    }

    #[test]
    fn cookie_parser_never_returns_empty_or_too_many_pairs() {
        assert!(parse_cookie("wt2=secret; bst=token").is_ok());
        assert!(parse_cookie("not-a-cookie").is_err());
        let too_many = (0..=MAX_COOKIE_PAIRS)
            .map(|index| format!("k{index}=v"))
            .collect::<Vec<_>>()
            .join(";");
        assert!(parse_cookie(&too_many).is_err());
    }

    #[test]
    fn security_check_url_is_locally_constructed_with_fresh_timestamp() {
        let url = security_check_url().expect("security check URL");
        assert!(url.starts_with("https://www.zhipin.com/web/common/security-check.html?"));
        assert!(url.contains("name=e331459e"));
        assert!(url.contains("callbackUrl=https%3A%2F%2Fwww.zhipin.com%2Fweb%2Fgeek%2Fjobs"));
        assert!(url.contains("ts=") && !url.ends_with("ts="));
    }
}
