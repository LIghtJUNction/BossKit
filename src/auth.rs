//! Private local Cookie session storage.
//!
//! This module never contacts a recruitment platform or reads Cookie export
//! files. It stores only validated local sessions and never exposes values.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::io::{self, IsTerminal, Write};
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
    zhipin: StoredAuth,
    #[serde(default)]
    zhilian: StoredAuth,
    #[serde(default)]
    qiancheng: StoredAuth,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyAuthDocument {
    #[serde(default)]
    zhipin: StoredAuth,
    #[serde(default)]
    zhilian: StoredAuth,
    #[serde(default)]
    qiancheng: StoredAuth,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SerializedAuthDocument {
    Current(AuthDocument),
    Legacy(LegacyAuthDocument),
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
            Platform::Zhilian => "BOSS_ZHILIAN_COOKIE",
            Platform::Qiancheng => "BOSS_QIANCHENG_COOKIE",
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
        self.document.accounts.values().any(|account| {
            [&account.zhipin, &account.zhilian, &account.qiancheng]
                .into_iter()
                .any(|entry| entry.legacy_credential_file)
        })
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
            Platform::Zhilian => &self.zhilian,
            Platform::Qiancheng => &self.qiancheng,
        }
    }

    fn entry_mut(&mut self, platform: Platform) -> &mut StoredAuth {
        match platform {
            Platform::Zhipin => &mut self.zhipin,
            Platform::Zhilian => &mut self.zhilian,
            Platform::Qiancheng => &mut self.qiancheng,
        }
    }
}

/// Reads one manual Cookie with terminal echo disabled, if stdin is a TTY.
///
/// A non-terminal stdin is deliberately never read or prompted and returns
/// `Ok(None)` so the CLI can emit a structured manual-login-required result.
pub fn read_manual_cookie(platform: Platform) -> Result<Option<String>, BossError> {
    #[cfg(unix)]
    {
        if !io::stdin().is_terminal() {
            return Ok(None);
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
        Ok(Some(cookie.to_owned()))
    }

    #[cfg(not(unix))]
    {
        let _ = platform;
        Ok(None)
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
                zhilian: legacy.zhilian,
                qiancheng: legacy.qiancheng,
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
                && [&account.zhipin, &account.zhilian, &account.qiancheng]
                    .into_iter()
                    .all(|entry| {
                        entry
                            .cookie
                            .as_deref()
                            .is_none_or(|cookie| validate_cookie(cookie).is_ok())
                    })
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
        assert_eq!(
            rewritten["accounts"][DEFAULT_ACCOUNT]["zhilian"]["cookie"],
            "session=zhilian-fixture"
        );
        assert_eq!(
            rewritten["accounts"][DEFAULT_ACCOUNT]["qiancheng"]["cookie"],
            "session=qiancheng-fixture"
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

    #[cfg(unix)]
    #[test]
    fn named_accounts_isolate_sessions_and_environment_eligibility() {
        let directory = tempdir().expect("tempdir");
        let paths = DataPaths::new(directory.path());
        let mut store = AuthStore::from_paths(&paths);
        store
            .store_session(Platform::Zhilian, "session=default-fixture".to_owned())
            .expect("store default");

        store.use_account("work").expect("select work");
        assert!(!store.allows_environment_cookie());
        assert!(!store.has_session(Platform::Zhilian));
        store
            .store_session(Platform::Zhilian, "session=work-fixture".to_owned())
            .expect("store work");

        store
            .select_runtime_account(DEFAULT_ACCOUNT)
            .expect("select default");
        assert!(store.allows_environment_cookie());
        assert_eq!(
            store.session_cookie(Platform::Zhilian).as_deref(),
            Some("session=default-fixture")
        );
        store
            .select_runtime_account("work")
            .expect("select work override");
        assert_eq!(
            store.session_cookie(Platform::Zhilian).as_deref(),
            Some("session=work-fixture")
        );
        assert_eq!(store.default_account(), "work");

        let reloaded = AuthStore::from_paths(&paths);
        assert_eq!(reloaded.active_account(), "work");
        assert_eq!(
            reloaded.session_cookie(Platform::Zhilian).as_deref(),
            Some("session=work-fixture")
        );
    }
}
