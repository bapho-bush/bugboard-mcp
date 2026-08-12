use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};

use serde_json::json;

use crate::errors::ToolFailure;
#[derive(Clone)]
pub(crate) struct SessionConfig {
    pub(crate) cookie: String,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("cookie", &"<redacted>")
            .finish()
    }
}

impl SessionConfig {
    pub(crate) fn from_env() -> Result<Self, ConfigError> {
        let env_file = std::env::var_os("BUGBOARD_SESSION_ENV")
            .map(PathBuf::from)
            .ok_or(ConfigError::MissingEnvFilePath)?;
        let values = parse_env_file(&fs::read_to_string(&env_file).map_err(|source| {
            ConfigError::ReadEnvFile {
                path: env_file.clone(),
                source: Arc::new(source),
            }
        })?);
        Self::from_values(&values)
    }

    pub(crate) fn from_values(values: &HashMap<String, String>) -> Result<Self, ConfigError> {
        let cookie = values
            .get("BUGBOARD_COOKIE")
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or(ConfigError::MissingCookie)?
            .to_owned();

        Ok(Self { cookie })
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ConfigError {
    MissingEnvFilePath,
    MissingCookie,
    ReadEnvFile {
        path: PathBuf,
        source: Arc<std::io::Error>,
    },
}

impl From<ConfigError> for ToolFailure {
    fn from(error: ConfigError) -> Self {
        match error {
            ConfigError::MissingEnvFilePath => ToolFailure::new(
                "config_missing",
                "Set BUGBOARD_SESSION_ENV to an env-file outside the repository.",
                json!({"required_env": "BUGBOARD_SESSION_ENV"}),
            ),
            ConfigError::MissingCookie => ToolFailure::new(
                "config_missing",
                "Session env-file must contain BUGBOARD_COOKIE.",
                json!({"required_key": "BUGBOARD_COOKIE"}),
            ),
            ConfigError::ReadEnvFile { path, source } => ToolFailure::new(
                "config_error",
                "Could not read BUGBOARD_SESSION_ENV file.",
                json!({
                    "path": path.to_string_lossy(),
                    "source": source.to_string(),
                }),
            ),
        }
    }
}

pub(crate) fn parse_env_file(contents: &str) -> HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            Some((
                key.trim().to_owned(),
                unquote_env_value(value.trim()).to_owned(),
            ))
        })
        .collect()
}

fn unquote_env_value(value: &str) -> &str {
    if value.len() >= 2 {
        let quoted = (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''));
        if quoted {
            return &value[1..value.len() - 1];
        }
    }
    value
}
