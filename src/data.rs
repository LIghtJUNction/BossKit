//! Shared data-root discovery and scoped atomic file replacement.

use std::fs::{self, File};
use std::io::Write;
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
    }
}
