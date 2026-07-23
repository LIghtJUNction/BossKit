//! Local shortlist persistence and annotation.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::data::atomic_write;
use crate::{BossError, DataPaths, Job};

/// A cached job snapshot selected by the user.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShortlistEntry {
    /// Full normalized snapshot at the latest add/update.
    pub job: Job,
    /// Deduplicated user labels.
    pub tags: Vec<String>,
    /// Optional user note.
    pub note: Option<String>,
    /// Unix epoch seconds when first added.
    pub added_at: u64,
}

/// Summary used by shortlist comparison.
#[derive(Clone, Debug, Serialize)]
pub struct ShortlistComparison {
    /// Optional applied tag.
    pub tag: Option<String>,
    /// Number of matching entries.
    pub count: usize,
    /// Side-by-side local entries.
    pub entries: Vec<ShortlistEntry>,
}

/// Atomic local shortlist store.
#[derive(Clone, Debug)]
pub struct ShortlistStore {
    path: PathBuf,
}

impl ShortlistStore {
    /// Opens the shortlist at shared application paths.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.shortlist(),
        }
    }

    /// Adds a new item or updates its snapshot, tags, and note without duplication.
    pub fn add(
        &self,
        job: Job,
        tags: Vec<String>,
        note: Option<String>,
    ) -> Result<ShortlistEntry, BossError> {
        let mut entries = self.read_all()?;
        let tags = normalize_tags(tags);
        if let Some(entry) = entries.iter_mut().find(|entry| entry.job.id == job.id) {
            entry.job = job;
            entry.tags = tags;
            entry.note = normalize_note(note);
            let updated = entry.clone();
            self.save(&entries)?;
            return Ok(updated);
        }
        let entry = ShortlistEntry {
            job,
            tags,
            note: normalize_note(note),
            added_at: now_epoch_seconds()?,
        };
        entries.push(entry.clone());
        self.save(&entries)?;
        Ok(entry)
    }

    /// Lists entries, optionally filtering by an exact normalized tag.
    pub fn list(&self, tag: Option<&str>) -> Result<Vec<ShortlistEntry>, BossError> {
        let tag = tag.map(str::trim).filter(|tag| !tag.is_empty());
        Ok(self
            .read_all()?
            .into_iter()
            .filter(|entry| tag.is_none_or(|tag| entry.tags.iter().any(|item| item == tag)))
            .collect())
    }

    /// Updates local tags and optionally replaces the note.
    pub fn annotate(
        &self,
        job_id: &str,
        add_tags: Vec<String>,
        remove_tags: Vec<String>,
        note: Option<String>,
    ) -> Result<ShortlistEntry, BossError> {
        let mut entries = self.read_all()?;
        let entry = entries
            .iter_mut()
            .find(|entry| entry.job.id == job_id)
            .ok_or_else(|| {
                BossError::InvalidArgument(format!("shortlist item not found: {job_id}"))
            })?;
        let remove: HashSet<String> = normalize_tags(remove_tags).into_iter().collect();
        entry.tags.retain(|tag| !remove.contains(tag));
        entry.tags.extend(normalize_tags(add_tags));
        entry.tags = normalize_tags(std::mem::take(&mut entry.tags));
        if note.is_some() {
            entry.note = normalize_note(note);
        }
        let updated = entry.clone();
        self.save(&entries)?;
        Ok(updated)
    }

    /// Removes one entry.
    pub fn remove(&self, job_id: &str) -> Result<ShortlistEntry, BossError> {
        let mut entries = self.read_all()?;
        let index = entries
            .iter()
            .position(|entry| entry.job.id == job_id)
            .ok_or_else(|| {
                BossError::InvalidArgument(format!("shortlist item not found: {job_id}"))
            })?;
        let removed = entries.remove(index);
        self.save(&entries)?;
        Ok(removed)
    }

    /// Returns local entries in a comparison-friendly structure.
    pub fn compare(&self, tag: Option<&str>) -> Result<ShortlistComparison, BossError> {
        let entries = self.list(tag)?;
        Ok(ShortlistComparison {
            tag: tag.map(ToOwned::to_owned),
            count: entries.len(),
            entries,
        })
    }

    /// Verifies that persisted shortlist JSON can be read.
    pub(crate) fn check_readable(&self) -> Result<(), BossError> {
        self.read_all().map(|_| ())
    }

    fn read_all(&self) -> Result<Vec<ShortlistEntry>, BossError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| BossError::ShortlistJson(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(BossError::ShortlistIo(error.to_string())),
        }
    }

    fn save(&self, entries: &[ShortlistEntry]) -> Result<(), BossError> {
        let bytes = serde_json::to_vec_pretty(entries)
            .map_err(|error| BossError::ShortlistJson(error.to_string()))?;
        atomic_write(&self.path, &bytes, |error| {
            BossError::ShortlistIo(error.to_string())
        })
    }
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    tags.into_iter()
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty() && seen.insert(tag.clone()))
        .collect()
}

fn normalize_note(note: Option<String>) -> Option<String> {
    note.map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn now_epoch_seconds() -> Result<u64, BossError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| BossError::ShortlistIo(error.to_string()))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::Platform;

    fn fixture() -> (tempfile::TempDir, ShortlistStore, Job) {
        let directory = tempdir().expect("tempdir");
        let store = ShortlistStore::from_paths(&DataPaths::new(directory.path()));
        let mut job = Job::new(
            "job-1",
            Platform::Zhipin,
            "remote",
            "Rust",
            "https://example.test/job",
        );
        job.company = "Example".to_owned();
        job.city = "深圳".to_owned();
        job.salary = "20K".to_owned();
        (directory, store, job)
    }

    #[test]
    fn add_deduplicates_tags_and_list_filters() {
        let (_directory, store, job) = fixture();
        store
            .add(job, vec![" remote ".to_owned(), "remote".to_owned()], None)
            .expect("add");
        assert_eq!(store.list(Some("remote")).expect("list")[0].tags.len(), 1);
    }

    #[test]
    fn readd_updates_without_duplicate_and_preserves_added_at() {
        let (_directory, store, job) = fixture();
        let first = store.add(job.clone(), Vec::new(), None).expect("first");
        let second = store
            .add(job, vec!["later".to_owned()], Some("note".to_owned()))
            .expect("second");
        assert_eq!(
            (store.list(None).expect("list").len(), second.added_at),
            (1, first.added_at)
        );
    }

    #[test]
    fn annotate_and_remove_complete_lifecycle() {
        let (_directory, store, job) = fixture();
        store.add(job, vec!["old".to_owned()], None).expect("add");
        let changed = store
            .annotate(
                "job-1",
                vec!["new".to_owned()],
                vec!["old".to_owned()],
                Some("updated".to_owned()),
            )
            .expect("annotate");
        assert_eq!(changed.tags, vec!["new"]);
        assert_eq!(store.remove("job-1").expect("remove").job.id, "job-1");
    }
}
