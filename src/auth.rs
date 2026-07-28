//! Private local Cookie session storage.
//!
//! This module never contacts a recruitment platform or reads Cookie export
//! files. It stores only validated local sessions and never exposes values.

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
    health: AuthHealth,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AuthDocument {
    #[serde(default)]
    zhipin: StoredAuth,
    #[serde(default)]
    zhilian: StoredAuth,
    #[serde(default)]
    qiancheng: StoredAuth,
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

impl AuthStore {
    /// Opens a private store without making ordinary application startup fail.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        let directory = paths.auth_dir();
        let path = paths.auth_sessions();
        let (document, health) = match load_document(&directory, &path) {
            Ok(Some(document)) if document_is_valid(&document) => (document, AuthHealth::Ready),
            Ok(Some(_)) | Err(()) => (AuthDocument::default(), AuthHealth::Unavailable),
            Ok(None) => (AuthDocument::default(), AuthHealth::Missing),
        };
        let mut store = Self {
            directory,
            path,
            document,
            health,
        };
        if store.has_legacy_credential_file() && store.persist().is_err() {
            store.document = AuthDocument::default();
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

    /// Resolves the normal provider runtime order: environment, then session.
    #[must_use]
    pub fn runtime_cookie(&self, platform: Platform) -> Option<String> {
        Self::environment_cookie(platform).or_else(|| self.session_cookie(platform))
    }

    /// Returns whether a local session exists for one platform.
    #[must_use]
    pub fn has_session(&self, platform: Platform) -> bool {
        self.session_cookie(platform).is_some()
    }

    /// Prepares the private root used for an isolated interactive browser profile.
    #[must_use]
    pub(crate) fn browser_profile_root(&self) -> Option<PathBuf> {
        ensure_private_directory(&self.directory).ok()?;
        Some(self.directory.clone())
    }

    /// Stores one validated session.
    pub fn store_session(&mut self, platform: Platform, cookie: String) -> Result<(), BossError> {
        validate_cookie(&cookie)?;
        self.entry_mut(platform).cookie = Some(cookie);
        self.persist()
    }

    /// Removes the saved session for one platform.
    pub fn revoke(&mut self, platform: Platform) -> Result<bool, BossError> {
        let entry = self.entry_mut(platform);
        let changed = entry.cookie.take().is_some();
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    fn session_cookie(&self, platform: Platform) -> Option<String> {
        self.entry(platform).cookie.clone()
    }

    fn entry(&self, platform: Platform) -> &StoredAuth {
        match platform {
            Platform::Zhipin => &self.document.zhipin,
            Platform::Zhilian => &self.document.zhilian,
            Platform::Qiancheng => &self.document.qiancheng,
        }
    }

    fn entry_mut(&mut self, platform: Platform) -> &mut StoredAuth {
        match platform {
            Platform::Zhipin => &mut self.document.zhipin,
            Platform::Zhilian => &mut self.document.zhilian,
            Platform::Qiancheng => &mut self.document.qiancheng,
        }
    }

    fn has_legacy_credential_file(&self) -> bool {
        [
            &self.document.zhipin,
            &self.document.zhilian,
            &self.document.qiancheng,
        ]
        .into_iter()
        .any(|entry| entry.legacy_credential_file)
    }

    fn persist(&mut self) -> Result<(), BossError> {
        ensure_private_directory(&self.directory)?;
        let bytes = serde_json::to_vec(&self.document)
            .map_err(|_| auth_error("unable to encode private credential store"))?;
        atomic_write_private(&self.path, &bytes, |_| {
            auth_error("unable to write private credential store")
        })?;
        self.health = AuthHealth::Ready;
        Ok(())
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

fn load_document(directory: &Path, path: &Path) -> Result<Option<AuthDocument>, ()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if private_directory_metadata(&metadata) => {}
        Ok(_) => return Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    }
    let Some(bytes) = read_private_store_file(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|_| ())
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
    [&document.zhipin, &document.zhilian, &document.qiancheng]
        .into_iter()
        .all(|entry| {
            entry
                .cookie
                .as_deref()
                .is_none_or(|cookie| validate_cookie(cookie).is_ok())
        })
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

pub(crate) fn domain_matches(platform: Platform, domain: &str) -> bool {
    let domain = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    let expected = match platform {
        Platform::Zhipin => "zhipin.com",
        Platform::Zhilian => "zhaopin.com",
        Platform::Qiancheng => "51job.com",
    };
    domain == expected || domain.ends_with(&format!(".{expected}"))
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
            br#"{"zhipin":{"cookie":"session=fixture","credential_file":"retired.txt"}}"#,
        )
        .expect("write legacy session");
        fs::set_permissions(paths.auth_sessions(), fs::Permissions::from_mode(0o600))
            .expect("secure legacy session");

        let store = AuthStore::from_paths(&paths);
        assert_eq!(store.health(), AuthHealth::Ready);
        assert_eq!(
            store.session_cookie(Platform::Zhipin).as_deref(),
            Some("session=fixture")
        );
        let rewritten: serde_json::Value = serde_json::from_slice(
            &fs::read(paths.auth_sessions()).expect("read rewritten session"),
        )
        .expect("parse rewritten session");
        assert!(rewritten["zhipin"].get("credential_file").is_none());
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
}
