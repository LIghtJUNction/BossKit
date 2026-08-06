//! Private local Cookie session storage.
//!
//! This module never contacts a recruitment platform or reads Cookie export
//! files. It stores only validated local sessions and never exposes values.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::Write;
use std::io::{self, IsTerminal, Read};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::data::atomic_write_private;
use crate::{BossError, DataPaths, Platform};

const MAX_COOKIE_BYTES: usize = 16 * 1024;
const MAX_AUTH_STORE_BYTES: u64 = 64 * 1024;
const MAX_ACCOUNTS: usize = 32;
pub(crate) const MAX_ACCOUNT_ALIAS_CHARS: usize = 32;
pub(crate) const DEFAULT_ACCOUNT: &str = "default";

/// The only supported BOSS account surfaces. This is safe local metadata.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZhipinRole {
    #[default]
    Geek,
    Recruiter,
}

impl ZhipinRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Geek => "geek",
            Self::Recruiter => "recruiter",
        }
    }
}

/// Safe, value-free health of the private credential store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthHealth {
    /// No private store has been created yet.
    Missing,
    /// The private store is readable and has safe permissions.
    Ready,
    /// The private store is malformed or unsafe and was ignored.
    Unavailable,
}

impl AuthHealth {
    /// Returns the stable, safe diagnostic value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Value-free health of a stored Zhipin session.
///
/// This intentionally exposes only the presence of cookie classes. Cookie
/// values, names, and paths never leave the private auth store.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct SessionHealth {
    pub(crate) cookie_present: bool,
    pub(crate) primary_cookie_present: bool,
    pub(crate) stoken_present: bool,
    pub(crate) auxiliary_cookie_present: bool,
    pub(crate) state: &'static str,
    pub(crate) next_action: &'static str,
}

impl SessionHealth {
    const MISSING: Self = Self {
        cookie_present: false,
        primary_cookie_present: false,
        stoken_present: false,
        auxiliary_cookie_present: false,
        state: "missing",
        next_action: "boss login",
    };
}

/// Local private Cookie sessions.
#[derive(Clone, Debug)]
pub struct AuthStore {
    directory: PathBuf,
    path: PathBuf,
    document: AuthDocument,
    active_account: String,
    health: AuthHealth,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AuthDocument {
    #[serde(default = "default_account_alias")]
    selected_account: String,
    #[serde(default = "default_accounts")]
    accounts: BTreeMap<String, AccountAuth>,
}

impl Default for AuthDocument {
    fn default() -> Self {
        Self {
            selected_account: default_account_alias(),
            accounts: default_accounts(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AccountAuth {
    #[serde(default)]
    role: ZhipinRole,
    #[serde(default)]
    zhipin: StoredAuth,
    #[serde(
        default,
        rename = "zhilian",
        deserialize_with = "discard_legacy_entry",
        skip_serializing
    )]
    _legacy_zhilian: (),
    #[serde(
        default,
        rename = "qiancheng",
        deserialize_with = "discard_legacy_entry",
        skip_serializing
    )]
    _legacy_qiancheng: (),
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyAuthDocument {
    #[serde(default)]
    zhipin: StoredAuth,
    #[serde(default, rename = "zhilian", deserialize_with = "discard_legacy_entry")]
    _legacy_zhilian: (),
    #[serde(
        default,
        rename = "qiancheng",
        deserialize_with = "discard_legacy_entry"
    )]
    _legacy_qiancheng: (),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SerializedAuthDocument {
    Current(AuthDocument),
    Legacy(LegacyAuthDocument),
}

fn discard_legacy_entry<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAuth {
    #[serde(default)]
    cookie: Option<String>,
    #[serde(
        default,
        rename = "credential_file",
        deserialize_with = "discard_legacy_credential_file",
        skip_serializing
    )]
    legacy_credential_file: bool,
}

fn discard_legacy_credential_file<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

fn default_account_alias() -> String {
    DEFAULT_ACCOUNT.to_owned()
}

fn default_accounts() -> BTreeMap<String, AccountAuth> {
    BTreeMap::from([(DEFAULT_ACCOUNT.to_owned(), AccountAuth::default())])
}

impl AuthStore {
    /// Opens a private store without making ordinary application startup fail.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        let directory = paths.auth_dir();
        let path = paths.auth_sessions();
        let (document, health, migration_required) = match load_document(&directory, &path) {
            Ok(Some((document, migration_required))) if document_is_valid(&document) => {
                (document, AuthHealth::Ready, migration_required)
            }
            Ok(Some(_)) | Err(()) => (AuthDocument::default(), AuthHealth::Unavailable, false),
            Ok(None) => (AuthDocument::default(), AuthHealth::Missing, false),
        };
        let active_account = document.selected_account.clone();
        let mut store = Self {
            directory,
            path,
            document,
            active_account,
            health,
        };
        if (migration_required || store.has_legacy_credential_file()) && store.persist().is_err() {
            store.document = AuthDocument::default();
            store.active_account = default_account_alias();
            store.health = AuthHealth::Unavailable;
        }
        store
    }

    /// Returns the safe local store health.
    #[must_use]
    pub const fn health(&self) -> AuthHealth {
        self.health
    }

    /// Returns the environment variable used for one platform.
    #[must_use]
    pub const fn cookie_env(platform: Platform) -> &'static str {
        match platform {
            Platform::Zhipin => "BOSS_ZHIPIN_COOKIE",
            Platform::Zhilian | Platform::Qiancheng => "BOSS_ZHIPIN_COOKIE",
        }
    }

    /// Returns a validated environment Cookie without printing it.
    #[must_use]
    pub fn environment_cookie(platform: Platform) -> Option<String> {
        std::env::var(Self::cookie_env(platform))
            .ok()
            .filter(|value| validate_cookie(value).is_ok())
    }

    /// Selects a validated account for this process without changing the saved default.
    pub(crate) fn select_runtime_account(&mut self, alias: &str) -> Result<(), BossError> {
        validate_account_alias(alias)?;
        self.active_account = alias.to_owned();
        Ok(())
    }

    /// Returns the account selected for this process.
    #[must_use]
    pub(crate) fn active_account(&self) -> &str {
        &self.active_account
    }

    /// Returns the persisted account used when no runtime override is supplied.
    #[must_use]
    pub(crate) fn default_account(&self) -> &str {
        &self.document.selected_account
    }

    /// Lists safe account aliases in stable order.
    #[must_use]
    pub(crate) fn account_aliases(&self) -> Vec<&str> {
        self.document.accounts.keys().map(String::as_str).collect()
    }

    #[must_use]
    pub(crate) fn account_role(&self, alias: &str) -> ZhipinRole {
        self.document
            .accounts
            .get(alias)
            .map_or(ZhipinRole::Geek, |account| account.role)
    }

    #[must_use]
    pub(crate) fn active_role(&self) -> ZhipinRole {
        self.account_role(&self.active_account)
    }

    /// Creates or selects the persisted default account.
    pub(crate) fn use_account(&mut self, alias: &str) -> Result<(), BossError> {
        validate_account_alias(alias)?;
        if !self.document.accounts.contains_key(alias) {
            if self.document.accounts.len() >= MAX_ACCOUNTS {
                return Err(BossError::InvalidArgument(format!(
                    "account limit must not exceed {MAX_ACCOUNTS}"
                )));
            }
            self.document
                .accounts
                .insert(alias.to_owned(), AccountAuth::default());
        }
        self.document.selected_account = alias.to_owned();
        self.active_account = alias.to_owned();
        self.persist()
    }

    /// Returns whether the active account may consume generic environment Cookies.
    #[must_use]
    pub(crate) fn allows_environment_cookie(&self) -> bool {
        self.active_account == DEFAULT_ACCOUNT
    }

    /// Returns a validated generic environment Cookie only for the default account.
    #[must_use]
    pub(crate) fn active_environment_cookie(&self, platform: Platform) -> Option<String> {
        self.allows_environment_cookie()
            .then(|| Self::environment_cookie(platform))
            .flatten()
    }

    /// Resolves the normal provider runtime order: environment, then session.
    #[must_use]
    pub fn runtime_cookie(&self, platform: Platform) -> Option<String> {
        self.active_environment_cookie(platform)
            .or_else(|| self.session_cookie(platform))
    }

    /// Returns whether a local session exists for one platform.
    #[must_use]
    pub fn has_session(&self, platform: Platform) -> bool {
        self.session_cookie(platform).is_some()
    }

    /// Returns safe, local-only health for the active account's stored session.
    #[must_use]
    pub(crate) fn session_health(&self, platform: Platform) -> SessionHealth {
        let Some(cookie) = self.session_cookie(platform) else {
            return SessionHealth::MISSING;
        };
        let mut primary_cookie_present = false;
        let mut stoken_present = false;
        let mut auxiliary_cookie_present = false;
        for part in cookie.split(';') {
            let Some((name, _)) = part.trim().split_once('=') else {
                continue;
            };
            match name.trim() {
                "wt2" => primary_cookie_present = true,
                "__zp_stoken__" => stoken_present = true,
                "wbg" | "zp_at" => auxiliary_cookie_present = true,
                _ => {}
            }
        }
        let ready = primary_cookie_present;
        SessionHealth {
            cookie_present: true,
            primary_cookie_present,
            stoken_present,
            auxiliary_cookie_present,
            state: if ready { "ready" } else { "partial" },
            next_action: if ready { "none" } else { "boss login" },
        }
    }

    /// Returns whether one named account has a saved session.
    #[must_use]
    pub(crate) fn account_has_session(&self, alias: &str, platform: Platform) -> bool {
        self.document
            .accounts
            .get(alias)
            .and_then(|account| account.entry(platform).cookie.as_ref())
            .is_some()
    }

    /// Stores one validated session.
    pub fn store_session(&mut self, platform: Platform, cookie: String) -> Result<(), BossError> {
        validate_cookie(&cookie)?;
        if !self.document.accounts.contains_key(&self.active_account) {
            if self.document.accounts.len() >= MAX_ACCOUNTS {
                return Err(auth_error("private credential account limit was exceeded"));
            }
            self.document
                .accounts
                .insert(self.active_account.clone(), AccountAuth::default());
        }
        let Some(account) = self.document.accounts.get_mut(&self.active_account) else {
            return Err(auth_error("unable to select private credential account"));
        };
        account.entry_mut(platform).cookie = Some(cookie);
        self.persist()
    }

    /// Stores a newly verified Cookie and its requested account role in one
    /// private-store update. Verification happens before this method is called.
    pub(crate) fn store_verified_login(
        &mut self,
        platform: Platform,
        cookie: String,
        role: ZhipinRole,
    ) -> Result<(), BossError> {
        validate_cookie(&cookie)?;
        let previous_document = self.document.clone();
        let previous_health = self.health;
        if !self.document.accounts.contains_key(&self.active_account) {
            if self.document.accounts.len() >= MAX_ACCOUNTS {
                return Err(auth_error("private credential account limit was exceeded"));
            }
            self.document
                .accounts
                .insert(self.active_account.clone(), AccountAuth::default());
        }
        let Some(account) = self.document.accounts.get_mut(&self.active_account) else {
            return Err(auth_error("unable to select private credential account"));
        };
        account.role = role;
        account.entry_mut(platform).cookie = Some(cookie);
        if let Err(error) = self.persist() {
            self.document = previous_document;
            self.health = previous_health;
            return Err(error);
        }
        Ok(())
    }

    /// Removes the saved session for one platform.
    pub fn revoke(&mut self, platform: Platform) -> Result<bool, BossError> {
        let Some(account) = self.document.accounts.get_mut(&self.active_account) else {
            return Ok(false);
        };
        let entry = account.entry_mut(platform);
        let changed = entry.cookie.take().is_some();
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    pub(crate) fn session_cookie(&self, platform: Platform) -> Option<String> {
        self.document
            .accounts
            .get(&self.active_account)
            .and_then(|account| account.entry(platform).cookie.clone())
    }

    fn has_legacy_credential_file(&self) -> bool {
        self.document
            .accounts
            .values()
            .any(|account| account.zhipin.legacy_credential_file)
    }

    fn persist(&mut self) -> Result<(), BossError> {
        ensure_private_directory(&self.directory)?;
        let bytes = serde_json::to_vec(&self.document)
            .map_err(|_| auth_error("unable to encode private credential store"))?;
        if bytes.len() as u64 > MAX_AUTH_STORE_BYTES {
            return Err(auth_error("private credential store is too large"));
        }
        atomic_write_private(&self.path, &bytes, |_| {
            auth_error("unable to write private credential store")
        })?;
        self.health = AuthHealth::Ready;
        Ok(())
    }
}

impl AccountAuth {
    fn entry(&self, platform: Platform) -> &StoredAuth {
        match platform {
            Platform::Zhipin => &self.zhipin,
            Platform::Zhilian | Platform::Qiancheng => &self.zhipin,
        }
    }

    fn entry_mut(&mut self, platform: Platform) -> &mut StoredAuth {
        match platform {
            Platform::Zhipin => &mut self.zhipin,
            Platform::Zhilian | Platform::Qiancheng => &mut self.zhipin,
        }
    }
}

/// Reads exactly one bounded Cookie from standard input without echoing it.
pub(crate) fn read_cookie_stdin() -> Result<String, BossError> {
    if io::stdin().is_terminal() {
        return Err(auth_error(
            "--cookie-stdin requires non-terminal standard input",
        ));
    }
    let mut input = Vec::with_capacity(MAX_COOKIE_BYTES.min(4096));
    Read::by_ref(&mut io::stdin().lock())
        .take(MAX_COOKIE_BYTES as u64 + 3)
        .read_to_end(&mut input)
        .map_err(|_| auth_error("unable to read credential input"))?;
    parse_cookie_stdin(&input)
}

fn parse_cookie_stdin(input: &[u8]) -> Result<String, BossError> {
    if input.len() > MAX_COOKIE_BYTES + 2 {
        return Err(auth_error("credential input is invalid"));
    }
    let input =
        std::str::from_utf8(input).map_err(|_| auth_error("credential input is invalid"))?;
    let cookie = input
        .strip_suffix("\r\n")
        .or_else(|| input.strip_suffix('\n'))
        .unwrap_or(input);
    if cookie.contains(['\r', '\n']) {
        return Err(auth_error("credential input is invalid"));
    }
    validate_cookie(cookie)?;
    Ok(cookie.to_owned())
}

/// Reads exactly one newly supplied Cookie from a TTY with echo disabled.
pub(crate) fn read_cookie_tty(platform: Platform) -> Result<String, BossError> {
    #[cfg(unix)]
    {
        if !io::stdin().is_terminal() {
            return Err(auth_error(
                "login requires hidden terminal Cookie input; use --cookie-stdin for piped input",
            ));
        }
        eprint!("Enter {} Cookie (input hidden): ", platform.display_name());
        io::stderr()
            .flush()
            .map_err(|_| auth_error("unable to prepare hidden credential input"))?;
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut original) } != 0 {
            return Err(auth_error("unable to configure hidden credential input"));
        }
        let mut hidden = original;
        hidden.c_lflag &= !libc::ECHO;
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &hidden) } != 0 {
            return Err(auth_error("unable to configure hidden credential input"));
        }
        let restore = EchoRestore { original };
        let mut input = String::new();
        let read_result = io::stdin().read_line(&mut input);
        drop(restore);
        eprintln!();
        read_result.map_err(|_| auth_error("unable to read hidden credential input"))?;
        let cookie = input.trim_end_matches(['\r', '\n']);
        validate_cookie(cookie)?;
        Ok(cookie.to_owned())
    }

    #[cfg(not(unix))]
    {
        let _ = platform;
        Err(auth_error(
            "hidden terminal Cookie input is unsupported on this platform; use --cookie-stdin",
        ))
    }
}

#[cfg(unix)]
struct EchoRestore {
    original: libc::termios,
}

#[cfg(unix)]
impl Drop for EchoRestore {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.original);
        }
    }
}

fn load_document(directory: &Path, path: &Path) -> Result<Option<(AuthDocument, bool)>, ()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if private_directory_metadata(&metadata) => {}
        Ok(_) => return Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    }
    let Some(bytes) = read_private_store_file(path)? else {
        return Ok(None);
    };
    let stored: SerializedAuthDocument = serde_json::from_slice(&bytes).map_err(|_| ())?;
    Ok(Some(match stored {
        SerializedAuthDocument::Current(document) => (document, false),
        SerializedAuthDocument::Legacy(legacy) => {
            let account = AccountAuth {
                zhipin: legacy.zhipin,
                ..AccountAuth::default()
            };
            (
                AuthDocument {
                    selected_account: default_account_alias(),
                    accounts: BTreeMap::from([(DEFAULT_ACCOUNT.to_owned(), account)]),
                },
                true,
            )
        }
    }))
}

#[cfg(unix)]
fn read_private_store_file(path: &Path) -> Result<Option<Vec<u8>>, ()> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let metadata = file.metadata().map_err(|_| ())?;
    if !private_file_metadata(&metadata) || metadata.len() > MAX_AUTH_STORE_BYTES {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_AUTH_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_AUTH_STORE_BYTES {
        return Err(());
    }
    Ok(Some(bytes))
}

#[cfg(not(unix))]
fn read_private_store_file(_path: &Path) -> Result<Option<Vec<u8>>, ()> {
    Err(())
}

fn document_is_valid(document: &AuthDocument) -> bool {
    document.accounts.len() <= MAX_ACCOUNTS
        && document.accounts.contains_key(DEFAULT_ACCOUNT)
        && document
            .accounts
            .contains_key(document.selected_account.as_str())
        && valid_account_alias(&document.selected_account)
        && document.accounts.iter().all(|(alias, account)| {
            valid_account_alias(alias)
                && account
                    .zhipin
                    .cookie
                    .as_deref()
                    .is_none_or(|cookie| validate_cookie(cookie).is_ok())
        })
}

pub(crate) fn validate_account_alias(alias: &str) -> Result<(), BossError> {
    if valid_account_alias(alias) {
        Ok(())
    } else {
        Err(BossError::InvalidArgument(format!(
            "account alias must contain 1..={MAX_ACCOUNT_ALIAS_CHARS} ASCII letters, digits, '_' or '-', starting with a letter or digit"
        )))
    }
}

fn valid_account_alias(alias: &str) -> bool {
    let mut bytes = alias.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    alias.len() <= MAX_ACCOUNT_ALIAS_CHARS
        && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn ensure_private_directory(path: &Path) -> Result<(), BossError> {
    #[cfg(not(unix))]
    {
        let _ = path;
        return Err(auth_error(
            "private credential persistence requires Unix permissions",
        ));
    }

    #[cfg(unix)]
    {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    || !metadata.is_dir()
                    || metadata.uid() != current_uid()
                {
                    return Err(auth_error("private credential directory is unsafe"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(path)
                    .map_err(|_| auth_error("unable to create private credential directory"))?;
            }
            Err(_) => return Err(auth_error("unable to access private credential directory")),
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| auth_error("unable to secure private credential directory"))?;
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| auth_error("unable to inspect private credential directory"))?;
        if !private_directory_metadata(&metadata) {
            return Err(auth_error("private credential directory is unsafe"));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn private_directory_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == current_uid()
        && (metadata.mode() & 0o777) == 0o700
}

#[cfg(not(unix))]
fn private_directory_metadata(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn private_file_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == current_uid()
        && (metadata.mode() & 0o777) == 0o600
}

#[cfg(not(unix))]
fn private_file_metadata(_metadata: &fs::Metadata) -> bool {
    false
}

pub(crate) fn validate_cookie(cookie: &str) -> Result<(), BossError> {
    if cookie.is_empty()
        || cookie.len() > MAX_COOKIE_BYTES
        || cookie.contains(['\r', '\n', '\0'])
        || !cookie.is_ascii()
    {
        return Err(auth_error("credential input is invalid"));
    }
    let mut count = 0;
    for part in cookie.split(';') {
        let part = part.trim();
        let Some((name, value)) = part.split_once('=') else {
            return Err(auth_error("credential input is invalid"));
        };
        if !valid_cookie_name(name) || !valid_cookie_value(value) {
            return Err(auth_error("credential input is invalid"));
        }
        count += 1;
    }
    if count == 0 {
        return Err(auth_error("credential input is invalid"));
    }
    Ok(())
}

fn valid_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_cookie_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b';')
}

fn auth_error(message: &'static str) -> BossError {
    BossError::Authentication(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn legacy_credential_file_is_migrated_when_store_is_opened() {
        let directory = tempdir().expect("tempdir");
        let paths = DataPaths::new(directory.path());
        let auth_directory = paths.auth_dir();
        fs::create_dir(&auth_directory).expect("create auth directory");
        fs::set_permissions(&auth_directory, fs::Permissions::from_mode(0o700))
            .expect("secure auth directory");
        fs::write(
            paths.auth_sessions(),
            br#"{"zhipin":{"cookie":"session=zhipin-fixture","credential_file":"retired.txt"},"zhilian":{"cookie":"session=zhilian-fixture"},"qiancheng":{"cookie":"session=qiancheng-fixture"}}"#,
        )
        .expect("write legacy session");
        fs::set_permissions(paths.auth_sessions(), fs::Permissions::from_mode(0o600))
            .expect("secure legacy session");

        let store = AuthStore::from_paths(&paths);
        assert_eq!(store.health(), AuthHealth::Ready);
        assert_eq!(
            store.session_cookie(Platform::Zhipin).as_deref(),
            Some("session=zhipin-fixture")
        );
        let rewritten: serde_json::Value = serde_json::from_slice(
            &fs::read(paths.auth_sessions()).expect("read rewritten session"),
        )
        .expect("parse rewritten session");
        assert_eq!(rewritten["selected_account"], DEFAULT_ACCOUNT);
        assert_eq!(
            rewritten["accounts"][DEFAULT_ACCOUNT]["zhipin"]["cookie"],
            "session=zhipin-fixture"
        );
        assert!(
            rewritten["accounts"][DEFAULT_ACCOUNT]
                .get("zhilian")
                .is_none()
        );
        assert!(
            rewritten["accounts"][DEFAULT_ACCOUNT]
                .get("qiancheng")
                .is_none()
        );
        assert!(
            rewritten["accounts"][DEFAULT_ACCOUNT]["zhipin"]
                .get("credential_file")
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_store_requires_safe_permissions() {
        let directory = tempdir().expect("tempdir");
        let paths = DataPaths::new(directory.path());
        let mut store = AuthStore::from_paths(&paths);
        store
            .store_session(Platform::Zhipin, "session=fixture".to_owned())
            .expect("store");
        let directory_mode = fs::metadata(paths.auth_dir()).expect("directory").mode() & 0o777;
        let file_mode = fs::metadata(paths.auth_sessions()).expect("file").mode() & 0o777;
        assert_eq!((directory_mode, file_mode), (0o700, 0o600));
    }

    #[test]
    fn account_aliases_are_bounded_and_safe() {
        for valid in ["default", "work", "work-2", "client_3"] {
            assert!(validate_account_alias(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "-work",
            "_work",
            "work account",
            "工作",
            "work.",
            "a23456789012345678901234567890123",
        ] {
            assert!(validate_account_alias(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn cookie_stdin_accepts_one_cookie_with_optional_line_ending() {
        for input in [
            &b"session=fixture"[..],
            &b"session=fixture\n"[..],
            &b"session=fixture\r\n"[..],
        ] {
            assert_eq!(
                parse_cookie_stdin(input).expect("valid stdin"),
                "session=fixture"
            );
        }
    }

    #[test]
    fn cookie_stdin_rejects_empty_multiline_and_oversized_input() {
        for input in [
            Vec::new(),
            b"\n".to_vec(),
            b"session=first\nsession=second\n".to_vec(),
            b"session=first\n\n".to_vec(),
            vec![b'x'; MAX_COOKIE_BYTES + 3],
        ] {
            let error = parse_cookie_stdin(&input).expect_err("invalid stdin");
            assert_eq!(
                error.to_string(),
                "authentication setup failed: credential input is invalid"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn session_health_reports_cookie_classes_without_values() {
        let directory = tempdir().expect("tempdir");
        let paths = DataPaths::new(directory.path());
        let mut store = AuthStore::from_paths(&paths);
        assert_eq!(
            store.session_health(Platform::Zhipin),
            SessionHealth::MISSING
        );

        store
            .store_session(Platform::Zhipin, "wt2=primary-fixture".to_owned())
            .expect("store ready session");
        let ready = store.session_health(Platform::Zhipin);
        assert_eq!(ready.state, "ready");
        assert_eq!(ready.next_action, "none");
        assert!(ready.cookie_present);
        assert!(ready.primary_cookie_present);
        assert!(!ready.stoken_present);
        assert!(!ready.auxiliary_cookie_present);

        store
            .store_session(
                Platform::Zhipin,
                "__zp_stoken__=stoken-fixture; wbg=aux-fixture".to_owned(),
            )
            .expect("store partial session");
        let partial = store.session_health(Platform::Zhipin);
        assert_eq!(partial.state, "partial");
        assert_eq!(partial.next_action, "boss login");
        assert!(partial.cookie_present);
        assert!(!partial.primary_cookie_present);
        assert!(partial.stoken_present);
        assert!(partial.auxiliary_cookie_present);

        let serialized = serde_json::to_string(&[ready, partial]).expect("serialize health");
        assert!(serialized.contains("primary_cookie_present"));
        assert!(serialized.contains("stoken_present"));
        assert!(serialized.contains("auxiliary_cookie_present"));
        assert!(serialized.contains("\"state\":\"ready\""));
        assert!(serialized.contains("\"state\":\"partial\""));
        assert!(!serialized.contains("primary-fixture"));
        assert!(!serialized.contains("stoken-fixture"));
        assert!(!serialized.contains("aux-fixture"));
    }

    #[cfg(unix)]
    #[test]
    fn verified_login_updates_role_and_cookie_together_after_validation() {
        let directory = tempdir().expect("tempdir");
        let paths = DataPaths::new(directory.path());
        let mut store = AuthStore::from_paths(&paths);
        store
            .store_session(Platform::Zhipin, "wt2=previous".to_owned())
            .expect("previous session");
        let error = store
            .store_verified_login(
                Platform::Zhipin,
                "not-a-cookie".to_owned(),
                ZhipinRole::Recruiter,
            )
            .expect_err("invalid Cookie");
        assert!(matches!(error, BossError::Authentication(_)));
        assert_eq!(store.active_role(), ZhipinRole::Geek);
        assert_eq!(
            store.session_cookie(Platform::Zhipin).as_deref(),
            Some("wt2=previous")
        );

        store
            .store_verified_login(
                Platform::Zhipin,
                "wt2=replacement".to_owned(),
                ZhipinRole::Recruiter,
            )
            .expect("verified login");
        assert_eq!(store.active_role(), ZhipinRole::Recruiter);
        assert_eq!(
            store.session_cookie(Platform::Zhipin).as_deref(),
            Some("wt2=replacement")
        );
    }

    #[cfg(unix)]
    #[test]
    fn named_accounts_isolate_sessions_and_environment_eligibility() {
        let directory = tempdir().expect("tempdir");
        let paths = DataPaths::new(directory.path());
        let mut store = AuthStore::from_paths(&paths);
        store
            .store_session(Platform::Zhipin, "session=default-fixture".to_owned())
            .expect("store default");

        store.use_account("work").expect("select work");
        assert!(!store.allows_environment_cookie());
        assert!(!store.has_session(Platform::Zhipin));
        store
            .store_session(Platform::Zhipin, "session=work-fixture".to_owned())
            .expect("store work");

        store
            .select_runtime_account(DEFAULT_ACCOUNT)
            .expect("select default");
        assert!(store.allows_environment_cookie());
        assert_eq!(
            store.session_cookie(Platform::Zhipin).as_deref(),
            Some("session=default-fixture")
        );
        store
            .select_runtime_account("work")
            .expect("select work override");
        assert_eq!(
            store.session_cookie(Platform::Zhipin).as_deref(),
            Some("session=work-fixture")
        );
        assert_eq!(store.default_account(), "work");

        let reloaded = AuthStore::from_paths(&paths);
        assert_eq!(reloaded.active_account(), "work");
        assert_eq!(
            reloaded.session_cookie(Platform::Zhipin).as_deref(),
            Some("session=work-fixture")
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_accounts_default_to_geek_and_persist_explicit_recruiter_role() {
        let directory = tempdir().expect("tempdir");
        let paths = DataPaths::new(directory.path());
        fs::create_dir(paths.auth_dir()).expect("auth directory");
        fs::set_permissions(paths.auth_dir(), fs::Permissions::from_mode(0o700))
            .expect("secure directory");
        fs::write(paths.auth_sessions(), br#"{"selected_account":"work","accounts":{"work":{"zhipin":{"cookie":"session=fixture"}}}}"#).expect("session");
        fs::set_permissions(paths.auth_sessions(), fs::Permissions::from_mode(0o600))
            .expect("secure session");
        let mut store = AuthStore::from_paths(&paths);
        assert_eq!(store.active_role(), ZhipinRole::Geek);
        store
            .store_verified_login(
                Platform::Zhipin,
                "session=replacement".to_owned(),
                ZhipinRole::Recruiter,
            )
            .expect("store role with verified login");
        assert_eq!(
            AuthStore::from_paths(&paths).active_role(),
            ZhipinRole::Recruiter
        );
    }
}
