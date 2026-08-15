//! Shared, validated setup workflow for interactive terminal configuration.

use crate::{config, fleet, install};
use serde::Serialize;
use std::io::{self, IsTerminal, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceMode {
    Auto,
    Tui,
    Line,
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeploymentMode {
    LocalOnly,
    SelfHostedCollector,
    HostedFleet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AiMode {
    KeepCurrent,
    Gemini,
    Ollama,
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalCapability {
    pub stdin_is_terminal: bool,
    pub stdout_is_terminal: bool,
    pub term_configured: bool,
    pub tui_available: bool,
    pub impact: &'static str,
    pub recovery_command: &'static str,
}

pub fn terminal_capability() -> TerminalCapability {
    let stdin_is_terminal = io::stdin().is_terminal();
    let stdout_is_terminal = io::stdout().is_terminal();
    let term_configured = std::env::var_os("TERM").is_some_and(|v| !v.is_empty() && v != "dumb");
    let tui_available = stdin_is_terminal && stdout_is_terminal && term_configured;
    TerminalCapability {
        stdin_is_terminal,
        stdout_is_terminal,
        term_configured,
        tui_available,
        impact: if tui_available {
            "none"
        } else {
            "full-screen setup is unavailable; no configuration was changed"
        },
        recovery_command: "syspilot setup --line",
    }
}

pub fn run(mode: InterfaceMode) -> bool {
    let capability = terminal_capability();
    if mode == InterfaceMode::Check {
        match serde_json::to_string_pretty(&capability) {
            Ok(value) => println!("{value}"),
            Err(error) => {
                eprintln!("❌ Could not encode terminal capability: {error}");
                return false;
            }
        }
        return capability.tui_available;
    }
    let use_tui = match mode {
        InterfaceMode::Tui if !capability.tui_available => {
            eprintln!(
                "❌ Full-screen setup is unavailable: {}. Recovery: `{}`.",
                capability.impact, capability.recovery_command
            );
            return false;
        }
        InterfaceMode::Tui => true,
        InterfaceMode::Line => false,
        InterfaceMode::Auto if capability.tui_available => true,
        InterfaceMode::Auto => {
            eprintln!(
                "⚠️  Full-screen setup is unavailable: {}.",
                capability.impact
            );
            eprintln!("Using the explicit line interface equivalent; run `{}` directly to suppress this notice.", capability.recovery_command);
            false
        }
        InterfaceMode::Check => unreachable!(),
    };
    match wizard(use_tui) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("❌ Setup failed: {error}");
            false
        }
    }
}

fn wizard(full_screen: bool) -> Result<(), String> {
    let config_path = config::get_syspilot_dir().join("config.json");
    let original_config = match std::fs::read(&config_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "could not preserve configuration for rollback: {error}"
            ))
        }
    };
    let mut cfg =
        config::load_checked().map_err(|e| format!("could not load configuration: {e}"))?;
    screen(full_screen, "Deployment mode");
    let deployment = match choose(
        "Where should SysPilot operate?",
        &[
            "Local only (offline, no export)",
            "Self-hosted collector",
            "Hosted fleet",
        ],
        1,
    )? {
        1 => DeploymentMode::LocalOnly,
        2 => DeploymentMode::SelfHostedCollector,
        _ => DeploymentMode::HostedFleet,
    };

    let mut endpoint = String::new();
    let mut node_id = String::new();
    let mut credential = String::new();
    let mut credential_variable = None;
    if deployment != DeploymentMode::LocalOnly {
        endpoint = prompt("Collector HTTPS endpoint")?;
        node_id = prompt("Node ID")?;
        let variable = if deployment == DeploymentMode::HostedFleet {
            "SYSPILOT_FLEET_TOKEN"
        } else {
            "SYSPILOT_TELEMETRY_TOKEN"
        };
        credential_variable = Some(variable.to_string());
        credential = std::env::var(variable).map_err(|_| {
            format!("{variable} is required; credentials are never echoed by the setup UI")
        })?;
        if credential.trim().is_empty() {
            return Err(format!("{variable} must not be empty"));
        }
    }

    screen(full_screen, "Optional AI explanation");
    let ai = match choose(
        "AI is optional and never controls alerts or remediation.",
        &[
            "Keep current provider",
            "Gemini",
            "Ollama",
            "Disable for now",
        ],
        1,
    )? {
        1 => AiMode::KeepCurrent,
        2 => AiMode::Gemini,
        3 => AiMode::Ollama,
        _ => AiMode::Disabled,
    };
    match ai {
        AiMode::Gemini => {
            cfg.active_provider = "gemini".into();
            if let Ok(key) = std::env::var("GEMINI_API_KEY") {
                if !key.is_empty() {
                    cfg.gemini_api_key = key;
                    cfg.gemini_credential = crate::credentials::CredentialRef::Environment {
                        variable: "GEMINI_API_KEY".into(),
                    };
                }
            }
        }
        AiMode::Ollama => {
            cfg.active_provider = "ollama".into();
            let value = prompt_default("Ollama URL", &cfg.ollama_url)?;
            cfg.ollama_url = value;
        }
        AiMode::Disabled => {
            cfg.active_provider = "disabled".into();
            cfg.gemini_api_key.clear();
            cfg.syspilot_api_key.clear();
        }
        AiMode::KeepCurrent => {}
    }

    match deployment {
        DeploymentMode::LocalOnly => {
            fleet::disable(&mut cfg);
            cfg.distributed_telemetry.enabled = false;
        }
        DeploymentMode::SelfHostedCollector => {
            fleet::disable(&mut cfg);
            cfg.distributed_telemetry.enabled = true;
            cfg.distributed_telemetry.endpoint = endpoint.clone();
            cfg.distributed_telemetry.node_id = node_id.clone();
            cfg.distributed_telemetry.bearer_token = credential;
            cfg.distributed_telemetry.bearer_credential =
                crate::credentials::CredentialRef::Environment {
                    variable: credential_variable
                        .clone()
                        .ok_or_else(|| "telemetry credential source is missing".to_string())?,
                };
        }
        DeploymentMode::HostedFleet => fleet::enroll_with_reference(
            &mut cfg,
            endpoint.clone(),
            node_id.clone(),
            credential,
            crate::credentials::CredentialRef::Environment {
                variable: credential_variable
                    .clone()
                    .ok_or_else(|| "fleet credential source is missing".to_string())?,
            },
        )
        .map_err(|e| format!("fleet enrollment is invalid: {e}"))?,
    }
    config::validate(&cfg).map_err(|e| format!("configuration is invalid: {e}"))?;

    if deployment != DeploymentMode::LocalOnly {
        screen(full_screen, "Telemetry preview");
        let preview = crate::distributed::preview_envelope(
            &cfg.distributed_telemetry,
            crate::distributed::TelemetryKind::SystemSnapshot,
            &crate::telemetry::collect_system_telemetry(),
        )
        .map_err(|e| format!("could not create telemetry preview: {e}"))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&preview)
                .map_err(|e| format!("could not render telemetry preview: {e}"))?
        );
        if !confirm("Continue to final review after inspecting this export preview?")? {
            println!("Setup cancelled; no configuration was changed.");
            return Ok(());
        }
    }

    screen(full_screen, "Review and apply");
    println!("Deployment: {:?}", deployment);
    println!("AI choice: {:?}", ai);
    println!(
        "Endpoint: {}",
        if endpoint.is_empty() {
            "disabled"
        } else {
            endpoint.as_str()
        }
    );
    println!(
        "Node ID: {}",
        if node_id.is_empty() {
            "not configured"
        } else {
            node_id.as_str()
        }
    );
    println!(
        "Credential: {}",
        if cfg.distributed_telemetry.bearer_token.is_empty() {
            "not configured"
        } else {
            "[configured]"
        }
    );
    println!("Telemetry remains local until this confirmation.");
    if !confirm("Apply this configuration?")? {
        println!("Setup cancelled; no configuration was changed.");
        return Ok(());
    }

    if !install::install() {
        return Err(
            "installation files could not be prepared; configuration was not applied".into(),
        );
    }
    println!("Applied step 1/3: installation files prepared");
    config::save(&cfg).map_err(|e| format!("could not atomically save configuration: {e}"))?;
    println!("Applied step 2/3: configuration committed");
    if !install::install_user_binary(false) {
        let rollback = match original_config {
            Some(bytes) => crate::config_migration::atomic_write(&config_path, &bytes),
            None => std::fs::remove_file(&config_path)
                .map_err(|e| crate::error::AppError::io("remove newly-created configuration", e)),
        };
        if let Err(error) = rollback {
            return Err(format!(
                "binary installation failed and configuration rollback also failed: {error}"
            ));
        }
        return Err("configuration was saved, but the user binary installation failed; run `syspilot install --binary`".into());
    }
    println!("Applied step 3/3: user binary installed");
    println!("✅ Setup applied. Run `syspilot doctor` to review capability health.");
    Ok(())
}

fn screen(full_screen: bool, title: &str) {
    if full_screen {
        use ratatui::{
            backend::CrosstermBackend,
            style::{Color, Modifier, Style},
            widgets::{Block, Borders, Paragraph},
            Terminal,
        };
        let backend = CrosstermBackend::new(io::stdout());
        match Terminal::new(backend).and_then(|mut terminal| {
            terminal.clear()?;
            terminal.draw(|frame| {
                frame.render_widget(
                    Paragraph::new(format!("SysPilot setup — {title}"))
                        .style(
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        )
                        .block(Block::default().borders(Borders::ALL)),
                    ratatui::layout::Rect::new(0, 0, frame.area().width, 3),
                );
            })?;
            Ok(())
        }) {
            Ok(()) => {
                if let Err(error) =
                    crossterm::execute!(io::stdout(), crossterm::cursor::MoveTo(0, 3))
                {
                    eprintln!("terminal cursor update failed: {error}");
                }
            }
            Err(error) => eprintln!("terminal screen update failed: {error}"),
        }
    } else {
        println!("SysPilot setup — {title}\n{}", "─".repeat(50));
    }
}

fn choose(prompt_text: &str, options: &[&str], default: usize) -> Result<usize, String> {
    println!("{prompt_text}");
    for (index, option) in options.iter().enumerate() {
        println!("  {}. {option}", index + 1);
    }
    loop {
        let value = prompt(&format!("Choice [{default}]"))?;
        if value.is_empty() {
            return Ok(default);
        }
        if let Ok(choice) = value.parse::<usize>() {
            if (1..=options.len()).contains(&choice) {
                return Ok(choice);
            }
        }
        eprintln!("Invalid choice. Enter 1 through {}.", options.len());
    }
}

fn prompt(label: &str) -> Result<String, String> {
    print!("{label}: ");
    io::stdout()
        .flush()
        .map_err(|e| format!("could not display prompt: {e}"))?;
    let mut value = String::new();
    let bytes = io::stdin()
        .read_line(&mut value)
        .map_err(|e| format!("could not read terminal input: {e}"))?;
    if bytes == 0 {
        return Err(
            "terminal input closed before setup completed; no pending configuration was applied"
                .into(),
        );
    }
    Ok(value.trim().to_string())
}

fn prompt_default(label: &str, default: &str) -> Result<String, String> {
    let value = prompt(&format!("{label} [{default}]"))?;
    Ok(if value.is_empty() {
        default.to_string()
    } else {
        value
    })
}

fn confirm(label: &str) -> Result<bool, String> {
    loop {
        match prompt(&format!("{label} [y/N]"))?
            .to_ascii_lowercase()
            .as_str()
        {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => eprintln!("Enter y or n."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_contract_has_actionable_fallback() {
        let capability = terminal_capability();
        assert_eq!(capability.recovery_command, "syspilot setup --line");
        assert_eq!(
            capability.tui_available,
            capability.stdin_is_terminal
                && capability.stdout_is_terminal
                && capability.term_configured
        );
    }
}
