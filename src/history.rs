//! Local search-attempt audit history.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::data::atomic_write;
use crate::{BossError, DataPaths, Platform, SearchFilters};

const HISTORY_LIMIT: usize = 200;

/// One provider outcome recorded for an attempted search.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoryProviderSummary {
    /// Provider name.
    pub platform: Platform,
    /// Number of locally filtered results.
    pub count: usize,
    /// Stable error code when the provider failed.
    pub error_code: Option<String>,
}

/// One completed local search attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchHistoryEntry {
    /// Unix epoch seconds.
    pub timestamp: u64,
    /// Search phrase.
    pub query: String,
    /// Selected provider name or `all`.
    pub platform: String,
    /// Logical/native city input.
    pub city: Option<String>,
    /// One-based page.
    pub page: u32,
    /// Requested result limit.
    pub limit: u32,
    /// Normalized local filters.
    pub filters: SearchFilters,
    /// Per-provider outcome summaries.
    pub providers: Vec<HistoryProviderSummary>,
}

/// Atomic capped history store.
#[derive(Clone, Debug)]
pub struct SearchHistoryStore {
    path: PathBuf,
}

impl SearchHistoryStore {
    /// Opens history from shared paths.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.history(),
        }
    }

    /// Adds a newest-first entry and caps the file at 200 records.
    pub fn record(&self, entry: SearchHistoryEntry) -> Result<(), BossError> {
        let mut entries = self.read_all()?;
        entries.insert(0, entry);
        entries.truncate(HISTORY_LIMIT);
        let bytes = serde_json::to_vec_pretty(&entries)
            .map_err(|error| BossError::HistoryJson(error.to_string()))?;
        atomic_write(&self.path, &bytes, |error| {
            BossError::HistoryIo(error.to_string())
        })
    }

    /// Lists newest local attempts, optionally by selected provider.
    pub fn list(
        &self,
        platform: Option<Platform>,
        limit: usize,
    ) -> Result<Vec<SearchHistoryEntry>, BossError> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter(|entry| platform.is_none_or(|selected| entry.platform == selected.as_str()))
            .take(limit)
            .collect())
    }

    pub(crate) fn read_all(&self) -> Result<Vec<SearchHistoryEntry>, BossError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| BossError::HistoryJson(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(BossError::HistoryIo(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn entry(index: usize, platform: &str) -> SearchHistoryEntry {
        SearchHistoryEntry {
            timestamp: index as u64,
            query: format!("query-{index}"),
            platform: platform.to_owned(),
            city: None,
            page: 1,
            limit: 20,
            filters: SearchFilters::default(),
            providers: Vec::new(),
        }
    }

    #[test]
    fn record_caps_newest_entries_at_two_hundred() {
        let directory = tempdir().expect("tempdir");
        let store = SearchHistoryStore::from_paths(&DataPaths::new(directory.path()));
        for index in 0..205 {
            store.record(entry(index, "all")).expect("record");
        }
        let entries = store.list(None, 500).expect("list");
        assert_eq!((entries.len(), entries[0].timestamp), (200, 204));
    }

    #[test]
    fn list_filters_selected_platform() {
        let directory = tempdir().expect("tempdir");
        let store = SearchHistoryStore::from_paths(&DataPaths::new(directory.path()));
        store.record(entry(1, "all")).expect("all");
        store.record(entry(2, "zhipin")).expect("zhipin");
        let entries = store.list(Some(Platform::Zhipin), 20).expect("list");
        assert_eq!(entries.len(), 1);
    }
}
