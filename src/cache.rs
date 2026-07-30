//! Atomic local JSON job cache.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use crate::data::atomic_write;
use crate::{BossError, DataPaths, Job, Platform};

/// Local normalized job cache.
#[derive(Debug)]
pub struct JobCache {
    path: PathBuf,
}

impl JobCache {
    /// Resolves the cache from `BOSS_DATA_DIR`, the platform data directory, or `.boss`.
    #[must_use]
    pub fn discover() -> Self {
        Self::from_paths(&DataPaths::discover())
    }

    /// Creates a cache rooted at `data_dir`.
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self::from_paths(&DataPaths::new(data_dir))
    }

    /// Creates a cache from shared application paths.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self { path: paths.jobs() }
    }

    /// Reads jobs in newest/search order.
    pub fn list(&self, platform: Option<Platform>, limit: usize) -> Result<Vec<Job>, BossError> {
        let jobs = self.read_all()?;
        Ok(jobs
            .into_iter()
            .filter(|job| platform.is_none_or(|selected| job.platform == selected))
            .take(limit)
            .collect())
    }

    /// Looks up a job by stable local ID.
    pub fn show(&self, id: &str) -> Result<Option<Job>, BossError> {
        Ok(self.read_all()?.into_iter().find(|job| job.id == id))
    }

    /// Merges new jobs at the front and replaces the cache atomically.
    pub fn save(&self, incoming: &[Job]) -> Result<(), BossError> {
        let existing = self.read_all()?;
        let mut existing_by_id: HashMap<&str, &Job> = HashMap::new();
        for job in &existing {
            existing_by_id.entry(job.id.as_str()).or_insert(job);
        }
        let mut seen = HashSet::new();
        let mut jobs = Vec::with_capacity(incoming.len() + existing.len());
        for job in incoming {
            if job.platform != Platform::Zhipin {
                continue;
            }
            if seen.insert(job.id.as_str()) {
                jobs.push(
                    existing_by_id
                        .get(job.id.as_str())
                        .map_or_else(|| job.clone(), |cached| merge_job(cached, job)),
                );
            }
        }
        jobs.extend(
            existing
                .iter()
                .filter(|job| seen.insert(job.id.as_str()))
                .cloned(),
        );
        let bytes = serde_json::to_vec_pretty(&jobs)?;
        atomic_write(&self.path, &bytes, BossError::CacheIo)
    }

    /// Replaces or inserts one enriched job at the front atomically.
    pub fn upsert(&self, job: Job) -> Result<(), BossError> {
        self.save(std::slice::from_ref(&job))
    }

    /// Returns every cached job in newest order.
    pub fn all(&self) -> Result<Vec<Job>, BossError> {
        self.read_all()
    }

    pub(crate) fn read_all(&self) -> Result<Vec<Job>, BossError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice::<Vec<Job>>(&bytes)?
                .into_iter()
                .filter(|job| job.platform == Platform::Zhipin)
                .collect()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }
}

fn merge_job(cached: &Job, incoming: &Job) -> Job {
    let mut merged = cached.clone();
    merged.platform = incoming.platform;
    merge_text(&mut merged.remote_id, &incoming.remote_id);
    merge_text(&mut merged.title, &incoming.title);
    merge_text(&mut merged.company, &incoming.company);
    merge_text(&mut merged.city, &incoming.city);
    merge_text(&mut merged.salary, &incoming.salary);
    merge_text(&mut merged.url, &incoming.url);
    merge_text(&mut merged.district, &incoming.district);
    merge_text(&mut merged.experience, &incoming.experience);
    merge_text(&mut merged.education, &incoming.education);
    merge_text(&mut merged.employment_type, &incoming.employment_type);
    merge_list(&mut merged.skills, &incoming.skills);
    merge_list(&mut merged.welfare, &incoming.welfare);
    merge_text(&mut merged.description, &incoming.description);
    merge_text(&mut merged.address, &incoming.address);
    merged
}

fn merge_text(cached: &mut String, incoming: &str) {
    if !incoming.is_empty() {
        incoming.clone_into(cached);
    }
}

fn merge_list(cached: &mut Vec<String>, incoming: &[String]) {
    if !incoming.is_empty() {
        incoming.clone_into(cached);
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn cache_roundtrip_supports_list_and_lookup() {
        let directory = tempdir().expect("temporary directory");
        let cache = JobCache::new(directory.path());
        let mut job = Job::new(
            "job-1",
            Platform::Zhipin,
            "1",
            "Rust",
            "https://example.test/1",
        );
        job.company = "Example".to_owned();
        job.city = "深圳".to_owned();
        job.salary = "20-30K".to_owned();
        cache.save(std::slice::from_ref(&job)).expect("save");
        assert_eq!(cache.show("job-1").expect("show"), Some(job));
    }

    #[test]
    fn old_eight_field_cache_deserializes_with_empty_discovery_fields() {
        let directory = tempdir().expect("temporary directory");
        std::fs::write(
            directory.path().join("jobs.json"),
            r#"[{"id":"old","platform":"zhipin","remote_id":"1","title":"Rust","company":"Example","city":"深圳","salary":"20K","url":"https://example.test"}]"#.as_bytes(),
        )
        .expect("fixture");
        let job = JobCache::new(directory.path())
            .show("old")
            .expect("read")
            .expect("job");
        assert!(job.description.is_empty() && job.welfare.is_empty());
    }

    #[test]
    fn legacy_non_boss_records_are_excluded_from_every_cache_read() {
        let directory = tempdir().expect("temporary directory");
        std::fs::write(
            directory.path().join("jobs.json"),
            r#"[
                {"id":"boss","platform":"zhipin","remote_id":"1","title":"Rust","company":"Example","city":"深圳","salary":"20K","url":"https://example.test/boss"},
                {"id":"legacy-zhilian","platform":"zhilian","remote_id":"2","title":"Rust","company":"Example","city":"深圳","salary":"20K","url":"https://example.test/legacy-zhilian"},
                {"id":"legacy-qiancheng","platform":"qiancheng","remote_id":"3","title":"Rust","company":"Example","city":"深圳","salary":"20K","url":"https://example.test/legacy-qiancheng"}
            ]"#,
        )
        .expect("fixture");

        let cache = JobCache::new(directory.path());
        assert_eq!(cache.all().expect("all").len(), 1);
        assert!(cache.show("legacy-zhilian").expect("show").is_none());
        assert!(cache.show("legacy-qiancheng").expect("show").is_none());
    }

    #[test]
    fn sparse_incoming_job_preserves_cached_enrichment_and_updates_base_fields() {
        let directory = tempdir().expect("temporary directory");
        let cache = JobCache::new(directory.path());
        let mut enriched = Job::new(
            "same",
            Platform::Zhipin,
            "remote",
            "Old title",
            "https://example.test/old",
        );
        enriched.description = "Detailed role".to_owned();
        enriched.address = "Tech park".to_owned();
        enriched.skills = vec!["Rust".to_owned()];
        cache.save(&[enriched]).expect("seed");

        let sparse = Job::new(
            "same",
            Platform::Zhipin,
            "",
            "New title",
            "https://example.test/new",
        );
        cache.save(&[sparse]).expect("merge");
        let merged = cache.show("same").expect("show").expect("job");

        assert_eq!(
            (
                merged.title,
                merged.url,
                merged.remote_id,
                merged.description,
                merged.address,
                merged.skills,
            ),
            (
                "New title".to_owned(),
                "https://example.test/new".to_owned(),
                "remote".to_owned(),
                "Detailed role".to_owned(),
                "Tech park".to_owned(),
                vec!["Rust".to_owned()],
            )
        );
    }

    #[test]
    fn enriched_incoming_job_replaces_sparse_cached_fields() {
        let directory = tempdir().expect("temporary directory");
        let cache = JobCache::new(directory.path());
        cache
            .save(&[Job::new(
                "same",
                Platform::Zhipin,
                "remote",
                "Rust",
                "https://example.test/job",
            )])
            .expect("seed");
        let mut enriched = Job::new(
            "same",
            Platform::Zhipin,
            "remote",
            "Rust",
            "https://example.test/job",
        );
        enriched.description = "Detailed role".to_owned();
        enriched.welfare = vec!["Remote".to_owned()];

        cache.save(&[enriched]).expect("merge");
        let merged = cache.show("same").expect("show").expect("job");

        assert_eq!(
            (merged.description, merged.welfare),
            ("Detailed role".to_owned(), vec!["Remote".to_owned()])
        );
    }

    #[test]
    fn incoming_order_and_first_duplicate_are_preserved() {
        let directory = tempdir().expect("temporary directory");
        let cache = JobCache::new(directory.path());
        cache
            .save(&[
                Job::new("old", Platform::Zhipin, "old", "Old", "https://old"),
                Job::new("same", Platform::Zhipin, "same", "Cached", "https://same"),
            ])
            .expect("seed");
        cache
            .save(&[
                Job::new("same", Platform::Zhipin, "same", "First", "https://same"),
                Job::new("new", Platform::Zhipin, "new", "New", "https://new"),
                Job::new("same", Platform::Zhipin, "same", "Second", "https://same"),
            ])
            .expect("merge");
        let jobs = cache.all().expect("all");

        assert_eq!(
            jobs.iter()
                .map(|job| (job.id.as_str(), job.title.as_str()))
                .collect::<Vec<_>>(),
            vec![("same", "First"), ("new", "New"), ("old", "Old")]
        );
    }
}
