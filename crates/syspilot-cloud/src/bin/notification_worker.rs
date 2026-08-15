#![forbid(unsafe_code)]

use reqwest::{Client, Url};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, PgPoolOptions};
use std::{env, time::Duration};
use syspilot_cloud::db as sqlx;
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
        let endpoint = required("SYSPILOT_AWS_NOTIFICATION_ENDPOINT")?
            .parse::<Url>()
            .map_err(|error| format!("SYSPILOT_AWS_NOTIFICATION_ENDPOINT is invalid: {error}"))?;
        let loopback = endpoint
            .host_str()
            .is_some_and(|host| host == "localhost" || host == "127.0.0.1");
        if endpoint.scheme() != "https" && !loopback {
            return Err("AWS notification endpoint must use HTTPS".into());
        }
        let bearer_token = required("SYSPILOT_AWS_NOTIFICATION_TOKEN")?;
        if bearer_token.len() < 32 {
            return Err("AWS notification credential is too short".into());
        }
        let poll_ms = env::var("NOTIFICATION_POLL_MS")
            .unwrap_or_else(|_| "250".into())
            .parse::<u64>()
            .map_err(|error| format!("NOTIFICATION_POLL_MS is invalid: {error}"))?;
        if !(50..=60_000).contains(&poll_ms) {
            return Err("NOTIFICATION_POLL_MS is outside the supported range".into());
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

struct Delivery {
    tenant_id: Uuid,
    delivery_id: i64,
    alert_instance_id: String,
    channel: String,
    destination_ref: String,
    payload: Value,
}

#[derive(Serialize)]
struct NotificationRequest<'a> {
    schema_version: u16,
    alert_instance_id: &'a str,
    channel: &'a str,
    destination: &'a str,
    payload: &'a Value,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();
    if let Err(error) = run().await {
        tracing::error!(error = %error, "notification worker stopped");
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
        .build()
        .map_err(|error| format!("could not create notification client: {error}"))?;
    loop {
        match lease(&pool).await {
            Ok(Some(delivery)) => process(&pool, &client, &config, delivery).await,
            Ok(None) => tokio::time::sleep(config.poll_interval).await,
            Err(error) => {
                tracing::error!(error = %error, "notification lease failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn lease(pool: &PgPool) -> Result<Option<Delivery>, sqlx::Error> {
    sqlx::query_as::<_, (Uuid, i64, String, String, String, Value)>(
        "SELECT tenant_id,delivery_id,alert_instance_id,channel,destination_ref,payload
         FROM syspilot_control.lease_notification_delivery()",
    )
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.map(
            |(tenant_id, delivery_id, alert_instance_id, channel, destination_ref, payload)| {
                Delivery {
                    tenant_id,
                    delivery_id,
                    alert_instance_id,
                    channel,
                    destination_ref,
                    payload,
                }
            },
        )
    })
}

async fn process(pool: &PgPool, client: &Client, config: &WorkerConfig, delivery: Delivery) {
    let request = NotificationRequest {
        schema_version: 1,
        alert_instance_id: &delivery.alert_instance_id,
        channel: &delivery.channel,
        destination: &delivery.destination_ref,
        payload: &delivery.payload,
    };
    let outcome = client
        .post(config.endpoint.clone())
        .bearer_auth(&config.bearer_token)
        .json(&request)
        .send()
        .await;
    let result = match outcome {
        Ok(response) if response.status().is_success() => complete(pool, &delivery).await,
        Ok(response) if response.status().as_u16() == 429 => {
            fail(pool, &delivery, "rate_limited").await
        }
        Ok(response) if response.status().is_server_error() => {
            fail(pool, &delivery, "provider_unavailable").await
        }
        Ok(_) => fail(pool, &delivery, "provider_rejected").await,
        Err(error) if error.is_timeout() => fail(pool, &delivery, "provider_timeout").await,
        Err(_) => fail(pool, &delivery, "transport_failure").await,
    };
    if let Err(error) = result {
        tracing::error!(delivery_id = delivery.delivery_id, error = %error, "notification state update failed");
    }
}

async fn complete(pool: &PgPool, delivery: &Delivery) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT syspilot_control.complete_notification_delivery($1,$2)")
        .bind(delivery.tenant_id)
        .bind(delivery.delivery_id)
        .fetch_one(pool)
        .await
        .map(|_| ())
}

async fn fail(pool: &PgPool, delivery: &Delivery, code: &str) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT syspilot_control.fail_notification_delivery($1,$2,$3)")
        .bind(delivery.tenant_id)
        .bind(delivery.delivery_id)
        .bind(code)
        .fetch_one(pool)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_request_contains_no_tenant_or_worker_credential() {
        let payload = serde_json::json!({"state":"firing"});
        let request = NotificationRequest {
            schema_version: 1,
            alert_instance_id: "alert-a",
            channel: "email",
            destination: "ops@example.test",
            payload: &payload,
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(!encoded.contains("tenant"));
        assert!(!encoded.contains("token"));
    }
}
