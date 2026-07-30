//! Foreground local saved-search watches.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::data::atomic_write;
use crate::preset::validate_name;
use crate::{BossError, DataPaths, SearchSpec};

/// One explicit foreground watch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    /// Unique local name.
    pub name: String,
    /// Copied full search specification.
    pub spec: SearchSpec,
    /// Every observed stable job ID, retained exactly to prevent false rediscovery.
    pub seen_ids: Vec<String>,
    /// Creation timestamp.
    pub created_at: u64,
    /// Last mutation timestamp.
    pub updated_at: u64,
    /// Last successful foreground run.
    pub last_run_at: Option<u64>,
}

/// Atomic watch collection.
pub struct WatchStore {
    path: PathBuf,
}

impl WatchStore {
    /// Opens watches below shared paths.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.watches(),
        }
    }

    /// Adds or replaces one watch.
    pub fn add(&self, name: &str, spec: SearchSpec, now: u64) -> Result<Watch, BossError> {
        let name = validate_name(name)?;
        let mut entries = self.read_all()?;
        if let Some(entry) = entries.iter_mut().find(|entry| entry.name == name) {
            entry.spec = spec;
            entry.updated_at = now;
            let updated = entry.clone();
            self.save(&entries)?;
            return Ok(updated);
        }
        let entry = Watch {
            name,
            spec,
            seen_ids: Vec::new(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
        };
        entries.push(entry.clone());
        self.save(&entries)?;
        Ok(entry)
    }

    /// Lists watches.
    pub fn list(&self) -> Result<Vec<Watch>, BossError> {
        self.read_all()
    }

    /// Shows one watch.
    pub fn show(&self, name: &str) -> Result<Watch, BossError> {
        let name = validate_name(name)?;
        self.read_all()?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| BossError::Watch(format!("not found: {name}")))
    }

    /// Removes one watch.
    pub fn remove(&self, name: &str) -> Result<Watch, BossError> {
        let name = validate_name(name)?;
        let mut entries = self.read_all()?;
        let index = entries
            .iter()
            .position(|entry| entry.name == name)
            .ok_or_else(|| BossError::Watch(format!("not found: {name}")))?;
        let removed = entries.remove(index);
        self.save(&entries)?;
        Ok(removed)
    }

    /// Records IDs after a run with at least one provider success.
    pub fn record_success(
        &self,
        name: &str,
        ids: &[String],
        now: u64,
    ) -> Result<Vec<String>, BossError> {
        let mut entries = self.read_all()?;
        let entry = entries
            .iter_mut()
            .find(|entry| entry.name == name)
            .ok_or_else(|| BossError::Watch(format!("not found: {name}")))?;
        let known: HashSet<&str> = entry.seen_ids.iter().map(String::as_str).collect();
        let mut new_seen = HashSet::new();
        let new_ids: Vec<String> = ids
            .iter()
            .filter(|id| !known.contains(id.as_str()) && new_seen.insert(id.as_str()))
            .cloned()
            .collect();
        let mut combined = entry.seen_ids.clone();
        combined.extend(ids.iter().cloned());
        let mut seen = HashSet::new();
        combined.retain(|id| seen.insert(id.clone()));
        entry.seen_ids = combined;
        entry.last_run_at = Some(now);
        entry.updated_at = now;
        self.save(&entries)?;
        Ok(new_ids)
    }

    fn read_all(&self) -> Result<Vec<Watch>, BossError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| BossError::Watch(error.to_string()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(BossError::Watch(error.to_string())),
        }
    }

    fn save(&self, entries: &[Watch]) -> Result<(), BossError> {
        let bytes = serde_json::to_vec_pretty(entries)
            .map_err(|error| BossError::Watch(error.to_string()))?;
        atomic_write(&self.path, &bytes, |error| {
            BossError::Watch(error.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::SearchFilters;

    fn spec() -> SearchSpec {
        SearchSpec {
            query: "rust".to_owned(),
            city: None,
            page: 1,
            limit: 20,
            filters: SearchFilters::default(),
        }
    }

    #[test]
    fn identical_large_runs_never_rediscover_forgotten_ids() {
        let directory = tempdir().expect("tempdir");
        let store = WatchStore::from_paths(&DataPaths::new(directory.path()));
        store.add("daily", spec(), 1).expect("add");
        let ids: Vec<String> = (0..2_100).map(|index| format!("job-{index}")).collect();
        let first = store.record_success("daily", &ids, 2).expect("first");
        let second = store.record_success("daily", &ids, 3).expect("second");
        let watch = store.show("daily").expect("show");
        assert_eq!(
            (first.len(), second.len(), watch.seen_ids.len()),
            (2_100, 0, 2_100)
        );
    }

    #[test]
    fn overlapping_and_repeated_runs_persist_exact_deduplicated_union() {
        let directory = tempdir().expect("tempdir");
        let store = WatchStore::from_paths(&DataPaths::new(directory.path()));
        store.add("daily", spec(), 1).expect("add");
        store
            .record_success("daily", &["a".to_owned(), "b".to_owned()], 2)
            .expect("first");
        let overlap = store
            .record_success(
                "daily",
                &["b".to_owned(), "c".to_owned(), "c".to_owned()],
                3,
            )
            .expect("overlap");
        let repeated = store
            .record_success(
                "daily",
                &["a".to_owned(), "b".to_owned(), "c".to_owned()],
                4,
            )
            .expect("repeat");
        assert_eq!(
            (
                overlap,
                repeated,
                store.show("daily").expect("show").seen_ids
            ),
            (
                vec!["c".to_owned()],
                Vec::<String>::new(),
                vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
            )
        );
    }

    #[test]
    fn readd_preserves_seen_state_and_creation_time() {
        let directory = tempdir().expect("tempdir");
        let store = WatchStore::from_paths(&DataPaths::new(directory.path()));
        store.add("daily", spec(), 1).expect("add");
        store
            .record_success("daily", &["job".to_owned()], 2)
            .expect("run");
        let updated = store.add("daily", spec(), 3).expect("update");
        assert_eq!(
            (updated.created_at, updated.seen_ids),
            (1, vec!["job".to_owned()])
        );
    }
}
