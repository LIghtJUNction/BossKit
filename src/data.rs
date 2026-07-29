//! Shared data-root discovery and scoped atomic file replacement.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Paths for every persistent BossKit JSON file.
#[derive(Clone, Debug)]
pub struct DataPaths {
    root: PathBuf,
}

impl DataPaths {
    /// Resolves `BOSS_DATA_DIR`, the OS local data directory, or `.boss`.
    #[must_use]
    pub fn discover() -> Self {
        let root = std::env::var_os("BOSS_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::data_local_dir().map(|path| path.join("bosskit")))
            .unwrap_or_else(|| PathBuf::from(".boss"));
        Self::new(root)
    }

    /// Uses an explicit data root.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the normalized job cache path.
    #[must_use]
    pub fn jobs(&self) -> PathBuf {
        self.root.join("jobs.json")
    }

    /// Returns the user configuration path.
    #[must_use]
    pub fn config(&self) -> PathBuf {
        self.root.join("config.json")
    }

    /// Returns the shortlist path.
    #[must_use]
    pub fn shortlist(&self) -> PathBuf {
        self.root.join("shortlist.json")
    }

    /// Returns the local search audit history path.
    #[must_use]
    pub fn history(&self) -> PathBuf {
        self.root.join("history.json")
    }

    /// Returns the saved search presets path.
    #[must_use]
    pub fn presets(&self) -> PathBuf {
        self.root.join("presets.json")
    }

    /// Returns the foreground watches path.
    #[must_use]
    pub fn watches(&self) -> PathBuf {
        self.root.join("watches.json")
    }

    /// Returns the local typed resumes path.
    #[must_use]
    pub fn resumes(&self) -> PathBuf {
        self.root.join("resumes.json")
    }

    /// Returns the local keyword-reply rule path.
    #[must_use]
    pub fn reply_rules(&self) -> PathBuf {
        self.root.join("reply_rules.json")
    }

    /// Returns the reusable local campaign policy path.
    #[must_use]
    pub fn campaign_policies(&self) -> PathBuf {
        self.root.join("campaign_policies.json")
    }

    /// Returns the local campaign blacklist path.
    #[must_use]
    pub fn campaign_blacklist(&self) -> PathBuf {
        self.root.join("campaign_blacklist.json")
    }

    /// Returns the local greeting-template path.
    #[must_use]
    pub fn greeting_templates(&self) -> PathBuf {
        self.root.join("greeting_templates.json")
    }

    /// Returns the local manual-review application-plan path.
    #[must_use]
    pub fn application_plans(&self) -> PathBuf {
        self.root.join("application_plans.json")
    }

    /// Returns the credential-free local AI profile path.
    #[must_use]
    pub fn ai_profiles(&self) -> PathBuf {
        self.root.join("ai_profiles.json")
    }

    /// Returns the redacted local notification audit path.
    #[must_use]
    pub fn notification_audit(&self) -> PathBuf {
        self.root.join("notification_audit.json")
    }

    /// Returns the private authentication directory.
    #[must_use]
    pub fn auth_dir(&self) -> PathBuf {
        self.root.join(".auth")
    }

    /// Returns the private persisted-session path.
    #[must_use]
    pub fn auth_sessions(&self) -> PathBuf {
        self.auth_dir().join("sessions.json")
    }
}

/// Atomically replaces one JSON file below its data root.
pub(crate) fn atomic_write(
    path: &Path,
    bytes: &[u8],
    map_error: impl Fn(std::io::Error) -> crate::BossError,
) -> Result<(), crate::BossError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(&map_error)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = parent.join(format!(".bosskit-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = File::create(&temporary).map_err(&map_error)?;
        file.write_all(bytes).map_err(&map_error)?;
        file.sync_all().map_err(&map_error)?;
        fs::rename(&temporary, path).map_err(&map_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

/// Atomically replaces a file that must remain private to the current user.
///
/// Callers are responsible for preparing a private parent directory. This
/// helper refuses non-file or symlink destinations and creates the temporary
/// file with mode `0600` on Unix.
pub(crate) fn atomic_write_private(
    path: &Path,
    bytes: &[u8],
    map_error: impl Fn(std::io::Error) -> crate::BossError,
) -> Result<(), crate::BossError> {
    #[cfg(not(unix))]
    {
        let _ = (path, bytes);
        return Err(map_error(std::io::Error::other(
            "private credential persistence requires Unix permissions",
        )));
    }

    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(&map_error)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(map_error(std::io::Error::other(
                    "private destination is not a regular file",
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(map_error(error)),
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let temporary = parent.join(format!(
            ".bosskit-private-{}-{nonce}.tmp",
            std::process::id()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(&map_error)?;
            file.write_all(bytes).map_err(&map_error)?;
            file.sync_all().map_err(&map_error)?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                .map_err(&map_error)?;
            fs::rename(&temporary, path).map_err(&map_error)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(&map_error)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_root_derives_all_json_paths() {
        let paths = DataPaths::new("/tmp/bosskit-test");
        assert_eq!(paths.jobs(), PathBuf::from("/tmp/bosskit-test/jobs.json"));
        assert_eq!(
            paths.shortlist(),
            PathBuf::from("/tmp/bosskit-test/shortlist.json")
        );
        assert_eq!(
            paths.reply_rules(),
            PathBuf::from("/tmp/bosskit-test/reply_rules.json")
        );
        assert_eq!(
            paths.application_plans(),
            PathBuf::from("/tmp/bosskit-test/application_plans.json")
        );
        assert_eq!(
            paths.ai_profiles(),
            PathBuf::from("/tmp/bosskit-test/ai_profiles.json")
        );
        assert_eq!(
            paths.notification_audit(),
            PathBuf::from("/tmp/bosskit-test/notification_audit.json")
        );
        assert_eq!(
            paths.auth_sessions(),
            PathBuf::from("/tmp/bosskit-test/.auth/sessions.json")
        );
    }
}
