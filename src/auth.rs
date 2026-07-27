//! Private local credential storage and deliberately bounded Cookie import.
//!
//! This module never contacts a recruitment platform. It accepts only an
//! explicitly selected or previously registered export file, never scans
//! desktop-client stores, and never exposes Cookie values or source paths.

use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::data::atomic_write_private;
use crate::{BossError, DataPaths, Platform};

const MAX_COOKIE_BYTES: usize = 16 * 1024;
const MAX_EXPORT_BYTES: u64 = 64 * 1024;

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

/// Local private session and registered export references.
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
    #[serde(default)]
    credential_file: Option<PathBuf>,
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
        Self {
            directory,
            path,
            document,
            health,
        }
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

    /// Returns whether a registered, user-selected export file exists in the store.
    #[must_use]
    pub fn has_registered_export(&self, platform: Platform) -> bool {
        self.entry(platform).credential_file.is_some()
    }

    /// Returns the stable, data-root-local default export path for one platform.
    #[must_use]
    pub fn default_export_path(&self, platform: Platform) -> PathBuf {
        self.directory.join(format!("{}.cookie", platform.as_str()))
    }

    /// Reads a default export from an existing private auth directory only.
    #[must_use]
    pub fn default_export_cookie(&self, platform: Platform) -> Option<String> {
        if !fs::symlink_metadata(&self.directory)
            .is_ok_and(|metadata| private_directory_metadata(&metadata))
        {
            return None;
        }
        self.read_export(platform, &self.default_export_path(platform))
            .ok()
    }

    /// Reads and validates an explicitly selected credential export.
    pub fn read_export(&self, platform: Platform, path: &Path) -> Result<String, BossError> {
        let bytes = read_export_file(path)?;
        parse_cookie_export(&bytes, platform)
    }

    /// Re-reads the registered export for automatic login attempts.
    pub fn registered_export_cookie(
        &self,
        platform: Platform,
    ) -> Result<Option<String>, BossError> {
        self.entry(platform)
            .credential_file
            .as_deref()
            .map(|path| self.read_export(platform, path))
            .transpose()
    }

    /// Stores one validated session without changing a registered export path.
    pub fn store_session(&mut self, platform: Platform, cookie: String) -> Result<(), BossError> {
        validate_cookie(&cookie)?;
        self.entry_mut(platform).cookie = Some(cookie);
        self.persist()
    }

    /// Stores one session and remembers the explicit export path used to import it.
    pub fn store_file_session(
        &mut self,
        platform: Platform,
        path: &Path,
        cookie: String,
    ) -> Result<(), BossError> {
        validate_cookie(&cookie)?;
        let entry = self.entry_mut(platform);
        entry.cookie = Some(cookie);
        entry.credential_file = Some(path.to_path_buf());
        self.persist()
    }

    /// Removes the saved session and registered export reference for one platform.
    pub fn revoke(&mut self, platform: Platform) -> Result<bool, BossError> {
        let entry = self.entry_mut(platform);
        let changed = entry.cookie.take().is_some() | entry.credential_file.take().is_some();
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
    if !private_file_metadata(&metadata) || metadata.len() > MAX_EXPORT_BYTES {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
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
                && entry
                    .credential_file
                    .as_ref()
                    .is_none_or(|path| !path.as_os_str().is_empty())
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

#[cfg(unix)]
fn read_export_file(path: &Path) -> Result<Vec<u8>, BossError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| auth_error("credential export is unavailable"))?;
    let metadata = file
        .metadata()
        .map_err(|_| auth_error("credential export is unavailable"))?;
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || (metadata.mode() & 0o077) != 0
        || metadata.len() > MAX_EXPORT_BYTES
    {
        return Err(auth_error("credential export is unsafe"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_EXPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| auth_error("credential export is unavailable"))?;
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        return Err(auth_error("credential export is too large"));
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_export_file(_path: &Path) -> Result<Vec<u8>, BossError> {
    Err(auth_error(
        "credential export import requires Unix file safety checks",
    ))
}

fn parse_cookie_export(bytes: &[u8], platform: Platform) -> Result<String, BossError> {
    if bytes.len() > MAX_EXPORT_BYTES as usize {
        return Err(auth_error("credential export is too large"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| auth_error("credential export must be UTF-8 text"))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(auth_error("credential export is empty"));
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return parse_json_export(trimmed, platform);
    }
    if trimmed.contains('\t') || trimmed.starts_with("# Netscape HTTP Cookie File") {
        return parse_netscape_export(trimmed, platform);
    }
    if trimmed.contains(['\r', '\n']) {
        return Err(auth_error("credential export format is unsupported"));
    }
    parse_raw_cookie(trimmed)
}

fn parse_raw_cookie(text: &str) -> Result<String, BossError> {
    let cookie = text
        .strip_prefix("Cookie:")
        .or_else(|| text.strip_prefix("cookie:"))
        .unwrap_or(text)
        .trim();
    validate_cookie(cookie)?;
    Ok(cookie.to_owned())
}

fn parse_netscape_export(text: &str, platform: Platform) -> Result<String, BossError> {
    let mut cookies = Vec::new();
    let mut saw_cookie_row = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("# Netscape HTTP Cookie File") {
            continue;
        }
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 7 {
            return Err(auth_error("credential export format is unsupported"));
        }
        saw_cookie_row = true;
        if domain_matches(platform, fields[0]) {
            cookies.push(cookie_pair(fields[5], fields[6])?);
        }
    }
    if !saw_cookie_row || cookies.is_empty() {
        return Err(auth_error(
            "credential export has no matching platform cookies",
        ));
    }
    let cookie = cookies.join("; ");
    validate_cookie(&cookie)?;
    Ok(cookie)
}

fn parse_json_export(text: &str, platform: Platform) -> Result<String, BossError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| auth_error("credential export format is unsupported"))?;
    if let Some(cookie) = value
        .as_object()
        .and_then(|object| object.get("cookie"))
        .and_then(Value::as_str)
    {
        return parse_raw_cookie(cookie);
    }
    let cookies = match &value {
        Value::Array(cookies) => cookies,
        Value::Object(object) => object
            .get("cookies")
            .and_then(Value::as_array)
            .ok_or_else(|| auth_error("credential export format is unsupported"))?,
        _ => return Err(auth_error("credential export format is unsupported")),
    };
    let mut selected = Vec::new();
    for item in cookies {
        let object = item
            .as_object()
            .ok_or_else(|| auth_error("credential export format is unsupported"))?;
        let domain = object
            .get("domain")
            .and_then(Value::as_str)
            .ok_or_else(|| auth_error("credential export format is unsupported"))?;
        if domain_matches(platform, domain) {
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| auth_error("credential export format is unsupported"))?;
            let value = object
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| auth_error("credential export format is unsupported"))?;
            selected.push(cookie_pair(name, value)?);
        }
    }
    if selected.is_empty() {
        return Err(auth_error(
            "credential export has no matching platform cookies",
        ));
    }
    let cookie = selected.join("; ");
    validate_cookie(&cookie)?;
    Ok(cookie)
}

fn domain_matches(platform: Platform, domain: &str) -> bool {
    let domain = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    let expected = match platform {
        Platform::Zhipin => "zhipin.com",
        Platform::Zhilian => "zhaopin.com",
        Platform::Qiancheng => "51job.com",
    };
    domain == expected || domain.ends_with(&format!(".{expected}"))
}

fn cookie_pair(name: &str, value: &str) -> Result<String, BossError> {
    if !valid_cookie_name(name) || !valid_cookie_value(value) {
        return Err(auth_error("credential export contains an invalid cookie"));
    }
    Ok(format!("{name}={value}"))
}

fn validate_cookie(cookie: &str) -> Result<(), BossError> {
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

    #[test]
    fn raw_json_and_netscape_exports_are_constrained() {
        assert_eq!(
            parse_cookie_export(b"Cookie: session=fixture; token=value", Platform::Zhipin)
                .expect("raw"),
            "session=fixture; token=value"
        );
        assert_eq!(
            parse_cookie_export(
                br#"{"cookies":[{"domain":".zhipin.com","name":"session","value":"fixture"},{"domain":".51job.com","name":"other","value":"skip"}]}"#,
                Platform::Zhipin,
            )
            .expect("json"),
            "session=fixture"
        );
        assert_eq!(
            parse_cookie_export(
                b"# Netscape HTTP Cookie File\n.zhipin.com\tTRUE\t/\tFALSE\t0\tsession\tfixture\n.51job.com\tTRUE\t/\tFALSE\t0\tother\tskip\n",
                Platform::Zhipin,
            )
            .expect("netscape"),
            "session=fixture"
        );
        assert!(
            parse_cookie_export(b"session=fixture\r\nInjected: value", Platform::Zhipin).is_err()
        );
    }

    #[test]
    fn default_export_paths_are_data_root_local_and_platform_specific() {
        let directory = tempdir().expect("tempdir");
        let store = AuthStore::from_paths(&DataPaths::new(directory.path()));
        let auth_directory = directory.path().join(".auth");
        assert_eq!(
            store.default_export_path(Platform::Zhipin),
            auth_directory.join("zhipin.cookie")
        );
        assert_eq!(
            store.default_export_path(Platform::Zhilian),
            auth_directory.join("zhilian.cookie")
        );
        assert_eq!(
            store.default_export_path(Platform::Qiancheng),
            auth_directory.join("qiancheng.cookie")
        );
    }

    #[cfg(unix)]
    #[test]
    fn default_export_cookie_requires_a_private_directory_and_safe_file() {
        let directory = tempdir().expect("tempdir");
        let store = AuthStore::from_paths(&DataPaths::new(directory.path()));
        assert_eq!(store.default_export_cookie(Platform::Zhipin), None);

        let auth_directory = directory.path().join(".auth");
        fs::create_dir(&auth_directory).expect("create auth directory");
        fs::set_permissions(&auth_directory, fs::Permissions::from_mode(0o700))
            .expect("secure auth directory");
        let source = store.default_export_path(Platform::Zhipin);
        fs::write(&source, b"session=fixture").expect("fixture");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).expect("open export");
        assert_eq!(store.default_export_cookie(Platform::Zhipin), None);
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).expect("secure export");
        assert_eq!(
            store.default_export_cookie(Platform::Zhipin),
            Some("session=fixture".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn default_export_cookie_does_not_follow_an_auth_directory_symlink() {
        let data_root = tempdir().expect("data root");
        let external_directory = tempdir().expect("external directory");
        fs::set_permissions(external_directory.path(), fs::Permissions::from_mode(0o700))
            .expect("secure external directory");
        let external_source = external_directory.path().join("zhipin.cookie");
        fs::write(&external_source, b"session=external-fixture").expect("fixture");
        fs::set_permissions(&external_source, fs::Permissions::from_mode(0o600))
            .expect("secure external export");
        std::os::unix::fs::symlink(external_directory.path(), data_root.path().join(".auth"))
            .expect("symlink auth directory");

        let store = AuthStore::from_paths(&DataPaths::new(data_root.path()));
        assert_eq!(store.default_export_cookie(Platform::Zhipin), None);
    }

    #[cfg(unix)]
    #[test]
    fn source_exports_and_private_store_require_safe_permissions() {
        let directory = tempdir().expect("tempdir");
        let source = directory.path().join("desktop-export.txt");
        fs::write(&source, b"session=fixture").expect("fixture");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).expect("open export");
        assert!(read_export_file(&source).is_err());
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).expect("chmod source");
        assert_eq!(
            parse_cookie_export(
                &read_export_file(&source).expect("private export"),
                Platform::Zhipin,
            )
            .expect("parse"),
            "session=fixture"
        );

        let paths = DataPaths::new(directory.path());
        let mut store = AuthStore::from_paths(&paths);
        store
            .store_session(Platform::Zhipin, "session=fixture".to_owned())
            .expect("store");
        let directory_mode = fs::metadata(paths.auth_dir()).expect("directory").mode() & 0o777;
        let file_mode = fs::metadata(paths.auth_sessions()).expect("file").mode() & 0o777;
        assert_eq!((directory_mode, file_mode), (0o700, 0o600));
    }

    #[cfg(unix)]
    #[test]
    fn source_exports_reject_symlinks_non_files_and_oversized_files() {
        let directory = tempdir().expect("tempdir");
        let source = directory.path().join("source.txt");
        fs::write(&source, b"session=fixture").expect("fixture");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).expect("chmod source");

        let link = directory.path().join("source-link.txt");
        std::os::unix::fs::symlink(&source, &link).expect("symlink");
        assert!(read_export_file(&link).is_err());

        let non_file = directory.path().join("not-a-file");
        fs::create_dir(&non_file).expect("directory");
        assert!(read_export_file(&non_file).is_err());

        let oversized = directory.path().join("oversized.txt");
        fs::write(&oversized, vec![b'x'; MAX_EXPORT_BYTES as usize + 1]).expect("large export");
        fs::set_permissions(&oversized, fs::Permissions::from_mode(0o600))
            .expect("chmod oversized");
        assert!(read_export_file(&oversized).is_err());
    }
}
