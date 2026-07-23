//! Strict typed local resumes stored in one atomic JSON collection.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::data::atomic_write;
use crate::preset::validate_name;
use crate::{BossError, DataPaths};

/// Typed work-experience entry.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeExperience {
    /// Employer.
    pub company: String,
    /// Role.
    pub role: String,
    /// Start date text.
    pub start_date: String,
    /// End date text.
    pub end_date: String,
    /// Role summary.
    pub summary: String,
}

/// Typed education entry.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeEducation {
    /// Institution.
    pub institution: String,
    /// Degree.
    pub degree: String,
    /// Field of study.
    pub field: String,
    /// Start date text.
    pub start_date: String,
    /// End date text.
    pub end_date: String,
}

/// Typed project entry.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeProject {
    /// Project name.
    pub name: String,
    /// Project description.
    pub description: String,
    /// Optional URL text.
    pub url: String,
}

/// One strict local resume document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeDocument {
    /// Unique local name.
    pub name: String,
    /// Headline or target role.
    pub title: String,
    /// Profile summary.
    pub summary: String,
    /// Named basic facts.
    pub basics: BTreeMap<String, String>,
    /// Deduplicated skills.
    pub skills: Vec<String>,
    /// Typed work history.
    pub experience: Vec<ResumeExperience>,
    /// Typed education history.
    pub education: Vec<ResumeEducation>,
    /// Typed projects.
    pub projects: Vec<ResumeProject>,
    /// Creation timestamp.
    pub created_at: u64,
    /// Last update timestamp.
    pub updated_at: u64,
}

/// Stable resume comparison summary.
#[derive(Clone, Debug, Serialize)]
pub struct ResumeDiff {
    /// Left resume name.
    pub left: String,
    /// Right resume name.
    pub right: String,
    /// Names of fields whose values differ.
    pub changed_fields: Vec<String>,
}

/// Atomic typed resume collection.
pub struct ResumeStore {
    path: PathBuf,
}

impl ResumeStore {
    /// Opens the shared resume collection.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.resumes(),
        }
    }

    /// Initializes one new resume.
    pub fn init(
        &self,
        name: &str,
        title: Option<String>,
        now: u64,
    ) -> Result<ResumeDocument, BossError> {
        let name = validate_name(name)?;
        let mut documents = self.read_all()?;
        if documents.iter().any(|document| document.name == name) {
            return Err(BossError::Resume(format!("already exists: {name}")));
        }
        let document = ResumeDocument {
            name,
            title: title.unwrap_or_default().trim().to_owned(),
            summary: String::new(),
            basics: BTreeMap::new(),
            skills: Vec::new(),
            experience: Vec::new(),
            education: Vec::new(),
            projects: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        documents.push(document.clone());
        self.save(&documents)?;
        Ok(document)
    }

    /// Lists every resume.
    pub fn list(&self) -> Result<Vec<ResumeDocument>, BossError> {
        self.read_all()
    }

    /// Shows one resume.
    pub fn show(&self, name: &str) -> Result<ResumeDocument, BossError> {
        let name = validate_name(name)?;
        self.read_all()?
            .into_iter()
            .find(|document| document.name == name)
            .ok_or_else(|| BossError::Resume(format!("not found: {name}")))
    }

    /// Sets an allow-listed scalar or basic field.
    pub fn set(
        &self,
        name: &str,
        field: &str,
        value: String,
        now: u64,
    ) -> Result<ResumeDocument, BossError> {
        let name = validate_name(name)?;
        let mut documents = self.read_all()?;
        let document = find_mut(&mut documents, &name)?;
        match field {
            "title" => document.title = value,
            "summary" => document.summary = value,
            _ => {
                let key = field.strip_prefix("basics.").ok_or_else(|| {
                    BossError::InvalidArgument(format!("unsupported resume field: {field}"))
                })?;
                let key = key.trim();
                if key.is_empty() {
                    return Err(BossError::InvalidArgument(
                        "basics key must not be empty".to_owned(),
                    ));
                }
                document.basics.insert(key.to_owned(), value);
            }
        }
        document.updated_at = now;
        let updated = document.clone();
        self.save(&documents)?;
        Ok(updated)
    }

    /// Adds and removes normalized skills.
    pub fn skills(
        &self,
        name: &str,
        add: Vec<String>,
        remove: Vec<String>,
        now: u64,
    ) -> Result<ResumeDocument, BossError> {
        let name = validate_name(name)?;
        let mut documents = self.read_all()?;
        let document = find_mut(&mut documents, &name)?;
        let remove = normalize_skills(remove)?;
        document
            .skills
            .retain(|skill| !remove.iter().any(|item| item.eq_ignore_ascii_case(skill)));
        document.skills.extend(normalize_skills(add)?);
        document.skills = normalize_skills(std::mem::take(&mut document.skills))?;
        document.updated_at = now;
        let updated = document.clone();
        self.save(&documents)?;
        Ok(updated)
    }

    /// Clones one resume under a new name.
    pub fn clone_document(
        &self,
        name: &str,
        new_name: &str,
        now: u64,
    ) -> Result<ResumeDocument, BossError> {
        let source = self.show(name)?;
        let new_name = validate_name(new_name)?;
        let mut documents = self.read_all()?;
        if documents.iter().any(|document| document.name == new_name) {
            return Err(BossError::Resume(format!("already exists: {new_name}")));
        }
        let mut cloned = source;
        cloned.name = new_name;
        cloned.created_at = now;
        cloned.updated_at = now;
        documents.push(cloned.clone());
        self.save(&documents)?;
        Ok(cloned)
    }

    /// Compares two typed documents.
    pub fn diff(&self, left: &str, right: &str) -> Result<ResumeDiff, BossError> {
        let left_document = self.show(left)?;
        let right_document = self.show(right)?;
        let mut changed_fields = Vec::new();
        if left_document.title != right_document.title {
            changed_fields.push("title".to_owned());
        }
        if left_document.summary != right_document.summary {
            changed_fields.push("summary".to_owned());
        }
        if left_document.basics != right_document.basics {
            changed_fields.push("basics".to_owned());
        }
        if left_document.skills != right_document.skills {
            changed_fields.push("skills".to_owned());
        }
        if left_document.experience != right_document.experience {
            changed_fields.push("experience".to_owned());
        }
        if left_document.education != right_document.education {
            changed_fields.push("education".to_owned());
        }
        if left_document.projects != right_document.projects {
            changed_fields.push("projects".to_owned());
        }
        Ok(ResumeDiff {
            left: left_document.name,
            right: right_document.name,
            changed_fields,
        })
    }

    /// Imports one strict document into the collection.
    pub fn import(
        &self,
        mut document: ResumeDocument,
        force: bool,
    ) -> Result<ResumeDocument, BossError> {
        document.name = validate_name(&document.name)?;
        document.skills = normalize_skills(document.skills)?;
        let mut documents = self.read_all()?;
        if let Some(existing) = documents
            .iter_mut()
            .find(|existing| existing.name == document.name)
        {
            if !force {
                return Err(BossError::Resume(format!(
                    "already exists: {}",
                    document.name
                )));
            }
            *existing = document.clone();
        } else {
            documents.push(document.clone());
        }
        self.save(&documents)?;
        Ok(document)
    }

    /// Removes one confirmed resume.
    pub fn remove(&self, name: &str) -> Result<ResumeDocument, BossError> {
        let name = validate_name(name)?;
        let mut documents = self.read_all()?;
        let index = documents
            .iter()
            .position(|document| document.name == name)
            .ok_or_else(|| BossError::Resume(format!("not found: {name}")))?;
        let removed = documents.remove(index);
        self.save(&documents)?;
        Ok(removed)
    }

    fn read_all(&self) -> Result<Vec<ResumeDocument>, BossError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| BossError::Resume(error.to_string()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(BossError::Resume(error.to_string())),
        }
    }

    fn save(&self, documents: &[ResumeDocument]) -> Result<(), BossError> {
        let bytes = serde_json::to_vec_pretty(documents)
            .map_err(|error| BossError::Resume(error.to_string()))?;
        atomic_write(&self.path, &bytes, |error| {
            BossError::Resume(error.to_string())
        })
    }
}

fn find_mut<'a>(
    documents: &'a mut [ResumeDocument],
    name: &str,
) -> Result<&'a mut ResumeDocument, BossError> {
    documents
        .iter_mut()
        .find(|document| document.name == name)
        .ok_or_else(|| BossError::Resume(format!("not found: {name}")))
}

fn normalize_skills(skills: Vec<String>) -> Result<Vec<String>, BossError> {
    let mut normalized: Vec<String> = Vec::new();
    for skill in skills {
        let skill = skill.trim();
        if skill.is_empty() {
            return Err(BossError::InvalidArgument(
                "skills must not contain empty values".to_owned(),
            ));
        }
        if !normalized
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(skill))
        {
            normalized.push(skill.to_owned());
        }
    }
    Ok(normalized)
}

/// Writes one strict JSON resume atomically without unintended clobbering.
pub fn export_document(
    path: &Path,
    document: &ResumeDocument,
    force: bool,
) -> Result<(), BossError> {
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|error| BossError::Resume(error.to_string()))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| BossError::Resume(error.to_string()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = parent.join(format!(
        ".bosskit-resume-{}-{nonce}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| BossError::Resume(error.to_string()))?;
        file.write_all(&bytes)
            .map_err(|error| BossError::Resume(error.to_string()))?;
        file.sync_all()
            .map_err(|error| BossError::Resume(error.to_string()))?;
        if force {
            fs::rename(&temporary, path).map_err(|error| BossError::Resume(error.to_string()))
        } else {
            fs::hard_link(&temporary, path)
                .map_err(|error| BossError::Resume(error.to_string()))?;
            fs::remove_file(&temporary).map_err(|error| BossError::Resume(error.to_string()))
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn typed_lifecycle_normalizes_skills_and_reports_diff() {
        let directory = tempdir().expect("tempdir");
        let store = ResumeStore::from_paths(&DataPaths::new(directory.path()));
        store
            .init("base", Some("Engineer".to_owned()), 1)
            .expect("init");
        store
            .skills(
                "base",
                vec![" Rust ".to_owned(), "rust".to_owned()],
                Vec::new(),
                2,
            )
            .expect("skills");
        store.clone_document("base", "target", 3).expect("clone");
        store
            .set("target", "summary", "Platform".to_owned(), 4)
            .expect("set");
        let diff = store.diff("base", "target").expect("diff");
        assert_eq!(
            (
                store.show("base").expect("show").skills,
                diff.changed_fields
            ),
            (vec!["Rust".to_owned()], vec!["summary".to_owned()])
        );
    }

    #[test]
    fn unsupported_fields_and_unconfirmed_overwrite_are_rejected() {
        let directory = tempdir().expect("tempdir");
        let store = ResumeStore::from_paths(&DataPaths::new(directory.path()));
        let document = store.init("base", None, 1).expect("init");
        assert!(
            store
                .set("base", "experience.0", "x".to_owned(), 2)
                .is_err()
                && store.import(document, false).is_err()
        );
    }

    #[test]
    fn export_is_atomic_and_does_not_clobber_without_force() {
        let directory = tempdir().expect("tempdir");
        let store = ResumeStore::from_paths(&DataPaths::new(directory.path()));
        let document = store.init("base", None, 1).expect("init");
        let output = directory.path().join("resume.json");
        export_document(&output, &document, false).expect("export");
        assert!(export_document(&output, &document, false).is_err());
        export_document(&output, &document, true).expect("force");
    }
}
