use ingot_domain::ids::{ItemId, ItemRevisionId};
use ingot_domain::ports::{ConflictKind, RepositoryError};
use serde::de::DeserializeOwned;
use sqlx::sqlite::{SqliteQueryResult, SqliteRow};
use sqlx::{Sqlite, Transaction};

#[derive(Debug, thiserror::Error)]
pub(super) enum StoreDecodeError {
    #[error("invalid json value: {0}")]
    Json(String),
}

pub(super) fn parse_json<T>(value: String) -> Result<T, RepositoryError>
where
    T: DeserializeOwned,
{
    serde_json::from_str(&value).map_err(|err| {
        RepositoryError::Database(Box::new(StoreDecodeError::Json(format!("{value}: {err}"))))
    })
}

pub(super) fn serialize_optional_json(
    value: Option<&serde_json::Value>,
) -> Result<Option<String>, RepositoryError> {
    value
        .map(serde_json::to_string)
        .transpose()
        .map_err(json_err)
}

pub(super) fn db_err<E>(err: E) -> RepositoryError
where
    E: std::error::Error + Send + Sync + 'static,
{
    RepositoryError::Database(Box::new(err))
}

pub(super) fn db_write_err(err: sqlx::Error) -> RepositoryError {
    match err {
        sqlx::Error::Database(database_error)
            if database_error.is_unique_violation()
                || database_error.is_foreign_key_violation() =>
        {
            RepositoryError::Conflict(ConflictKind::DatabaseConstraint(
                database_error.message().to_string(),
            ))
        }
        other => db_err(other),
    }
}

pub(super) fn map_optional_row<T>(
    row: Option<SqliteRow>,
    map: impl FnOnce(&SqliteRow) -> Result<T, RepositoryError>,
) -> Result<Option<T>, RepositoryError> {
    row.as_ref().map(map).transpose()
}

pub(super) fn required<T>(value: Option<T>) -> Result<T, RepositoryError> {
    value.ok_or(RepositoryError::NotFound)
}

pub(super) fn required_row<T>(
    row: Option<SqliteRow>,
    map: impl FnOnce(&SqliteRow) -> Result<T, RepositoryError>,
) -> Result<T, RepositoryError> {
    required(map_optional_row(row, map)?)
}

pub(super) fn ensure_rows_affected(result: SqliteQueryResult) -> Result<(), RepositoryError> {
    if result.rows_affected() == 0 {
        return Err(RepositoryError::NotFound);
    }

    Ok(())
}

pub(super) fn json_err(err: serde_json::Error) -> RepositoryError {
    RepositoryError::Database(Box::new(err))
}

pub(super) async fn item_revision_is_stale(
    tx: &mut Transaction<'_, Sqlite>,
    item_id: ItemId,
    expected_item_revision_id: ItemRevisionId,
) -> Result<bool, RepositoryError> {
    let current_revision_id: Option<ItemRevisionId> =
        sqlx::query_scalar("SELECT current_revision_id FROM items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db_err)?;

    Ok(current_revision_id != Some(expected_item_revision_id))
}
