use crate::{config, distributed::DistributedTelemetryConfig};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::time::{Duration, SystemTime};

#[derive(Deserialize)]
struct DaemonHealth {
    state: String,
    #[serde(default)]
    netlink_state: String,
    #[serde(default)]
    dropped_events: u64,
    #[serde(default)]
    dropped_telemetry: u64,
    #[serde(default)]
    exporter: Option<serde_json::Value>,
}

fn report(ok: bool, label: &str, detail: impl std::fmt::Display) -> bool {
    println!("{} {label}: {detail}", if ok { "✅" } else { "⚠️" });
    ok
}

fn check_storage() -> bool {
    let dir = config::get_syspilot_dir();
    if let Err(error) = fs::create_dir_all(&dir) {
        return report(false, "Storage", format!("{}: {error}", dir.display()));
    }
    let probe = dir.join(format!(".doctor-{}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(probe);
            report(true, "Storage", format!("{} is writable", dir.display()))
        }
        Err(error) => report(false, "Storage", format!("{}: {error}", dir.display())),
    }
}

fn check_ai(cfg: &config::Config) {
    let (configured, detail) = match cfg.active_provider.as_str() {
        "ollama" => (
            true,
            format!("optional Ollama provider at {}", cfg.ollama_url),
        ),
        "gemini" => (
            !cfg.gemini_api_key.is_empty(),
            format!("optional Gemini provider, model {}", cfg.gemini_model),
        ),
        "syspilot" => (
            !cfg.syspilot_api_key.is_empty(),
            format!("optional SysPilot provider, model {}", cfg.syspilot_model),
        ),
        provider => (false, format!("unknown provider {provider}")),
    };
    let suffix = if configured {
        "configured"
    } else {
        "credentials not configured; offline diagnostics remain available"
    };
    let _ = report(configured, "AI", format!("{detail}; {suffix}"));
}

fn check_export(cfg: &DistributedTelemetryConfig) -> bool {
    if !cfg.enabled {
        return report(true, "Telemetry export", "disabled (default)");
    }
    report(
        cfg.validate().is_ok(),
        "Telemetry export",
        format!(
            "enabled for {} as node {} (connectivity not tested)",
            cfg.endpoint, cfg.node_id
        ),
    )
}

fn check_daemon() -> bool {
    let path = config::daemon_health_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return report(
                false,
                "Daemon",
                format!("no health report at {}: {error}", path.display()),
            )
        }
    };
    let health: DaemonHealth = match serde_json::from_str(&text) {
        Ok(health) => health,
        Err(error) => return report(false, "Daemon", format!("malformed health report: {error}")),
    };
    let age = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    let fresh = age.is_some_and(|value| value <= Duration::from_secs(3));
    let netlink = if health.netlink_state.is_empty() {
        "unknown"
    } else {
        &health.netlink_state
    };
    let dropped_telemetry = health
        .exporter
        .as_ref()
        .and_then(|exporter| exporter["dropped"].as_u64())
        .unwrap_or(health.dropped_telemetry);
    report(
        fresh && health.state == "ready",
        "Daemon",
        format!(
            "state={}, netlink={}, dropped_events={}, dropped_telemetry={}, heartbeat_age={}ms",
            health.state,
            netlink,
            health.dropped_events,
            dropped_telemetry,
            age.map_or(u128::MAX, |value| value.as_millis())
        ),
    )
}

pub fn run() -> bool {
    println!(
        "SysPilot doctor {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("SYSPILOT_BUILD_COMMIT")
    );
    let mut healthy = report(cfg!(target_os = "linux"), "Platform", std::env::consts::OS);
    healthy &= report(
        std::path::Path::new("/proc").is_dir(),
        "procfs",
        "/proc is available",
    );
    healthy &= check_storage();
    match config::load_checked() {
        Ok(cfg) => {
            println!("✅ Configuration: valid");
            check_ai(&cfg);
            healthy &= check_export(&cfg.distributed_telemetry);
        }
        Err(error) => healthy &= report(false, "Configuration", error),
    }
    healthy &= check_daemon();
    healthy
}
