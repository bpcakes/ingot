use std::path::PathBuf;

use ingot_usecases::{UseCaseError, UseCaseInfraError};

use crate::error::ApiError;

pub(crate) async fn read_optional_text(path: PathBuf) -> Result<Option<String>, ApiError> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(infra_to_api(UseCaseInfraError::io(error))),
    }
}

pub(crate) async fn read_optional_json(
    path: PathBuf,
) -> Result<Option<serde_json::Value>, ApiError> {
    let Some(contents) = read_optional_text(path).await? else {
        return Ok(None);
    };

    serde_json::from_str(&contents)
        .map(Some)
        .map_err(serialization_to_api)
}

pub(crate) async fn read_optional_json_lines<T>(path: PathBuf) -> Result<Option<Vec<T>>, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    let Some(contents) = read_optional_text(path).await? else {
        return Ok(None);
    };

    let mut rows = Vec::new();
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let row = serde_json::from_str::<T>(line).map_err(serialization_to_api)?;
        rows.push(row);
    }

    Ok(Some(rows))
}

fn serialization_to_api(error: serde_json::Error) -> ApiError {
    infra_to_api(UseCaseInfraError::serialization(error))
}

fn infra_to_api(error: UseCaseInfraError) -> ApiError {
    ApiError::from(UseCaseError::from(error))
}
