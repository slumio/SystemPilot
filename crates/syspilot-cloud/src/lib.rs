#![forbid(unsafe_code)]

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use syspilot::distributed::{
    IngestionAcknowledgement, ProcessAlert, RejectedRecord, TelemetryEnvelope, TelemetryKind,
    TELEMETRY_SCHEMA_VERSION,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

#[doc(hidden)]
pub mod db {
    pub use sqlx_core::query::query;
    pub use sqlx_core::query_as::query_as;
    pub use sqlx_core::query_scalar::query_scalar;
    pub use sqlx_core::transaction::Transaction;
    pub use sqlx_core::Error;
    pub use sqlx_postgres::{PgPool, PgPoolOptions, Postgres};
}

use crate::db as sqlx;
use sqlx::{PgPool, PgPoolOptions, Postgres, Transaction};

const MAX_BATCH: usize = 256;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const RETRY_AFTER_SECONDS: &str = "1";
const NANOS_PER_SECOND: u64 = 1_000_000_000;
const MIN_REASONABLE_UNIX_SECONDS: u64 = 946_684_800; // 2000-01-01T00:00:00Z
const MAX_REASONABLE_UNIX_SECONDS: u64 = 4_102_444_799; // 2099-12-31T23:59:59Z

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnixTimestamp {
    seconds: i64,
    nanoseconds: i64,
}

impl UnixTimestamp {
    fn from_nanos(value: u64) -> Result<Self, ApiError> {
        let seconds = value / NANOS_PER_SECOND;
        if !(MIN_REASONABLE_UNIX_SECONDS..=MAX_REASONABLE_UNIX_SECONDS).contains(&seconds) {
            return Err(ApiError::Invalid(
                "telemetry timestamp must be between 2000-01-01 and 2099-12-31 UTC".into(),
            ));
        }
        Ok(Self {
            seconds: i64::try_from(seconds)
                .map_err(|_| ApiError::Invalid("telemetry timestamp overflows storage".into()))?,
            nanoseconds: i64::try_from(value % NANOS_PER_SECOND)
                .expect("subsecond nanoseconds always fit in i64"),
        })
    }

    #[cfg(test)]
    fn as_nanos(self) -> u64 {
        u64::try_from(self.seconds).expect("validated seconds are nonnegative") * NANOS_PER_SECOND
            + u64::try_from(self.nanoseconds).expect("validated nanoseconds are nonnegative")
    }
}

#[derive(Clone)]
pub struct CloudConfig {
    pub listen_addr: SocketAddr,
    pub database_url: String,
    credential_pepper: Vec<u8>,
    pub database_connections: u32,
    pub concurrent_requests: usize,
}

impl std::fmt::Debug for CloudConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CloudConfig")
            .field("listen_addr", &self.listen_addr)
            .field("database_url", &"[REDACTED]")
            .field("credential_pepper", &"[REDACTED]")
            .field("database_connections", &self.database_connections)
            .field("concurrent_requests", &self.concurrent_requests)
            .finish()
    }
}

impl CloudConfig {
    pub fn from_env() -> Result<Self, String> {
        let database_url = required_env("DATABASE_URL")?;
        let credential_pepper = required_env("SYSPILOT_CREDENTIAL_PEPPER")?.into_bytes();
        if credential_pepper.len() < 32 {
            return Err("SYSPILOT_CREDENTIAL_PEPPER must contain at least 32 bytes".into());
        }
        let listen_addr = env::var("LISTEN_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".into())
            .parse()
            .map_err(|error| format!("LISTEN_ADDR is invalid: {error}"))?;
        let database_connections = parse_env("DATABASE_CONNECTIONS", 32, 1..=256)?;
        let concurrent_requests = parse_env("CONCURRENT_REQUESTS", 512, 1..=16_384)?;
        Ok(Self {
            listen_addr,
            database_url,
            credential_pepper,
            database_connections,
            concurrent_requests,
        })
    }

    fn token_digest(&self, token: &str) -> [u8; 32] {
        let mut authenticator = Hmac::<Sha256>::new_from_slice(&self.credential_pepper)
            .expect("HMAC accepts keys of every size");
        authenticator.update(token.as_bytes());
        authenticator.finalize().into_bytes().into()
    }
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn parse_env<T>(name: &str, default: T, range: std::ops::RangeInclusive<T>) -> Result<T, String>
where
    T: Copy + PartialOrd + std::str::FromStr + std::fmt::Display,
    T::Err: std::fmt::Display,
{
    let value = match env::var(name) {
        Ok(raw) => raw
            .parse::<T>()
            .map_err(|error| format!("{name} is invalid: {error}"))?,
        Err(_) => default,
    };
    if !range.contains(&value) {
        return Err(format!("{name} is outside the supported range"));
    }
    Ok(value)
}

#[derive(Default)]
struct Metrics {
    requests: AtomicU64,
    accepted: AtomicU64,
    rejected: AtomicU64,
    unauthorized: AtomicU64,
    saturated: AtomicU64,
    database_errors: AtomicU64,
}

#[derive(Serialize)]
struct MetricsSnapshot {
    requests: u64,
    accepted_envelopes: u64,
    rejected_envelopes: u64,
    unauthorized_requests: u64,
    saturated_requests: u64,
    database_errors: u64,
}

impl Metrics {
    fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            accepted_envelopes: self.accepted.load(Ordering::Relaxed),
            rejected_envelopes: self.rejected.load(Ordering::Relaxed),
            unauthorized_requests: self.unauthorized.load(Ordering::Relaxed),
            saturated_requests: self.saturated.load(Ordering::Relaxed),
            database_errors: self.database_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone)]
struct AppState {
    config: Arc<CloudConfig>,
    pool: PgPool,
    admission: Arc<Admission>,
    metrics: Arc<Metrics>,
}

struct Admission {
    global: Arc<Semaphore>,
    identities: Mutex<HashMap<(Uuid, String), Arc<Semaphore>>>,
    per_identity: usize,
    max_identities: usize,
}

struct AdmissionPermit {
    _identity: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

impl Admission {
    fn new(capacity: usize) -> Self {
        Self {
            global: Arc::new(Semaphore::new(capacity)),
            identities: Mutex::new(HashMap::new()),
            per_identity: (capacity / 4).max(1),
            max_identities: capacity.saturating_mul(4),
        }
    }

    fn try_acquire(&self, tenant_id: Uuid, node_id: &str) -> Option<AdmissionPermit> {
        let key = (tenant_id, node_id.to_owned());
        let identity = {
            let mut identities = self.identities.lock().ok()?;
            if !identities.contains_key(&key) && identities.len() >= self.max_identities {
                identities.retain(|_, permits| Arc::strong_count(permits) > 1);
            }
            if !identities.contains_key(&key) && identities.len() >= self.max_identities {
                return None;
            }
            identities
                .entry(key)
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_identity)))
                .clone()
        };
        let identity_permit = identity.try_acquire_owned().ok()?;
        let global_permit = self.global.clone().try_acquire_owned().ok()?;
        Some(AdmissionPermit {
            _identity: identity_permit,
            _global: global_permit,
        })
    }
}

#[derive(Debug)]
enum ApiError {
    Unauthorized,
    Invalid(String),
    Saturated,
    Database,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message, retry) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "node credential is missing, expired, revoked, or invalid".into(),
                false,
            ),
            Self::Invalid(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                message,
                false,
            ),
            Self::Saturated => (
                StatusCode::SERVICE_UNAVAILABLE,
                "saturated",
                "collector admission capacity is exhausted; retry with jitter".into(),
                true,
            ),
            Self::Database => (
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "telemetry was not acknowledged; retry with jitter".into(),
                true,
            ),
        };
        let mut response = (status, Json(ErrorBody { code, message })).into_response();
        if retry {
            response
                .headers_mut()
                .insert("retry-after", HeaderValue::from_static(RETRY_AFTER_SECONDS));
        }
        response
    }
}

pub async fn build(config: CloudConfig) -> Result<Router, String> {
    let pool = PgPoolOptions::new()
        .max_connections(config.database_connections)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&config.database_url)
        .await
        .map_err(|error| format!("could not connect to PostgreSQL: {error}"))?;
    let state = AppState {
        admission: Arc::new(Admission::new(config.concurrent_requests)),
        config: Arc::new(config),
        pool,
        metrics: Arc::new(Metrics::default()),
    };
    Ok(Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/metrics", get(metrics))
        .route("/v1/telemetry", post(ingest))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state))
}

async fn live() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn ready(State(state): State<AppState>) -> StatusCode {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(1) => StatusCode::NO_CONTENT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn metrics(State(state): State<AppState>) -> Json<MetricsSnapshot> {
    Json(state.metrics.snapshot())
}

async fn ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(batch): Json<Vec<TelemetryEnvelope>>,
) -> Result<Json<IngestionAcknowledgement>, ApiError> {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    validate_batch(&batch)?;
    let token = bearer_token(&headers).ok_or_else(|| {
        state.metrics.unauthorized.fetch_add(1, Ordering::Relaxed);
        ApiError::Unauthorized
    })?;
    let digest = state.config.token_digest(token);
    let mut transaction = state.pool.begin().await.map_err(|error| {
        tracing::error!(error = %error, "could not start ingestion transaction");
        state
            .metrics
            .database_errors
            .fetch_add(1, Ordering::Relaxed);
        ApiError::Database
    })?;
    let identity = authenticate(&mut transaction, &digest)
        .await
        .map_err(|error| {
            tracing::warn!(error = %error, "credential lookup failed");
            state
                .metrics
                .database_errors
                .fetch_add(1, Ordering::Relaxed);
            ApiError::Database
        })?;
    let Some((tenant_id, authenticated_node)) = identity else {
        state.metrics.unauthorized.fetch_add(1, Ordering::Relaxed);
        return Err(ApiError::Unauthorized);
    };
    if batch
        .iter()
        .any(|record| record.node_id != authenticated_node)
    {
        return Err(ApiError::Invalid(
            "every envelope node_id must match the authenticated node".into(),
        ));
    }
    let _permit = state
        .admission
        .try_acquire(tenant_id, &authenticated_node)
        .ok_or_else(|| {
            state.metrics.saturated.fetch_add(1, Ordering::Relaxed);
            ApiError::Saturated
        })?;
    set_tenant(&mut transaction, tenant_id).await?;

    let mut accepted_message_ids = Vec::with_capacity(batch.len());
    let mut rejected_records = Vec::new();
    let mut highest_accepted_sequence = None;
    for record in &batch {
        match insert_envelope(&mut transaction, tenant_id, record).await? {
            InsertOutcome::Accepted => {
                accepted_message_ids.push(record.message_id.clone());
                highest_accepted_sequence =
                    Some(highest_accepted_sequence.unwrap_or(0).max(record.sequence));
            }
            InsertOutcome::SequenceConflict => rejected_records.push(RejectedRecord {
                message_id: record.message_id.clone(),
                reason: "sequence_conflict".into(),
            }),
            InsertOutcome::ReplayConflict => rejected_records.push(RejectedRecord {
                message_id: record.message_id.clone(),
                reason: "message_id_content_conflict".into(),
            }),
        }
    }
    record_batch_state(
        &mut transaction,
        tenant_id,
        &authenticated_node,
        highest_accepted_sequence,
    )
    .await?;
    transaction.commit().await.map_err(|error| {
        tracing::error!(error = %error, "ingestion commit failed");
        state
            .metrics
            .database_errors
            .fetch_add(1, Ordering::Relaxed);
        ApiError::Database
    })?;

    state
        .metrics
        .accepted
        .fetch_add(accepted_message_ids.len() as u64, Ordering::Relaxed);
    state
        .metrics
        .rejected
        .fetch_add(rejected_records.len() as u64, Ordering::Relaxed);
    Ok(Json(IngestionAcknowledgement {
        accepted_message_ids,
        highest_accepted_sequence,
        rejected_records,
        retry_after_ms: None,
    }))
}

fn validate_batch(batch: &[TelemetryEnvelope]) -> Result<(), ApiError> {
    if batch.is_empty() || batch.len() > MAX_BATCH {
        return Err(ApiError::Invalid(format!(
            "batch must contain between 1 and {MAX_BATCH} envelopes"
        )));
    }
    for record in batch {
        record
            .validate()
            .map_err(|error| ApiError::Invalid(error.to_string()))?;
        if record.schema_version != TELEMETRY_SCHEMA_VERSION || record.sequence == 0 {
            return Err(ApiError::Invalid(
                "schema_version must be 1 and sequence must be greater than zero".into(),
            ));
        }
        UnixTimestamp::from_nanos(record.observed_at_unix_nanos)?;
    }
    Ok(())
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| token.len() >= 32 && token.len() <= 1024)
}

async fn authenticate(
    transaction: &mut Transaction<'_, Postgres>,
    digest: &[u8],
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as::<_, (Uuid, String)>(
        "SELECT tenant_id, node_id FROM syspilot_control.authenticate_node($1)",
    )
    .bind(digest)
    .fetch_optional(&mut **transaction)
    .await
}

async fn set_tenant(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT set_config('syspilot.tenant_id', $1, true)")
        .bind(tenant_id.to_string())
        .execute(&mut **transaction)
        .await
        .map(|_| ())
        .map_err(|error| {
            tracing::error!(error = %error, "could not set transaction tenant");
            ApiError::Database
        })
}

enum InsertOutcome {
    Accepted,
    SequenceConflict,
    ReplayConflict,
}

fn envelope_digest(record: &TelemetryEnvelope) -> Result<[u8; 32], ApiError> {
    let canonical = serde_json::to_vec(record)
        .map_err(|_| ApiError::Invalid("envelope could not be encoded".into()))?;
    Ok(Sha256::digest(canonical).into())
}

async fn insert_envelope(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    record: &TelemetryEnvelope,
) -> Result<InsertOutcome, ApiError> {
    let sequence = i64::try_from(record.sequence)
        .map_err(|_| ApiError::Invalid("sequence exceeds PostgreSQL bigint".into()))?;
    let observed = UnixTimestamp::from_nanos(record.observed_at_unix_nanos)?;
    let envelope = serde_json::to_value(record)
        .map_err(|_| ApiError::Invalid("envelope could not be encoded".into()))?;
    let digest = envelope_digest(record)?;
    let kind = serde_json::to_value(&record.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| ApiError::Invalid("telemetry kind could not be encoded".into()))?;
    let inserted = sqlx::query_scalar::<_, bool>(
        "INSERT INTO syspilot_control.telemetry_messages
         (tenant_id,node_id,message_id,sequence,schema_version,kind,observed_at,envelope,envelope_digest)
         VALUES ($1,$2,$3,$4,$5,$6,
           TIMESTAMPTZ 'epoch' + $7 * INTERVAL '1 second' + $8 * INTERVAL '1 nanosecond',$9,$10)
         ON CONFLICT DO NOTHING RETURNING true",
    )
    .bind(tenant_id)
    .bind(&record.node_id)
    .bind(&record.message_id)
    .bind(sequence)
    .bind(i32::from(record.schema_version))
    .bind(kind)
    .bind(observed.seconds)
    .bind(observed.nanoseconds)
    .bind(&envelope)
    .bind(digest.as_slice())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    if inserted == Some(true) {
        if record.kind == TelemetryKind::ProcessAlert {
            materialize_alert(transaction, tenant_id, record).await?;
        }
        if record.kind != TelemetryKind::Health {
            sqlx::query(
                "INSERT INTO syspilot_control.reasoning_jobs(tenant_id,node_id,message_id)
                 VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
            )
            .bind(tenant_id)
            .bind(&record.node_id)
            .bind(&record.message_id)
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
        }
        return Ok(InsertOutcome::Accepted);
    }
    let replay = sqlx::query_as::<_, (Option<Vec<u8>>, serde_json::Value)>(
        "SELECT envelope_digest,envelope FROM syspilot_control.telemetry_messages
         WHERE tenant_id=$1 AND node_id=$2 AND message_id=$3",
    )
    .bind(tenant_id)
    .bind(&record.node_id)
    .bind(&record.message_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    match replay {
        Some((Some(stored), _)) if stored.as_slice() == digest.as_slice() => {
            Ok(InsertOutcome::Accepted)
        }
        Some((None, stored_envelope)) if stored_envelope == envelope => {
            sqlx::query(
                "UPDATE syspilot_control.telemetry_messages SET envelope_digest=$4
                 WHERE tenant_id=$1 AND node_id=$2 AND message_id=$3 AND envelope_digest IS NULL",
            )
            .bind(tenant_id)
            .bind(&record.node_id)
            .bind(&record.message_id)
            .bind(digest.as_slice())
            .execute(&mut **transaction)
            .await
            .map_err(database_error)?;
            Ok(InsertOutcome::Accepted)
        }
        Some(_) => Ok(InsertOutcome::ReplayConflict),
        None => Ok(InsertOutcome::SequenceConflict),
    }
}

async fn materialize_alert(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    record: &TelemetryEnvelope,
) -> Result<(), ApiError> {
    let alert: ProcessAlert = serde_json::from_value(record.payload.clone())
        .map_err(|_| ApiError::Invalid("process_alert payload does not match schema v1".into()))?;
    if alert.instance_id.trim().is_empty()
        || alert.rule_id.trim().is_empty()
        || !matches!(
            alert.state.as_str(),
            "firing" | "acknowledged" | "resolved" | "suppressed"
        )
    {
        return Err(ApiError::Invalid(
            "process_alert identity or state is invalid".into(),
        ));
    }
    let observed = UnixTimestamp::from_nanos(alert.observed_at_unix_nanos)?;
    sqlx::query(
        "INSERT INTO syspilot_control.alerts
         (tenant_id,alert_instance_id,node_id,rule_id,state,payload,first_observed_at,last_transition_at)
         VALUES ($1,$2,$3,$4,$5,$6,
           TIMESTAMPTZ 'epoch' + $7 * INTERVAL '1 second' + $8 * INTERVAL '1 nanosecond',
           TIMESTAMPTZ 'epoch' + $7 * INTERVAL '1 second' + $8 * INTERVAL '1 nanosecond')
         ON CONFLICT (tenant_id,alert_instance_id) DO UPDATE SET
           state=EXCLUDED.state,payload=EXCLUDED.payload,last_transition_at=EXCLUDED.last_transition_at",
    )
    .bind(tenant_id)
    .bind(&alert.instance_id)
    .bind(&record.node_id)
    .bind(&alert.rule_id)
    .bind(&alert.state)
    .bind(&record.payload)
    .bind(observed.seconds)
    .bind(observed.nanoseconds)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if alert.state == "firing" {
        sqlx::query(
            "INSERT INTO syspilot_control.notification_deliveries
             (tenant_id,alert_instance_id,channel,destination_ref)
             SELECT $1,$2,channel,destination_ref
             FROM syspilot_control.alert_destinations WHERE tenant_id=$1 AND enabled",
        )
        .bind(tenant_id)
        .bind(&alert.instance_id)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

async fn record_batch_state(
    transaction: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    node_id: &str,
    highest: Option<u64>,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE syspilot_control.nodes SET last_seen_at=clock_timestamp()
         WHERE tenant_id=$1 AND node_id=$2",
    )
    .bind(tenant_id)
    .bind(node_id)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    sqlx::query(
        "INSERT INTO syspilot_control.active_server_days(tenant_id,usage_day,node_id)
         VALUES ($1,CURRENT_DATE,$2)
         ON CONFLICT (tenant_id,usage_day,node_id)
         DO UPDATE SET last_seen_at=clock_timestamp()",
    )
    .bind(tenant_id)
    .bind(node_id)
    .execute(&mut **transaction)
    .await
    .map_err(database_error)?;
    if let Some(highest) = highest {
        let highest = i64::try_from(highest)
            .map_err(|_| ApiError::Invalid("sequence exceeds PostgreSQL bigint".into()))?;
        sqlx::query(
            "INSERT INTO syspilot_control.node_sequence_state
             (tenant_id,node_id,highest_accepted_sequence) VALUES ($1,$2,$3)
             ON CONFLICT (tenant_id,node_id) DO UPDATE SET
             highest_accepted_sequence=GREATEST(
               syspilot_control.node_sequence_state.highest_accepted_sequence,
               EXCLUDED.highest_accepted_sequence), updated_at=clock_timestamp()",
        )
        .bind(tenant_id)
        .bind(node_id)
        .bind(highest)
        .execute(&mut **transaction)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

fn database_error(error: sqlx::Error) -> ApiError {
    tracing::error!(error = %error, "ingestion database operation failed");
    ApiError::Database
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::collections::BTreeMap;
    use syspilot::distributed::TelemetryKind;

    fn envelope(sequence: u64) -> TelemetryEnvelope {
        TelemetryEnvelope {
            schema_version: 1,
            message_id: format!("message-{sequence}"),
            node_id: "node-a".into(),
            sequence,
            observed_at_unix_nanos: MIN_REASONABLE_UNIX_SECONDS * NANOS_PER_SECOND,
            kind: TelemetryKind::Health,
            payload: serde_json::json!({"healthy": true}),
            attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn batch_contract_is_bounded_and_requires_sequences() {
        assert!(validate_batch(&[]).is_err());
        assert!(validate_batch(&vec![envelope(1); MAX_BATCH + 1]).is_err());
        assert!(validate_batch(&[envelope(0)]).is_err());
        assert!(validate_batch(&[envelope(1)]).is_ok());
    }

    #[test]
    fn timestamps_preserve_integer_nanosecond_parts() {
        for value in [
            MIN_REASONABLE_UNIX_SECONDS * NANOS_PER_SECOND,
            1_725_000_000_123_456_789,
            MAX_REASONABLE_UNIX_SECONDS * NANOS_PER_SECOND + 999_999_999,
        ] {
            let timestamp = UnixTimestamp::from_nanos(value).unwrap();
            assert_eq!(timestamp.as_nanos(), value);
            assert!((0..1_000_000_000).contains(&timestamp.nanoseconds));
        }
    }

    #[test]
    fn timestamps_reject_zero_overflow_and_unreasonable_dates() {
        assert!(UnixTimestamp::from_nanos(0).is_err());
        assert!(
            UnixTimestamp::from_nanos(MIN_REASONABLE_UNIX_SECONDS * NANOS_PER_SECOND - 1).is_err()
        );
        assert!(
            UnixTimestamp::from_nanos((MAX_REASONABLE_UNIX_SECONDS + 1) * NANOS_PER_SECOND)
                .is_err()
        );
        assert!(UnixTimestamp::from_nanos(u64::MAX).is_err());
    }

    #[test]
    fn envelope_digest_binds_replays_to_exact_content() {
        let original = envelope(1);
        let exact_replay = original.clone();
        let mut changed = original.clone();
        changed.payload = serde_json::json!({"healthy": false});
        assert_eq!(
            envelope_digest(&original).unwrap(),
            envelope_digest(&exact_replay).unwrap()
        );
        assert_ne!(
            envelope_digest(&original).unwrap(),
            envelope_digest(&changed).unwrap()
        );
    }

    #[test]
    fn admission_limits_one_identity_without_starving_another() {
        let admission = Admission::new(8);
        let tenant = Uuid::from_u128(1);
        let node_a: Vec<_> = (0..2)
            .map(|_| admission.try_acquire(tenant, "node-a").unwrap())
            .collect();
        assert!(admission.try_acquire(tenant, "node-a").is_none());
        let node_b = admission.try_acquire(tenant, "node-b");
        assert!(node_b.is_some());
        drop(node_a);
        assert!(admission.try_acquire(tenant, "node-a").is_some());
    }

    #[test]
    fn admission_is_tenant_scoped_and_recovers_capacity() {
        let admission = Admission::new(4);
        let tenant_a = Uuid::from_u128(1);
        let tenant_b = Uuid::from_u128(2);
        let held = admission.try_acquire(tenant_a, "shared-node").unwrap();
        assert!(admission.try_acquire(tenant_a, "shared-node").is_none());
        assert!(admission.try_acquire(tenant_b, "shared-node").is_some());
        drop(held);
        assert!(admission.try_acquire(tenant_a, "shared-node").is_some());
    }

    #[test]
    fn bearer_parser_rejects_short_and_non_bearer_values() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer_token(&headers), None);
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer short"));
        assert_eq!(bearer_token(&headers), None);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer 01234567890123456789012345678901"),
        );
        assert!(bearer_token(&headers).is_some());
    }

    #[test]
    fn config_debug_never_reveals_database_or_pepper() {
        let config = CloudConfig {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            database_url: "postgres://secret".into(),
            credential_pepper: b"01234567890123456789012345678901".to_vec(),
            database_connections: 2,
            concurrent_requests: 4,
        };
        let output = format!("{config:?}");
        assert!(!output.contains("postgres://secret"));
        assert!(!output.contains("0123456789"));
    }

    #[test]
    fn token_digest_is_stable_and_keyed() {
        let base = CloudConfig {
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            database_url: "unused".into(),
            credential_pepper: vec![1; 32],
            database_connections: 1,
            concurrent_requests: 1,
        };
        let mut other = base.clone();
        other.credential_pepper = vec![2; 32];
        assert_eq!(base.token_digest("token"), base.token_digest("token"));
        assert_ne!(base.token_digest("token"), other.token_digest("token"));
    }
}
