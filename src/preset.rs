//! Atomic local saved-search presets.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::data::atomic_write;
use crate::{BossError, DataPaths, SearchSpec};

/// One named saved search.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    /// Unique trimmed local name.
    pub name: String,
    /// Complete validated search snapshot.
    pub spec: SearchSpec,
    /// Creation Unix timestamp.
    pub created_at: u64,
    /// Last update Unix timestamp.
    pub updated_at: u64,
}

/// Atomic preset collection.
pub struct PresetStore {
    path: PathBuf,
}

impl PresetStore {
    /// Opens presets below shared paths.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.presets(),
        }
    }

    /// Adds or updates one preset without duplication.
    pub fn add(&self, name: &str, spec: SearchSpec, now: u64) -> Result<Preset, BossError> {
        let name = validate_name(name)?;
        let mut entries = self.read_all()?;
        if let Some(entry) = entries.iter_mut().find(|entry| entry.name == name) {
            entry.spec = spec;
            entry.updated_at = now;
            let updated = entry.clone();
            self.save(&entries)?;
            return Ok(updated);
        }
        let entry = Preset {
            name,
            spec,
            created_at: now,
            updated_at: now,
        };
        entries.push(entry.clone());
        self.save(&entries)?;
        Ok(entry)
    }

    /// Lists every preset.
    pub fn list(&self) -> Result<Vec<Preset>, BossError> {
        self.read_all()
    }

    /// Shows one preset.
    pub fn show(&self, name: &str) -> Result<Preset, BossError> {
        let name = validate_name(name)?;
        self.read_all()?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| BossError::Preset(format!("not found: {name}")))
    }

    /// Removes one preset.
    pub fn remove(&self, name: &str) -> Result<Preset, BossError> {
        let name = validate_name(name)?;
        let mut entries = self.read_all()?;
        let index = entries
            .iter()
            .position(|entry| entry.name == name)
            .ok_or_else(|| BossError::Preset(format!("not found: {name}")))?;
        let removed = entries.remove(index);
        self.save(&entries)?;
        Ok(removed)
    }

    fn read_all(&self) -> Result<Vec<Preset>, BossError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| BossError::Preset(error.to_string()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(BossError::Preset(error.to_string())),
        }
    }

    fn save(&self, entries: &[Preset]) -> Result<(), BossError> {
        let bytes = serde_json::to_vec_pretty(entries)
            .map_err(|error| BossError::Preset(error.to_string()))?;
        atomic_write(&self.path, &bytes, |error| {
            BossError::Preset(error.to_string())
        })
    }
}

pub(crate) fn validate_name(name: &str) -> Result<String, BossError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        return Err(BossError::InvalidArgument(
            "name must contain 1..64 characters".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::SearchFilters;

    fn spec(query: &str) -> SearchSpec {
        SearchSpec {
            query: query.to_owned(),
            city: None,
            page: 1,
            limit: 20,
            filters: SearchFilters::default(),
        }
    }

    #[test]
    fn readd_updates_without_duplicate_and_preserves_creation_time() {
        let directory = tempdir().expect("tempdir");
        let store = PresetStore::from_paths(&DataPaths::new(directory.path()));
        store.add(" rust ", spec("rust"), 1).expect("add");
        let updated = store.add("rust", spec("backend"), 2).expect("update");
        assert_eq!(
            (
                store.list().expect("list").len(),
                updated.created_at,
                updated.updated_at
            ),
            (1, 1, 2)
        );
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name(" name ").expect("name"), "name");
        assert!(validate_name("").is_err() && validate_name(&"x".repeat(65)).is_err());
    }
}
