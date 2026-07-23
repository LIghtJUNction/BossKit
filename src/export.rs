//! Safe local structured and file export.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Platform;
use crate::model::redact_secrets;
use crate::{BossError, Job};

/// Local export source.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportSource {
    /// Normalized jobs cache.
    Jobs,
    /// Local shortlist snapshots.
    Shortlist,
}

/// File encoding requested by the CLI.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// Pretty JSON.
    Json,
    /// RFC-compatible CSV.
    Csv,
    /// Minimal escaped HTML table.
    Html,
}

/// Complete local export request.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Local source collection.
    pub source: ExportSource,
    /// Optional platform filter.
    pub platform: Option<Platform>,
    /// Maximum record count.
    pub limit: usize,
    /// Requested encoding or metadata format.
    pub format: ExportFormat,
    /// Optional local output path.
    pub output: Option<PathBuf>,
    /// Whether remote IDs are included.
    pub include_ids: bool,
    /// Whether an existing output file may be replaced.
    pub force: bool,
}

/// Structured export response.
#[derive(Clone, Debug, Serialize)]
pub struct ExportResult {
    /// Selected source.
    pub source: ExportSource,
    /// Requested format.
    pub format: ExportFormat,
    /// Number of jobs.
    pub count: usize,
    /// Written path, absent for structured-only output.
    pub output: Option<String>,
    /// Redacted structured jobs, always present without a path.
    pub jobs: Option<Vec<Value>>,
}

/// Builds redacted structured job objects.
#[must_use]
pub fn structured_jobs(jobs: &[Job], include_ids: bool) -> Vec<Value> {
    jobs.iter()
        .map(|job| {
            let mut value = json!({
                "id":job.id,
                "platform":job.platform,
                "title":safe(&job.title),
                "company":safe(&job.company),
                "city":safe(&job.city),
                "district":safe(&job.district),
                "salary":safe(&job.salary),
                "url":safe(&job.url),
                "experience":safe(&job.experience),
                "education":safe(&job.education),
                "employment_type":safe(&job.employment_type),
                "skills":job.skills.iter().map(|item| safe(item)).collect::<Vec<_>>(),
                "welfare":job.welfare.iter().map(|item| safe(item)).collect::<Vec<_>>(),
                "description":safe(&job.description),
                "address":safe(&job.address)
            });
            if include_ids {
                value["remote_id"] = Value::String(safe(&job.remote_id));
            }
            value
        })
        .collect()
}

/// Encodes and atomically writes the requested export file.
pub fn write_export(
    path: &Path,
    format: ExportFormat,
    jobs: &[Job],
    include_ids: bool,
    force: bool,
) -> Result<(), BossError> {
    let bytes = encode(format, jobs, include_ids)?;
    atomic_export(path, &bytes, force)
}

fn encode(format: ExportFormat, jobs: &[Job], include_ids: bool) -> Result<Vec<u8>, BossError> {
    match format {
        ExportFormat::Json => serde_json::to_vec_pretty(&structured_jobs(jobs, include_ids))
            .map_err(|error| BossError::ExportEncoding(error.to_string())),
        ExportFormat::Csv => encode_csv(jobs, include_ids),
        ExportFormat::Html => Ok(encode_html(jobs, include_ids).into_bytes()),
    }
}

fn encode_csv(jobs: &[Job], include_ids: bool) -> Result<Vec<u8>, BossError> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    let mut headers = vec![
        "id",
        "platform",
        "title",
        "company",
        "city",
        "district",
        "salary",
        "url",
        "experience",
        "education",
        "employment_type",
        "skills",
        "welfare",
        "description",
        "address",
    ];
    if include_ids {
        headers.insert(1, "remote_id");
    }
    writer
        .write_record(headers)
        .map_err(|error| BossError::ExportEncoding(error.to_string()))?;
    for job in jobs {
        let mut row = vec![csv_safe(&job.id)];
        if include_ids {
            row.push(csv_safe(&job.remote_id));
        }
        row.extend([
            job.platform.as_str().to_owned(),
            csv_safe(&job.title),
            csv_safe(&job.company),
            csv_safe(&job.city),
            csv_safe(&job.district),
            csv_safe(&job.salary),
            csv_safe(&job.url),
            csv_safe(&job.experience),
            csv_safe(&job.education),
            csv_safe(&job.employment_type),
            csv_safe(&job.skills.join(", ")),
            csv_safe(&job.welfare.join(", ")),
            csv_safe(&job.description),
            csv_safe(&job.address),
        ]);
        writer
            .write_record(row)
            .map_err(|error| BossError::ExportEncoding(error.to_string()))?;
    }
    writer
        .into_inner()
        .map_err(|error| BossError::ExportEncoding(error.to_string()))
}

fn encode_html(jobs: &[Job], include_ids: bool) -> String {
    let structured = structured_jobs(jobs, include_ids);
    let mut html = String::from(
        "<!doctype html><meta charset=\"utf-8\"><title>BossKit export</title><table><thead><tr>",
    );
    let mut fields = vec![
        "id", "platform", "title", "company", "city", "salary", "url",
    ];
    if include_ids {
        fields.insert(1, "remote_id");
    }
    for field in &fields {
        html.push_str("<th>");
        html.push_str(field);
        html.push_str("</th>");
    }
    html.push_str("</tr></thead><tbody>");
    for job in structured {
        html.push_str("<tr>");
        for field in &fields {
            html.push_str("<td>");
            html.push_str(&html_escape(
                job.get(*field).and_then(Value::as_str).unwrap_or_default(),
            ));
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html
}

fn safe(value: &str) -> String {
    redact_secrets(value)
}

fn csv_safe(value: &str) -> String {
    let redacted = safe(value);
    let mut leading_control = false;
    let first_meaningful = redacted.chars().find(|character| {
        if character.is_control() {
            leading_control = true;
            false
        } else {
            !character.is_whitespace()
        }
    });
    let dangerous = leading_control
        || matches!(
            first_meaningful,
            Some('=' | '+' | '-' | '@' | '＝' | '＋' | '－' | '＠')
        );
    if dangerous {
        format!("'{redacted}")
    } else {
        redacted
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn atomic_export(path: &Path, bytes: &[u8], force: bool) -> Result<(), BossError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| BossError::ExportIo(error.to_string()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary: PathBuf = parent.join(format!(
        ".bosskit-export-{}-{nonce}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| BossError::ExportIo(error.to_string()))?;
        file.write_all(bytes)
            .map_err(|error| BossError::ExportIo(error.to_string()))?;
        file.sync_all()
            .map_err(|error| BossError::ExportIo(error.to_string()))?;
        if force {
            fs::rename(&temporary, path).map_err(|error| BossError::ExportIo(error.to_string()))
        } else {
            fs::hard_link(&temporary, path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    BossError::ExportExists(path.display().to_string())
                } else {
                    BossError::ExportIo(error.to_string())
                }
            })?;
            fs::remove_file(&temporary).map_err(|error| BossError::ExportIo(error.to_string()))
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
    use crate::Platform;

    fn job() -> Job {
        let mut job = Job::new(
            "local",
            Platform::Zhipin,
            "remote",
            "<Rust>",
            "https://test",
        );
        job.company = "A, \"B\"".to_owned();
        job.description = "x & y".to_owned();
        job
    }

    #[test]
    fn csv_and_html_escape_content() {
        let mut fixture = job();
        fixture.company = "A, \"B\"\nC".to_owned();
        let bytes = encode_csv(&[fixture], false).expect("csv");
        let mut reader = csv::Reader::from_reader(bytes.as_slice());
        let record = reader.records().next().expect("row").expect("record");
        let html = encode_html(&[job()], false);
        assert_eq!(record.get(3), Some("A, \"B\"\nC"));
        assert!(html.contains("&lt;Rust&gt;"));
    }

    #[test]
    fn csv_neutralizes_ascii_and_full_width_formula_sigils() {
        for value in [
            "=1+1",
            "+cmd",
            "-2",
            "@SUM(A1)",
            "＝1",
            "＋1",
            "－1",
            "＠函数",
        ] {
            assert!(csv_safe(value).starts_with('\''), "value: {value:?}");
        }
    }

    #[test]
    fn csv_neutralizes_any_control_character_before_meaningful_text() {
        for value in ["\n", "\ntext", "\0text", "\u{1f}text", " \ttext", " \rtext"] {
            assert!(csv_safe(value).starts_with('\''), "value: {value:?}");
        }
    }

    #[test]
    fn csv_neutralizes_sigils_after_leading_spaces_and_controls() {
        for value in ["  =1", " \t+1", "\r－1", "\n＠函数", "\0 ＋1"] {
            assert!(csv_safe(value).starts_with('\''), "value: {value:?}");
        }
    }

    #[test]
    fn csv_preserves_safe_unicode_and_later_minus_values() {
        for value in [
            "Rust",
            "1+1",
            "text = value",
            "'already safe",
            "安全工程师",
            "Rust－平台",
            "salary - negotiable",
        ] {
            assert_eq!(csv_safe(value), value);
        }
    }

    #[test]
    fn existing_target_requires_force() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("jobs.json");
        std::fs::write(&path, b"old").expect("old");
        assert!(matches!(
            write_export(&path, ExportFormat::Json, &[job()], false, false),
            Err(BossError::ExportExists(_))
        ));
        write_export(&path, ExportFormat::Json, &[job()], false, true).expect("force");
    }

    #[test]
    fn ids_are_opt_in_except_stable_local_id() {
        let default = structured_jobs(&[job()], false);
        let included = structured_jobs(&[job()], true);
        assert!(default[0].get("remote_id").is_none() && included[0].get("remote_id").is_some());
    }

    #[test]
    fn failed_parent_creation_preserves_blocking_content() {
        let directory = tempdir().expect("tempdir");
        let blocker = directory.path().join("blocker");
        std::fs::write(&blocker, b"keep").expect("blocker");
        let result = write_export(
            &blocker.join("jobs.csv"),
            ExportFormat::Csv,
            &[job()],
            false,
            false,
        );
        assert!(
            matches!(result, Err(BossError::ExportIo(_)))
                && std::fs::read(blocker).expect("read blocker") == b"keep"
        );
    }

    #[test]
    fn structured_export_redacts_configured_cookie() {
        // SAFETY: This test restores its dedicated environment variable.
        unsafe { std::env::set_var("BOSS_ZHILIAN_COOKIE", "export-secret-cookie") };
        let mut fixture = job();
        fixture.description = "contains export-secret-cookie".to_owned();
        let output =
            serde_json::to_string(&structured_jobs(&[fixture], false)).expect("serialize export");
        // SAFETY: Restore process state before returning.
        unsafe { std::env::remove_var("BOSS_ZHILIAN_COOKIE") };
        assert!(!output.contains("export-secret-cookie"));
    }
}
