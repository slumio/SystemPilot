use syspilot_cloud::{build, CloudConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();
    if let Err(error) = serve().await {
        tracing::error!(error = %error, "cloud collector stopped");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), String> {
    let config = CloudConfig::from_env()?;
    let address = config.listen_addr;
    let app = build(config).await?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("could not bind {address}: {error}"))?;
    tracing::info!(%address, "cloud collector ready");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if tokio::signal::ctrl_c().await.is_err() {
                tracing::warn!("shutdown signal listener failed");
            }
        })
        .await
        .map_err(|error| format!("cloud collector failed: {error}"))
}
