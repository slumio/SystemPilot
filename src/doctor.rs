use crate::output::{CapabilityState, DiagnosticV1, Outcome, OutputEnvelopeV1};
use crate::{config, distributed::DistributedTelemetryConfig};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorDataV1 {
    pub version: String,
    pub build_commit: String,
}

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

fn diagnostic(
    component: &str,
    state: CapabilityState,
    detail: impl Into<String>,
    impact: impl Into<String>,
    recovery: Option<&str>,
) -> DiagnosticV1 {
    DiagnosticV1 {
        component: component.into(),
        state,
        detail: detail.into(),
        impact: impact.into(),
        recovery_command: recovery.map(str::to_owned),
        fallback_used: None,
    }
}

fn storage_check() -> DiagnosticV1 {
    let dir = config::get_syspilot_dir();
    if let Err(error) = fs::create_dir_all(&dir) {
        return diagnostic(
            "storage",
            CapabilityState::Unavailable,
            error.to_string(),
            "configuration, cases, spool, and support artifacts cannot be written",
            Some("check the SysPilot home directory ownership and free space"),
        );
    }
    let probe = dir.join(format!(".doctor-{}", std::process::id()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            match fs::remove_file(&probe) {
                Ok(()) => diagnostic(
                    "storage",
                    CapabilityState::Available,
                    format!("{} is writable", dir.display()),
                    "none",
                    None,
                ),
                Err(error) => diagnostic(
                    "storage",
                    CapabilityState::Degraded,
                    format!("write succeeded but probe cleanup failed: {error}"),
                    "temporary probe remains on disk",
                    Some("remove the reported .doctor-* file after checking ownership"),
                ),
            }
        }
        Err(error) => diagnostic(
            "storage",
            CapabilityState::Unavailable,
            error.to_string(),
            "persistent local operation is unavailable",
            Some("check the SysPilot home directory ownership and free space"),
        ),
    }
}

fn ai_check(cfg: &config::Config) -> DiagnosticV1 {
    let (configured, detail) = match cfg.active_provider.as_str() {
        "disabled" => (
            true,
            "AI explicitly disabled; offline diagnostics remain active".into(),
        ),
        "ollama" => (
            true,
            format!("optional Ollama provider at {}", cfg.ollama_url),
        ),
        "gemini" => (
            !cfg.gemini_api_key.is_empty(),
            format!(
                "optional Gemini provider, model {}; credential source={} available={}",
                cfg.gemini_model,
                crate::credentials::CredentialResolver::status(&cfg.gemini_credential).source,
                crate::credentials::CredentialResolver::status(&cfg.gemini_credential).available
            ),
        ),
        "syspilot" => (
            !cfg.syspilot_api_key.is_empty(),
            format!(
                "optional SysPilot provider, model {}; credential source={} available={}",
                cfg.syspilot_model,
                crate::credentials::CredentialResolver::status(&cfg.syspilot_credential).source,
                crate::credentials::CredentialResolver::status(&cfg.syspilot_credential).available
            ),
        ),
        provider => (false, format!("unknown provider {provider}")),
    };
    if configured {
        diagnostic("ai", CapabilityState::Available, detail, "none", None)
    } else {
        diagnostic(
            "ai",
            CapabilityState::Degraded,
            detail,
            "AI explanations are unavailable; offline diagnostics remain available",
            Some("syspilot config set-key <provider> <key>"),
        )
    }
}

fn export_check(cfg: &DistributedTelemetryConfig) -> DiagnosticV1 {
    if !cfg.enabled {
        return diagnostic(
            "telemetry_export",
            CapabilityState::Available,
            "disabled by explicit default",
            "fleet delivery is inactive; local diagnostics remain available",
            Some("syspilot config telemetry enable <endpoint> <node-id> [token]"),
        );
    }
    match cfg.validate() {
        Ok(()) => diagnostic(
            "telemetry_export",
            CapabilityState::Available,
            format!(
                "enabled for {} as node {}; connectivity not tested",
                cfg.endpoint, cfg.node_id
            ),
            "none",
            None,
        ),
        Err(error) => diagnostic(
            "telemetry_export",
            CapabilityState::Misconfigured,
            error.to_string(),
            "telemetry cannot be delivered",
            Some("syspilot config telemetry show"),
        ),
    }
}

fn daemon_check() -> DiagnosticV1 {
    let path = config::daemon_health_path();
    let text =
        match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => return diagnostic(
                "daemon",
                CapabilityState::Unavailable,
                error.to_string(),
                "live lifecycle events are unavailable; direct procfs diagnostics remain available",
                Some("syspilot daemon"),
            ),
        };
    let health: DaemonHealth = match serde_json::from_str(&text) {
        Ok(health) => health,
        Err(error) => {
            return diagnostic(
                "daemon",
                CapabilityState::Misconfigured,
                format!("malformed health report: {error}"),
                "daemon state cannot be trusted",
                Some("restart the SysPilot daemon"),
            )
        }
    };
    let age = match fs::metadata(&path).and_then(|metadata| metadata.modified()) {
        Ok(modified) => match SystemTime::now().duration_since(modified) {
            Ok(age) => age,
            Err(error) => {
                return diagnostic(
                    "daemon",
                    CapabilityState::Degraded,
                    format!("heartbeat timestamp is in the future: {error}"),
                    "freshness cannot be established",
                    Some("synchronize the host clock and restart the daemon"),
                )
            }
        },
        Err(error) => {
            return diagnostic(
                "daemon",
                CapabilityState::Degraded,
                format!("health metadata unavailable: {error}"),
                "freshness cannot be established",
                Some("check runtime directory permissions"),
            )
        }
    };
    let dropped = health
        .exporter
        .as_ref()
        .and_then(|value| value["dropped"].as_u64())
        .unwrap_or(health.dropped_telemetry);
    let detail = format!(
        "state={}, netlink={}, dropped_events={}, dropped_telemetry={}, heartbeat_age={}ms",
        health.state,
        if health.netlink_state.is_empty() {
            "unknown"
        } else {
            &health.netlink_state
        },
        health.dropped_events,
        dropped,
        age.as_millis()
    );
    if age <= Duration::from_secs(3) && health.state == "ready" {
        diagnostic("daemon", CapabilityState::Available, detail, "none", None)
    } else {
        diagnostic(
            "daemon",
            CapabilityState::Degraded,
            detail,
            "live lifecycle evidence may be stale or incomplete",
            Some("syspilot status"),
        )
    }
}

pub fn collect() -> crate::error::AppResult<OutputEnvelopeV1<DoctorDataV1>> {
    let mut diagnostics = vec![
        diagnostic(
            "platform",
            if cfg!(target_os = "linux") {
                CapabilityState::Available
            } else {
                CapabilityState::Unavailable
            },
            std::env::consts::OS,
            "Linux diagnostics require Linux",
            None,
        ),
        diagnostic(
            "procfs",
            if std::path::Path::new("/proc").is_dir() {
                CapabilityState::Available
            } else {
                CapabilityState::Unavailable
            },
            "/proc capability probe",
            "process and system evidence is unavailable",
            Some("mount procfs at /proc"),
        ),
        storage_check(),
    ];
    match config::load_checked() {
        Ok(cfg) => {
            diagnostics.push(diagnostic(
                "configuration",
                CapabilityState::Available,
                "schema and values are valid",
                "none",
                None,
            ));
            diagnostics.push(ai_check(&cfg));
            diagnostics.push(export_check(&cfg.distributed_telemetry));
        }
        Err(error) => diagnostics.push(diagnostic(
            "configuration",
            CapabilityState::Misconfigured,
            error.to_string(),
            "commands requiring configuration cannot start",
            Some("syspilot config rollback"),
        )),
    }
    let legacy_backup = config::get_syspilot_dir().join("config.pre-v2.json");
    if legacy_backup.exists() {
        use std::os::unix::fs::PermissionsExt;
        let secure = std::fs::metadata(&legacy_backup)
            .map(|metadata| metadata.permissions().mode() & 0o077 == 0)
            .unwrap_or(false);
        diagnostics.push(diagnostic(
            "legacy_secret_backup",
            if secure {
                CapabilityState::Degraded
            } else {
                CapabilityState::Misconfigured
            },
            format!(
                "{} exists; it may contain legacy serialized credentials; owner_only={secure}",
                legacy_backup.display()
            ),
            "rollback evidence must be protected and may contain secrets",
            Some("after the rollback window, archive securely or remove config.pre-v2.json"),
        ));
    }
    diagnostics.push(daemon_check());
    let outcome = if diagnostics.iter().any(|item| {
        matches!(item.state, CapabilityState::Misconfigured)
            || matches!(item.component.as_str(), "platform" | "procfs" | "storage")
                && item.state == CapabilityState::Unavailable
    }) {
        Outcome::Error
    } else if diagnostics
        .iter()
        .any(|item| item.state != CapabilityState::Available)
    {
        Outcome::Degraded
    } else {
        Outcome::Ok
    };
    OutputEnvelopeV1::new(
        "doctor",
        outcome,
        DoctorDataV1 {
            version: env!("CARGO_PKG_VERSION").into(),
            build_commit: env!("SYSPILOT_BUILD_COMMIT").into(),
        },
        diagnostics,
    )
}

pub fn render_human(report: &OutputEnvelopeV1<DoctorDataV1>) {
    println!(
        "SysPilot doctor {} ({})",
        report.data.version, report.data.build_commit
    );
    for item in &report.diagnostics {
        let marker = match item.state {
            CapabilityState::Available => "✅",
            CapabilityState::Degraded => "⚠️",
            CapabilityState::Unavailable | CapabilityState::Misconfigured => "❌",
        };
        println!("{marker} {}: {}", item.component, item.detail);
        if item.impact != "none" {
            println!("   Impact: {}", item.impact);
        }
        if let Some(command) = &item.recovery_command {
            println!("   Recovery: {command}");
        }
    }
}

pub fn run() -> bool {
    match collect() {
        Ok(report) => {
            render_human(&report);
            report.outcome == Outcome::Ok
        }
        Err(error) => {
            eprintln!("❌ Doctor failed: {error}");
            false
        }
    }
}
