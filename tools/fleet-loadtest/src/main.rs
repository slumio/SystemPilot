use axum::{extract::State, http::StatusCode, routing::get, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize, Serialize)]
struct Envelope {
    schema_version: u32,
    message_id: String,
    node_id: String,
    sequence: u64,
    observed_at_unix_nanos: u64,
    kind: String,
    payload: serde_json::Value,
    #[serde(default)]
    attributes: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct Acknowledgement {
    accepted_message_ids: Vec<String>,
    highest_accepted_sequence: Option<u64>,
    rejected_records: Vec<serde_json::Value>,
    retry_after_ms: Option<u64>,
}

#[derive(Default)]
struct ServerMetrics {
    requests: AtomicU64,
    envelopes: AtomicU64,
    invalid: AtomicU64,
}

#[derive(Serialize)]
struct ServerSnapshot {
    requests: u64,
    envelopes: u64,
    invalid: u64,
}

impl ServerMetrics {
    fn snapshot(&self) -> ServerSnapshot {
        ServerSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            envelopes: self.envelopes.load(Ordering::Relaxed),
            invalid: self.invalid.load(Ordering::Relaxed),
        }
    }
}

async fn ingest(
    State(metrics): State<Arc<ServerMetrics>>,
    Json(batch): Json<Vec<Envelope>>,
) -> Result<Json<Acknowledgement>, StatusCode> {
    if batch.is_empty()
        || batch.iter().any(|record| {
            record.schema_version != 1 || record.message_id.is_empty() || record.node_id.is_empty()
        })
    {
        metrics.invalid.fetch_add(1, Ordering::Relaxed);
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    metrics.requests.fetch_add(1, Ordering::Relaxed);
    metrics
        .envelopes
        .fetch_add(batch.len() as u64, Ordering::Relaxed);
    Ok(Json(Acknowledgement {
        accepted_message_ids: batch
            .iter()
            .map(|record| record.message_id.clone())
            .collect(),
        highest_accepted_sequence: batch.iter().map(|record| record.sequence).max(),
        rejected_records: Vec::new(),
        retry_after_ms: None,
    }))
}

async fn serve() -> Result<(), String> {
    let address = env("LISTEN_ADDR", "0.0.0.0:8080");
    let metrics = Arc::new(ServerMetrics::default());
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .route(
            "/metrics",
            get({
                let metrics = Arc::clone(&metrics);
                move || {
                    let metrics = Arc::clone(&metrics);
                    async move { Json(metrics.snapshot()) }
                }
            }),
        )
        .route("/v1/telemetry", post(ingest))
        .with_state(metrics);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|error| format!("could not bind {address}: {error}"))?;
    println!("synthetic collector listening on {address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|error| format!("collector failed: {error}"))
}

const LATENCY_BOUNDS_MS: [u64; 12] = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, u64::MAX];

struct LoadMetrics {
    succeeded: AtomicU64,
    failed: AtomicU64,
    envelopes: AtomicU64,
    latency: [AtomicU64; LATENCY_BOUNDS_MS.len()],
}

impl Default for LoadMetrics {
    fn default() -> Self {
        Self {
            succeeded: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            envelopes: AtomicU64::new(0),
            latency: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl LoadMetrics {
    fn observe(&self, elapsed: Duration) {
        let milliseconds = elapsed.as_millis().min(u64::MAX as u128) as u64;
        let index = LATENCY_BOUNDS_MS
            .iter()
            .position(|bound| milliseconds <= *bound)
            .unwrap_or(LATENCY_BOUNDS_MS.len() - 1);
        self.latency[index].fetch_add(1, Ordering::Relaxed);
    }

    fn percentile_ms(&self, percentile: f64) -> u64 {
        let total = self.succeeded.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let target = (total as f64 * percentile).ceil() as u64;
        let mut seen = 0;
        for (index, bucket) in self.latency.iter().enumerate() {
            seen += bucket.load(Ordering::Relaxed);
            if seen >= target {
                return LATENCY_BOUNDS_MS[index];
            }
        }
        u64::MAX
    }
}

#[derive(Serialize)]
struct LoadReport {
    virtual_servers: usize,
    configured_duration_seconds: u64,
    elapsed_seconds: f64,
    batch_size: usize,
    successful_requests: u64,
    failed_requests: u64,
    envelopes: u64,
    requests_per_second: f64,
    envelopes_per_second: f64,
    p50_latency_ms_upper_bound: u64,
    p95_latency_ms_upper_bound: u64,
    p99_latency_ms_upper_bound: u64,
    passed: bool,
    scope: &'static str,
}

async fn load() -> Result<(), String> {
    let endpoint = env("TARGET_URL", "http://collector:8080/v1/telemetry");
    let virtual_servers = parse_env("VIRTUAL_SERVERS", 100usize)?;
    let duration_seconds = parse_env("DURATION_SECONDS", 30u64)?;
    let batch_size = parse_env("BATCH_SIZE", 16usize)?;
    let requests_per_server_second = parse_env("REQUESTS_PER_SERVER_SECOND", 1u64)?;
    let max_failure_rate = parse_env("MAX_FAILURE_RATE", 0.001f64)?;
    let max_p95_ms = parse_env("MAX_P95_MS", 500u64)?;
    if virtual_servers == 0
        || duration_seconds == 0
        || batch_size == 0
        || requests_per_server_second == 0
    {
        return Err("load settings must be greater than zero".into());
    }

    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(virtual_servers.min(10_000))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("could not create load client: {error}"))?;
    let metrics = Arc::new(LoadMetrics::default());
    let load_started = Instant::now();
    let interval = Duration::from_secs_f64(1.0 / requests_per_server_second as f64);
    let requests_per_server = duration_seconds
        .checked_mul(requests_per_server_second)
        .ok_or_else(|| "requested load duration overflows".to_string())?;
    let mut tasks = Vec::with_capacity(virtual_servers);
    for server_index in 0..virtual_servers {
        let endpoint = endpoint.clone();
        let client = client.clone();
        let metrics = Arc::clone(&metrics);
        tasks.push(tokio::spawn(async move {
            let node_id = format!("load-node-{server_index}");
            let mut sequence = 1u64;
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Tokio intervals yield immediately once. Consume that tick so a
            // configured N-second run sends exactly N cycles at 1 Hz.
            ticker.tick().await;
            for _ in 0..requests_per_server {
                ticker.tick().await;
                let batch: Vec<_> = (0..batch_size)
                    .map(|_| {
                        let current = sequence;
                        sequence += 1;
                        Envelope {
                            schema_version: 1,
                            message_id: format!("{node_id}-{current}"),
                            node_id: node_id.clone(),
                            sequence: current,
                            observed_at_unix_nanos: now_ns(),
                            kind: "process_lifecycle".into(),
                            payload: serde_json::json!({"event_type":"EXEC","pid":current}),
                            attributes: serde_json::Map::new(),
                        }
                    })
                    .collect();
                let started = Instant::now();
                let result = client.post(&endpoint).json(&batch).send().await;
                match result {
                    Ok(response) if response.status().is_success() => {
                        metrics.succeeded.fetch_add(1, Ordering::Relaxed);
                        metrics
                            .envelopes
                            .fetch_add(batch.len() as u64, Ordering::Relaxed);
                        metrics.observe(started.elapsed());
                    }
                    Ok(_) | Err(_) => {
                        metrics.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    for task in tasks {
        task.await
            .map_err(|error| format!("load task failed: {error}"))?;
    }
    let elapsed_seconds = load_started.elapsed().as_secs_f64();

    let succeeded = metrics.succeeded.load(Ordering::Relaxed);
    let failed = metrics.failed.load(Ordering::Relaxed);
    let envelopes = metrics.envelopes.load(Ordering::Relaxed);
    let total = succeeded + failed;
    let failure_rate = if total == 0 {
        1.0
    } else {
        failed as f64 / total as f64
    };
    let p95 = metrics.percentile_ms(0.95);
    let passed = failure_rate <= max_failure_rate && p95 <= max_p95_ms;
    let report = LoadReport {
        virtual_servers,
        configured_duration_seconds: duration_seconds,
        elapsed_seconds,
        batch_size,
        successful_requests: succeeded,
        failed_requests: failed,
        envelopes,
        requests_per_second: succeeded as f64 / elapsed_seconds,
        envelopes_per_second: envelopes as f64 / elapsed_seconds,
        p50_latency_ms_upper_bound: metrics.percentile_ms(0.50),
        p95_latency_ms_upper_bound: p95,
        p99_latency_ms_upper_bound: metrics.percentile_ms(0.99),
        passed,
        scope: "synthetic stateless HTTP acknowledgement; excludes authentication, PostgreSQL, retention, and replay storms",
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    if passed {
        Ok(())
    } else {
        Err("benchmark thresholds were not met".into())
    }
}

async fn health() -> Result<(), String> {
    let url = env("HEALTH_URL", "http://127.0.0.1:8080/health");
    let response = reqwest::get(&url)
        .await
        .map_err(|error| format!("health request failed: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("health request returned {}", response.status()))
    }
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse_env<T>(name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr + Copy,
    T::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("invalid {name}: {error}")),
        Err(_) => Ok(default),
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[tokio::main]
async fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("server") => serve().await,
        Some("load") => load().await,
        Some("health") => health().await,
        _ => Err("usage: syspilot-fleet-loadtest <server|load|health>".into()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
