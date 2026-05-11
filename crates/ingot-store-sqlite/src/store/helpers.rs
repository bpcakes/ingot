use chrono::{DateTime, Utc};
use ingot_domain::activity::{ActivityEntityType, ActivityEventType};
use ingot_domain::agent::{AdapterKind, AgentProvider, AgentStatus};
use ingot_domain::agent_model::AgentModel;
use ingot_domain::branch_name::BranchName;
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::{CheckoutAdoptionState, ConvergenceStatus, ConvergenceStrategy};
use ingot_domain::convergence_queue::ConvergenceQueueEntryStatus;
use ingot_domain::finding::{
    EstimatedScope, FindingSeverity, FindingSubjectKind, FindingTriageState,
};
use ingot_domain::git_operation::{GitEntityType, GitOperationStatus, OperationKind};
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{
    ActivityId, AgentId, ConvergenceId, ConvergenceQueueEntryId, FindingId, GitOperationId, ItemId,
    ItemRevisionId, JobId, ProjectId, WorkspaceId,
};
use ingot_domain::item::{
    ApprovalState, Classification, DoneReason, EscalationReason, ParkingState, Priority,
    ResolutionSource, WorkflowVersion,
};
use ingot_domain::job::{
    ContextPolicy, ExecutionPermission, JobStatus, OutcomeClass, OutputArtifactKind, PhaseKind,
};
use ingot_domain::lease_owner_id::LeaseOwnerId;
use ingot_domain::ports::{ConflictKind, RepositoryError};
use ingot_domain::project::ExecutionMode;
use ingot_domain::revision::ApprovalPolicy;
use ingot_domain::step_id::StepId;
use ingot_domain::workspace::{RetentionPolicy, WorkspaceKind, WorkspaceStatus, WorkspaceStrategy};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::sqlite::{SqliteQueryResult, SqliteRow};
use sqlx::{Encode, Row, Sqlite, Transaction, Type};

#[derive(Debug, thiserror::Error)]
pub(super) enum StoreDecodeError {
    #[error("invalid json value: {0}")]
    Json(String),
    #[error("invalid database text value: {0}")]
    Text(String),
}

pub(super) fn parse_json<T>(value: String) -> Result<T, RepositoryError>
where
    T: DeserializeOwned,
{
    serde_json::from_str(&value).map_err(|err| {
        RepositoryError::Database(Box::new(StoreDecodeError::Json(format!("{value}: {err}"))))
    })
}

pub(super) trait StoreRowValue: Sized {
    fn get(row: &SqliteRow, column: &'static str) -> Result<Self, RepositoryError>;
}

pub(super) fn row_get<T>(row: &SqliteRow, column: &'static str) -> Result<T, RepositoryError>
where
    T: StoreRowValue,
{
    T::get(row, column)
}

macro_rules! impl_sqlx_row_value {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl StoreRowValue for $ty {
                fn get(row: &SqliteRow, column: &'static str) -> Result<Self, RepositoryError> {
                    row.try_get(column).map_err(db_err)
                }
            }
        )+
    };
}

impl_sqlx_row_value!(
    String,
    Option<String>,
    i64,
    Option<i64>,
    bool,
    Option<bool>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

pub(super) struct DbText<T>(T);

pub(super) fn db_text<T>(value: T) -> DbText<T> {
    DbText(value)
}

pub(super) fn optional_db_text<T>(value: Option<T>) -> Option<DbText<T>> {
    value.map(DbText)
}

impl<T> Type<Sqlite> for DbText<T> {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <String as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
        <String as Type<Sqlite>>::compatible(ty)
    }
}

impl<'q, T> Encode<'q, Sqlite> for DbText<T>
where
    T: Serialize,
{
    fn encode(
        self,
        buf: &mut <Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as Encode<Sqlite>>::encode(serialize_db_text(&self.0)?, buf)
    }

    fn encode_by_ref(
        &self,
        buf: &mut <Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as Encode<Sqlite>>::encode(serialize_db_text(&self.0)?, buf)
    }

    fn size_hint(&self) -> usize {
        0
    }
}

fn serialize_db_text<T>(value: &T) -> Result<String, serde_json::Error>
where
    T: Serialize + ?Sized,
{
    match serde_json::to_value(value)? {
        serde_json::Value::String(value) => Ok(value),
        other => Ok(other.to_string()),
    }
}

fn parse_db_text<T>(value: String) -> Result<T, RepositoryError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(serde_json::Value::String(value.clone())).map_err(|err| {
        RepositoryError::Database(Box::new(StoreDecodeError::Text(format!("{value}: {err}"))))
    })
}

fn row_get_db_text<T>(row: &SqliteRow, column: &'static str) -> Result<T, RepositoryError>
where
    T: DeserializeOwned,
{
    parse_db_text(row.try_get(column).map_err(db_err)?)
}

fn row_get_optional_db_text<T>(
    row: &SqliteRow,
    column: &'static str,
) -> Result<Option<T>, RepositoryError>
where
    T: DeserializeOwned,
{
    row.try_get::<Option<String>, _>(column)
        .map_err(db_err)?
        .map(parse_db_text)
        .transpose()
}

macro_rules! impl_db_text_row_value {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl StoreRowValue for $ty {
                fn get(row: &SqliteRow, column: &'static str) -> Result<Self, RepositoryError> {
                    row_get_db_text(row, column)
                }
            }

            impl StoreRowValue for Option<$ty> {
                fn get(row: &SqliteRow, column: &'static str) -> Result<Self, RepositoryError> {
                    row_get_optional_db_text(row, column)
                }
            }
        )+
    };
}

impl_db_text_row_value!(
    ActivityEntityType,
    ActivityEventType,
    ActivityId,
    AdapterKind,
    AgentId,
    AgentModel,
    AgentProvider,
    AgentStatus,
    ApprovalPolicy,
    ApprovalState,
    BranchName,
    CheckoutAdoptionState,
    Classification,
    CommitOid,
    ContextPolicy,
    ConvergenceId,
    ConvergenceQueueEntryId,
    ConvergenceQueueEntryStatus,
    ConvergenceStatus,
    ConvergenceStrategy,
    DoneReason,
    EscalationReason,
    EstimatedScope,
    ExecutionPermission,
    ExecutionMode,
    FindingId,
    FindingSeverity,
    FindingSubjectKind,
    FindingTriageState,
    GitEntityType,
    GitOperationId,
    GitOperationStatus,
    GitRef,
    ItemId,
    ItemRevisionId,
    JobId,
    JobStatus,
    LeaseOwnerId,
    OperationKind,
    OutcomeClass,
    OutputArtifactKind,
    ParkingState,
    PhaseKind,
    Priority,
    ProjectId,
    ResolutionSource,
    StepId,
    RetentionPolicy,
    WorkflowVersion,
    WorkspaceId,
    WorkspaceKind,
    WorkspaceStatus,
    WorkspaceStrategy,
);

pub(super) fn row_get_json<T>(row: &SqliteRow, column: &'static str) -> Result<T, RepositoryError>
where
    T: DeserializeOwned,
{
    let value = row_get::<String>(row, column)?;
    parse_json(value)
}

pub(super) fn row_get_optional_json<T>(
    row: &SqliteRow,
    column: &'static str,
) -> Result<Option<T>, RepositoryError>
where
    T: DeserializeOwned,
{
    let value = row_get::<Option<String>>(row, column)?;
    value.map(parse_json).transpose()
}

pub(super) fn serialize_json<T>(value: &T) -> Result<String, RepositoryError>
where
    T: Serialize + ?Sized,
{
    serde_json::to_string(value).map_err(json_err)
}

pub(super) fn serialize_optional_json<T>(
    value: Option<&T>,
) -> Result<Option<String>, RepositoryError>
where
    T: Serialize + ?Sized,
{
    value.map(serialize_json).transpose()
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
    let row = sqlx::query("SELECT current_revision_id FROM items WHERE id = ?")
        .bind(db_text(item_id))
        .fetch_optional(&mut **tx)
        .await
        .map_err(db_err)?;
    let current_revision_id = row
        .as_ref()
        .map(|row| row_get(row, "current_revision_id"))
        .transpose()?;

    Ok(current_revision_id != Some(expected_item_revision_id))
}
