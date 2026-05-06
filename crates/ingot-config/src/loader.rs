use std::path::Path;

use crate::schema::{IngotConfig, RawConfig};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
}

/// Load and merge config from global defaults and project-level overrides.
pub fn load_config(
    global_path: &Path,
    project_path: Option<&Path>,
) -> Result<IngotConfig, ConfigError> {
    let mut config = IngotConfig::default();

    if global_path.exists() {
        let raw_config = read_raw_config(global_path)?;
        raw_config.merge_into(&mut config);
    }

    if let Some(project_path) = project_path {
        if project_path.exists() {
            let raw_config = read_raw_config(project_path)?;
            raw_config.merge_into(&mut config);
        }
    }

    Ok(config)
}

fn read_raw_config(path: &Path) -> Result<RawConfig, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(e.to_string()))?;
    serde_yml::from_str(&contents).map_err(|e| ConfigError::Parse(e.to_string()))
}
