//! Strict local keyword-reply suggestions stored in one atomic JSON collection.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::data::atomic_write;
use crate::{BossError, DataPaths};

/// Maximum number of Unicode scalar values in a keyword.
pub const MAX_KEYWORD_CHARS: usize = 128;
/// Maximum number of Unicode scalar values in one suggested reply.
pub const MAX_REPLY_CHARS: usize = 2_000;
/// Maximum number of Unicode scalar values accepted for one message to match.
pub const MAX_MESSAGE_CHARS: usize = 10_000;

/// One local keyword-to-reply rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyRule {
    /// Trimmed keyword, unique with ASCII-case-insensitive identity.
    pub keyword: String,
    /// Trimmed text suggested for a matched message.
    pub reply: String,
    /// Creation Unix timestamp.
    pub created_at: u64,
    /// Last update Unix timestamp.
    pub updated_at: u64,
}

/// Deterministic local reply-matching result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReplyMatch {
    /// Whether any stored keyword matched the supplied message.
    pub matched: bool,
    /// The selected rule when a match exists, otherwise `null`.
    pub rule: Option<ReplyRule>,
}

/// Atomic local keyword-reply rule collection.
#[derive(Clone, Debug)]
pub struct ReplyStore {
    path: PathBuf,
}

impl ReplyStore {
    /// Opens reply rules below shared paths.
    #[must_use]
    pub fn from_paths(paths: &DataPaths) -> Self {
        Self {
            path: paths.reply_rules(),
        }
    }

    /// Adds or updates one local rule without changing its creation time.
    pub fn add(&self, keyword: &str, reply: &str, now: u64) -> Result<ReplyRule, BossError> {
        let keyword = normalize_keyword(keyword)?;
        let reply = normalize_reply(reply)?;
        let mut rules = self.read_all()?;
        if let Some(rule) = rules
            .iter_mut()
            .find(|rule| rule.keyword.eq_ignore_ascii_case(&keyword))
        {
            rule.reply = reply;
            rule.updated_at = now;
            let updated = rule.clone();
            self.save(&rules)?;
            return Ok(updated);
        }
        let rule = ReplyRule {
            keyword,
            reply,
            created_at: now,
            updated_at: now,
        };
        rules.push(rule.clone());
        self.save(&rules)?;
        Ok(rule)
    }

    /// Lists local rules in stored order.
    pub fn list(&self) -> Result<Vec<ReplyRule>, BossError> {
        self.read_all()
    }

    /// Removes one local rule using ASCII-case-insensitive keyword identity.
    pub fn remove(&self, keyword: &str) -> Result<ReplyRule, BossError> {
        let keyword = normalize_keyword(keyword)?;
        let mut rules = self.read_all()?;
        let index = rules
            .iter()
            .position(|rule| rule.keyword.eq_ignore_ascii_case(&keyword))
            .ok_or_else(|| BossError::Reply(format!("not found: {keyword}")))?;
        let removed = rules.remove(index);
        self.save(&rules)?;
        Ok(removed)
    }

    /// Returns the longest matching rule, retaining stored order for equal lengths.
    pub fn match_message(&self, message: &str) -> Result<ReplyMatch, BossError> {
        validate_message(message)?;
        let mut selected = None;
        let mut selected_length = 0;
        for rule in self.read_all()? {
            if contains_ascii_case_insensitive(message, &rule.keyword) {
                let length = rule.keyword.chars().count();
                if length > selected_length {
                    selected_length = length;
                    selected = Some(rule);
                }
            }
        }
        Ok(ReplyMatch {
            matched: selected.is_some(),
            rule: selected,
        })
    }

    /// Verifies that persisted reply rules can be read and are normalized.
    pub(crate) fn check_readable(&self) -> Result<(), BossError> {
        self.read_all().map(|_| ())
    }

    fn read_all(&self) -> Result<Vec<ReplyRule>, BossError> {
        let rules = match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| BossError::Reply(error.to_string()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(BossError::Reply(error.to_string())),
        };
        validate_stored_rules(&rules)?;
        Ok(rules)
    }

    fn save(&self, rules: &[ReplyRule]) -> Result<(), BossError> {
        let bytes = serde_json::to_vec_pretty(rules)
            .map_err(|error| BossError::Reply(error.to_string()))?;
        atomic_write(&self.path, &bytes, |error| {
            BossError::Reply(error.to_string())
        })
    }
}

fn normalize_keyword(keyword: &str) -> Result<String, BossError> {
    normalize_nonblank_bounded(keyword, "keyword", MAX_KEYWORD_CHARS)
}

fn normalize_reply(reply: &str) -> Result<String, BossError> {
    normalize_nonblank_bounded(reply, "reply", MAX_REPLY_CHARS)
}

fn normalize_nonblank_bounded(
    value: &str,
    field: &str,
    maximum_chars: usize,
) -> Result<String, BossError> {
    let trimmed = value.trim();
    let count = trimmed.chars().count();
    if count == 0 || count > maximum_chars {
        return Err(BossError::InvalidArgument(format!(
            "{field} must contain 1..={maximum_chars} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

fn validate_message(message: &str) -> Result<(), BossError> {
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(BossError::InvalidArgument(format!(
            "message must contain at most {MAX_MESSAGE_CHARS} characters"
        )));
    }
    Ok(())
}

fn validate_stored_rules(rules: &[ReplyRule]) -> Result<(), BossError> {
    for (index, rule) in rules.iter().enumerate() {
        let keyword_is_normalized = rule.keyword == rule.keyword.trim()
            && !rule.keyword.is_empty()
            && rule.keyword.chars().count() <= MAX_KEYWORD_CHARS;
        let reply_is_normalized = rule.reply == rule.reply.trim()
            && !rule.reply.is_empty()
            && rule.reply.chars().count() <= MAX_REPLY_CHARS;
        if !keyword_is_normalized || !reply_is_normalized {
            return Err(BossError::Reply(
                "stored reply rule contains invalid or unnormalized text".to_owned(),
            ));
        }
        if rules[..index]
            .iter()
            .any(|previous| previous.keyword.eq_ignore_ascii_case(&rule.keyword))
        {
            return Err(BossError::Reply(
                "stored reply rules contain duplicate keywords".to_owned(),
            ));
        }
    }
    Ok(())
}

fn contains_ascii_case_insensitive(message: &str, keyword: &str) -> bool {
    let message = message.as_bytes();
    let keyword = keyword.as_bytes();
    message
        .windows(keyword.len())
        .any(|candidate| candidate.eq_ignore_ascii_case(keyword))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn store() -> (tempfile::TempDir, ReplyStore) {
        let directory = tempdir().expect("temporary directory");
        let store = ReplyStore::from_paths(&DataPaths::new(directory.path()));
        (directory, store)
    }

    #[test]
    fn storage_normalizes_input_and_rejects_blank_or_oversized_values() {
        let (_directory, store) = store();
        let rule = store.add("  Offer  ", "  Thanks!  ", 1).expect("add");
        assert_eq!(
            (rule.keyword.as_str(), rule.reply.as_str()),
            ("Offer", "Thanks!")
        );
        assert!(store.add(" ", "reply", 2).is_err());
        assert!(store.add("keyword", "\t", 2).is_err());
        assert!(
            store
                .add(&"k".repeat(MAX_KEYWORD_CHARS + 1), "reply", 2)
                .is_err()
        );
        assert!(
            store
                .add("keyword", &"r".repeat(MAX_REPLY_CHARS + 1), 2)
                .is_err()
        );
    }

    #[test]
    fn equivalent_keywords_update_without_duplicate_and_preserve_creation_time() {
        let (_directory, store) = store();
        store.add("Offer", "first", 10).expect("add");
        let updated = store.add(" offer ", "second", 20).expect("update");
        assert_eq!(
            (
                store.list().expect("list").len(),
                updated.created_at,
                updated.updated_at,
                updated.reply.as_str(),
            ),
            (1, 10, 20, "second")
        );
    }

    #[test]
    fn matching_prefers_longest_keyword_then_first_stored_rule() {
        let (_directory, primary_store) = store();
        primary_store.add("rust", "short", 1).expect("short rule");
        primary_store
            .add("RUST engineer", "longest", 2)
            .expect("long rule");
        let longest = primary_store
            .match_message("Looking for a Rust Engineer")
            .expect("match");
        assert_eq!(
            longest.rule.as_ref().map(|rule| rule.reply.as_str()),
            Some("longest")
        );

        let (_directory, tie_store) = store();
        tie_store.add("first", "first reply", 1).expect("first");
        tie_store.add("other", "other reply", 2).expect("other");
        let tied = tie_store
            .match_message("OTHER before FIRST")
            .expect("tie match");
        assert_eq!(
            tied.rule.as_ref().map(|rule| rule.reply.as_str()),
            Some("first reply")
        );
    }

    #[test]
    fn unmatched_messages_return_an_explicit_no_match_result() {
        let (_directory, store) = store();
        store.add("offer", "thanks", 1).expect("add");
        assert_eq!(
            store.match_message("no relevant text").expect("match"),
            ReplyMatch {
                matched: false,
                rule: None,
            }
        );
    }

    #[test]
    fn stored_rules_reject_unknown_fields() {
        let (directory, store) = store();
        fs::write(
            directory.path().join("reply_rules.json"),
            br#"[{"keyword":"offer","reply":"thanks","created_at":1,"updated_at":1,"extra":true}]"#,
        )
        .expect("write");
        assert!(store.list().is_err());
    }
}
