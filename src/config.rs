//! Typed, allow-listed user configuration.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::data::atomic_write;
use crate::{BossError, DataPaths, Platform};

/// Configured default platform selection.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigPlatform {
    /// Search all registered providers.
    #[default]
    All,
    /// BOSS 直聘 only.
    Zhipin,
    /// 智联招聘 only.
    Zhilian,
    /// 前程无忧 / 51job only.
    Qiancheng,
}

impl ConfigPlatform {
    /// Converts to an optional provider selector.
    #[must_use]
    pub const fn selected(self) -> Option<Platform> {
        match self {
            Self::All => None,
            Self::Zhipin => Some(Platform::Zhipin),
            Self::Zhilian => Some(Platform::Zhilian),
            Self::Qiancheng => Some(Platform::Qiancheng),
        }
    }
}

/// Supported log verbosity.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Errors only.
    #[default]
    Error,
    /// Errors and warnings.
    Warn,
    /// Informational messages.
    Info,
    /// Debug diagnostics.
    Debug,
}

/// Supported operating mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingMode {
    /// Human-assisted read-only operation.
    #[default]
    Assisted,
}

/// Fully merged effective configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppConfig {
    /// Default provider selection.
    pub platform: ConfigPlatform,
    /// Provider request timeout.
    pub request_timeout_secs: u64,
    /// Default result count.
    pub page_size: usize,
    /// Restrained execution mode.
    pub operating_mode: OperatingMode,
    /// Diagnostic verbosity.
    pub log_level: LogLevel,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            platform: ConfigPlatform::All,
            request_timeout_secs: 15,
            page_size: 20,
            operating_mode: OperatingMode::Assisted,
            log_level: LogLevel::Error,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct UserOverrides {
    platform: Option<ConfigPlatform>,
    request_timeout_secs: Option<u64>,
    page_size: Option<usize>,
    operating_mode: Option<OperatingMode>,
    log_level: Option<LogLevel>,
}

/// One effective configuration value and its source.
#[derive(Clone, Debug, Serialize)]
pub struct ConfigEntry {
    /// Stable public key.
    pub key: &'static str,
    /// Effective typed value.
    pub value: Value,
    /// `default` or `user`.
    pub source: &'static str,
}

/// Honest before/after result for a mutation.
#[derive(Clone, Debug, Serialize)]
pub struct ConfigChange {
    /// Changed key, or `all` for a full reset.
    pub key: String,
    /// Previous effective value.
    pub previous: Value,
    /// New effective value.
    pub new: Value,
}

/// Persistent typed configuration store.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
    overrides: UserOverrides,
}

impl ConfigStore {
    /// Opens configuration at the discovered shared data root.
    pub fn discover() -> Result<Self, BossError> {
        Self::from_paths(&DataPaths::discover())
    }

    /// Opens configuration from shared paths.
    pub fn from_paths(paths: &DataPaths) -> Result<Self, BossError> {
        let path = paths.config();
        let overrides = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| BossError::ConfigJson(error.to_string()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => UserOverrides::default(),
            Err(error) => return Err(BossError::ConfigIo(error.to_string())),
        };
        let store = Self { path, overrides };
        store.validate()?;
        Ok(store)
    }

    /// Returns the effective configuration.
    #[must_use]
    pub fn effective(&self) -> AppConfig {
        let defaults = AppConfig::default();
        AppConfig {
            platform: self.overrides.platform.unwrap_or(defaults.platform),
            request_timeout_secs: self
                .overrides
                .request_timeout_secs
                .unwrap_or(defaults.request_timeout_secs),
            page_size: self.overrides.page_size.unwrap_or(defaults.page_size),
            operating_mode: self
                .overrides
                .operating_mode
                .unwrap_or(defaults.operating_mode),
            log_level: self.overrides.log_level.unwrap_or(defaults.log_level),
        }
    }

    /// Lists every safe public key.
    pub fn list(&self) -> Result<Vec<ConfigEntry>, BossError> {
        [
            "platform",
            "request_timeout_secs",
            "page_size",
            "operating_mode",
            "log_level",
        ]
        .into_iter()
        .map(|key| self.entry(key))
        .collect()
    }

    /// Reads one safe public key.
    pub fn get(&self, key: &str) -> Result<ConfigEntry, BossError> {
        self.entry(key)
    }

    /// Validates and persists one user override.
    pub fn set(&mut self, key: &str, raw: &str) -> Result<ConfigChange, BossError> {
        let previous = self.get(key)?.value;
        match key {
            "platform" => {
                self.overrides.platform = Some(parse_json_enum(raw, key)?);
            }
            "request_timeout_secs" => {
                self.overrides.request_timeout_secs = Some(parse_positive(raw, key)?);
            }
            "page_size" => {
                self.overrides.page_size = Some(parse_positive(raw, key)?);
            }
            "operating_mode" => {
                self.overrides.operating_mode = Some(parse_json_enum(raw, key)?);
            }
            "log_level" => {
                self.overrides.log_level = Some(parse_json_enum(raw, key)?);
            }
            _ => return Err(unknown_key(key)),
        }
        self.persist()?;
        Ok(ConfigChange {
            key: key.to_owned(),
            previous,
            new: self.get(key)?.value,
        })
    }

    /// Resets one override or every override.
    pub fn reset(&mut self, key: Option<&str>) -> Result<ConfigChange, BossError> {
        if let Some(key) = key {
            let previous = self.get(key)?.value;
            match key {
                "platform" => self.overrides.platform = None,
                "request_timeout_secs" => self.overrides.request_timeout_secs = None,
                "page_size" => self.overrides.page_size = None,
                "operating_mode" => self.overrides.operating_mode = None,
                "log_level" => self.overrides.log_level = None,
                _ => return Err(unknown_key(key)),
            }
            self.persist()?;
            Ok(ConfigChange {
                key: key.to_owned(),
                previous,
                new: self.get(key)?.value,
            })
        } else {
            let previous = serde_json::to_value(self.effective())
                .map_err(|error| BossError::ConfigJson(error.to_string()))?;
            self.overrides = UserOverrides::default();
            self.persist()?;
            Ok(ConfigChange {
                key: "all".to_owned(),
                previous,
                new: serde_json::to_value(self.effective())
                    .map_err(|error| BossError::ConfigJson(error.to_string()))?,
            })
        }
    }

    fn entry(&self, key: &str) -> Result<ConfigEntry, BossError> {
        let config = self.effective();
        let (value, user) = match key {
            "platform" => (json!(config.platform), self.overrides.platform.is_some()),
            "request_timeout_secs" => (
                json!(config.request_timeout_secs),
                self.overrides.request_timeout_secs.is_some(),
            ),
            "page_size" => (json!(config.page_size), self.overrides.page_size.is_some()),
            "operating_mode" => (
                json!(config.operating_mode),
                self.overrides.operating_mode.is_some(),
            ),
            "log_level" => (json!(config.log_level), self.overrides.log_level.is_some()),
            _ => return Err(unknown_key(key)),
        };
        Ok(ConfigEntry {
            key: match key {
                "platform" => "platform",
                "request_timeout_secs" => "request_timeout_secs",
                "page_size" => "page_size",
                "operating_mode" => "operating_mode",
                "log_level" => "log_level",
                _ => return Err(unknown_key(key)),
            },
            value,
            source: if user { "user" } else { "default" },
        })
    }

    fn validate(&self) -> Result<(), BossError> {
        if self.overrides.request_timeout_secs == Some(0) {
            return Err(BossError::ConfigJson(
                "request_timeout_secs must be positive".to_owned(),
            ));
        }
        if self.overrides.page_size == Some(0) {
            return Err(BossError::ConfigJson(
                "page_size must be positive".to_owned(),
            ));
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), BossError> {
        let bytes = serde_json::to_vec_pretty(&self.overrides)
            .map_err(|error| BossError::ConfigJson(error.to_string()))?;
        atomic_write(&self.path, &bytes, |error| {
            BossError::ConfigIo(error.to_string())
        })
    }
}

fn unknown_key(key: &str) -> BossError {
    BossError::InvalidArgument(format!("unknown or unsafe config key: {key}"))
}

fn parse_positive<T>(raw: &str, key: &str) -> Result<T, BossError>
where
    T: std::str::FromStr + PartialEq + From<u8>,
{
    raw.parse::<T>()
        .ok()
        .filter(|value| *value != T::from(0))
        .ok_or_else(|| BossError::InvalidArgument(format!("{key} must be a positive integer")))
}

fn parse_json_enum<T: for<'de> Deserialize<'de>>(raw: &str, key: &str) -> Result<T, BossError> {
    serde_json::from_value(Value::String(raw.to_owned()))
        .map_err(|_| BossError::InvalidArgument(format!("invalid {key}: {raw}")))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn store() -> (tempfile::TempDir, ConfigStore) {
        let directory = tempdir().expect("tempdir");
        let store =
            ConfigStore::from_paths(&DataPaths::new(directory.path())).expect("config store");
        (directory, store)
    }

    #[test]
    fn defaults_are_safe_and_complete() {
        let (_directory, store) = store();
        assert_eq!(store.effective(), AppConfig::default());
    }

    #[test]
    fn set_roundtrips_and_reports_user_source() {
        let (directory, mut store) = store();
        store.set("page_size", "7").expect("set");
        let reopened =
            ConfigStore::from_paths(&DataPaths::new(directory.path())).expect("reopen config");
        assert_eq!(reopened.get("page_size").expect("get").source, "user");
    }

    #[test]
    fn reset_restores_default() {
        let (_directory, mut store) = store();
        store.set("log_level", "debug").expect("set");
        let change = store.reset(Some("log_level")).expect("reset");
        assert_eq!(change.new, json!("error"));
    }

    #[test]
    fn secret_key_is_rejected() {
        let (_directory, mut store) = store();
        assert!(store.set("cookie", "secret").is_err());
    }

    #[test]
    fn unsupported_operating_mode_is_rejected() {
        let (_directory, mut store) = store();
        assert!(store.set("operating_mode", "research").is_err());
    }
}
