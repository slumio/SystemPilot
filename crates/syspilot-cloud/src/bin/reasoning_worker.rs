#![forbid(unsafe_code)]

use reqwest::{redirect::Policy, Client, StatusCode, Url};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, PgPoolOptions};
use std::{env, time::Duration};
use syspilot_cloud::db as sqlx;
use syspilot_cloud::DeliveryCircuit;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

struct WorkerConfig {
    database_url: String,
    endpoint: Url,
    bearer_token: String,
    poll_interval: Duration,
}

impl WorkerConfig {
    fn from_env() -> Result<Self, String> {
        let database_url = required("DATABASE_URL")?;
        let endpoint = required("SYSPILOT_AWS_REASONING_ENDPOINT")?
            .parse::<Url>()
            .map_err(|error| format!("SYSPILOT_AWS_REASONING_ENDPOINT is invalid: {error}"))?;
        let loopback = endpoint
            .host_str()
            .is_some_and(|host| host == "localhost" || host == "127.0.0.1");
        if endpoint.scheme() != "https" && !loopback {
            return Err("AWS reasoning endpoint must use HTTPS".into());
        }
        let bearer_token = required("SYSPILOT_AWS_REASONING_TOKEN")?;
        if bearer_token.len() < 32 {
            return Err("AWS reasoning credential is too short".into());
        }
        let poll_ms = env::var("REASONING_POLL_MS")
            .unwrap_or_else(|_| "250".into())
            .parse::<u64>()
            .map_err(|error| format!("REASONING_POLL_MS is invalid: {error}"))?;
        if !(50..=60_000).contains(&poll_ms) {
            return Err("REASONING_POLL_MS is outside the supported range".into());
        }
        Ok(Self {
            database_url,
            endpoint,
            bearer_token,
            poll_interval: Duration::from_millis(poll_ms),
        })
    }
}

fn required(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

#[derive(Serialize)]
struct ReasoningRequest<'a> {
    schema_version: u16,
    node_id: &'a str,
    message_id: &'a str,
    envelope: &'a Value,
}

struct Job {
    tenant_id: Uuid,
    job_id: i64,
    node_id: String,
    message_id: String,
    envelope: Value,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();
    if let Err(error) = run().await {
        tracing::error!(error = %error, "reasoning worker stopped");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = WorkerConfig::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&config.database_url)
        .await
        .map_err(|error| format!("could not connect to PostgreSQL: {error}"))?;
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(8)
        .redirect(Policy::none())
        .build()
        .map_err(|error| format!("could not create reasoning client: {error}"))?;
    let mut circuit = DeliveryCircuit::default();
    loop {
        if circuit.is_open() {
            tracing::warn!("reasoning provider circuit is open; durable jobs remain queued");
            tokio::time::sleep(config.poll_interval).await;
            continue;
        }
        match lease(&pool).await {
            Ok(Some(job)) => circuit.record(process(&pool, &client, &config, job).await),
            Ok(None) => tokio::time::sleep(config.poll_interval).await,
            Err(error) => {
                tracing::error!(error = %error, "reasoning lease failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn lease(pool: &PgPool) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as::<_, (Uuid, i64, String, String, Value)>(
        "SELECT tenant_id,job_id,node_id,message_id,envelope
         FROM syspilot_control.lease_reasoning_job()",
    )
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.map(|(tenant_id, job_id, node_id, message_id, envelope)| Job {
            tenant_id,
            job_id,
            node_id,
            message_id,
            envelope,
        })
    })
}

async fn process(pool: &PgPool, client: &Client, config: &WorkerConfig, job: Job) -> bool {
    let request = ReasoningRequest {
        schema_version: 1,
        node_id: &job.node_id,
        message_id: &job.message_id,
        envelope: &job.envelope,
    };
    let outcome = client
        .post(config.endpoint.clone())
        .bearer_auth(&config.bearer_token)
        .json(&request)
        .send()
        .await;
    let result = match outcome {
        Ok(response) => {
            let status = response.status();
            let body = if status.is_success() {
                response.json::<Value>().await.ok()
            } else {
                None
            };
            classify_response(status, body)
        }
        Err(error) if error.is_timeout() => Err("provider_timeout"),
        Err(_) => Err("transport_failure"),
    };
    let success = result.is_ok();
    let update = match result {
        Ok(result) => complete(pool, &job, result).await,
        Err(code) => fail(pool, &job, code).await,
    };
    if let Err(error) = update {
        tracing::error!(job_id = job.job_id, error = %error, "reasoning job state update failed");
        return false;
    }
    success
}

fn classify_response(status: StatusCode, body: Option<Value>) -> Result<Value, &'static str> {
    if status.is_redirection() {
        return Err("redirect_rejected");
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err("rate_limited");
    }
    if status.is_server_error() {
        return Err("provider_unavailable");
    }
    if !status.is_success() {
        return Err("provider_rejected");
    }
    body.filter(Value::is_object).ok_or("invalid_response")
}

async fn complete(pool: &PgPool, job: &Job, result: Value) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT syspilot_control.complete_reasoning_job($1,$2,$3)")
        .bind(job.tenant_id)
        .bind(job.job_id)
        .bind(result)
        .fetch_one(pool)
        .await
        .map(|_| ())
}

async fn fail(pool: &PgPool, job: &Job, code: &str) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT syspilot_control.fail_reasoning_job($1,$2,$3)")
        .bind(job.tenant_id)
        .bind(job.job_id)
        .bind(code)
        .fetch_one(pool)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_contains_no_tenant_or_credentials() {
        let envelope = serde_json::json!({"kind":"health"});
        let request = ReasoningRequest {
            schema_version: 1,
            node_id: "node-a",
            message_id: "message-a",
            envelope: &envelope,
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(!encoded.contains("tenant"));
        assert!(!encoded.contains("token"));
    }

    #[test]
    fn provider_responses_have_bounded_failure_codes() {
        assert_eq!(
            classify_response(StatusCode::TOO_MANY_REQUESTS, None),
            Err("rate_limited")
        );
        assert_eq!(
            classify_response(StatusCode::BAD_GATEWAY, None),
            Err("provider_unavailable")
        );
        assert_eq!(
            classify_response(StatusCode::FOUND, None),
            Err("redirect_rejected")
        );
        assert_eq!(
            classify_response(StatusCode::OK, Some(Value::Null)),
            Err("invalid_response")
        );
        assert!(
            classify_response(StatusCode::OK, Some(serde_json::json!({"summary":"ok"}))).is_ok()
        );
    }
}
