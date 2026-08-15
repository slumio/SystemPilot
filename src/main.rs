// main.rs — binary entry point.
// The global mimalloc allocator is declared in lib.rs.

use syspilot::{
    ai, alert, causal_engine, codebase, completions, config, daemon, daemon_client, distributed,
    doctor, evidence, fleet, install, output, profiler, setup, support, telemetry, ui, utils,
};

use causal_engine::CausalGraph;
use clap::Parser;

use std::fs;
use std::io::BufRead;
use ui::streamer::MdStreamer;

// ── Log entry ─────────────────────────────────────────────────────────────────

struct LogEntry {
    _timestamp: String,
    _directory: String,
    command: String,
    exit_code: i32,
}

fn read_recent_entries(count: usize) -> Vec<LogEntry> {
    let path = config::get_syspilot_dir().join("context.log");
    let f = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let lines: Vec<String> = std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.is_empty())
        .collect();

    let mut entries = Vec::new();
    for line in lines.iter().rev() {
        let parts: Vec<&str> = line.splitn(4, " | ").collect();
        if parts.len() == 4 {
            let cmd = parts[2].trim().to_string();
            if cmd.starts_with("syspilot") {
                continue;
            }
            entries.push(LogEntry {
                _timestamp: parts[0].to_string(),
                _directory: parts[1].to_string(),
                command: cmd,
                exit_code: parts[3].trim().parse().unwrap_or(0),
            });
            if entries.len() >= count {
                break;
            }
        }
    }
    entries
}

fn tail_session(max_lines: usize) -> String {
    let path = config::get_syspilot_dir().join("session.log");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = if lines.len() > max_lines {
        lines.len() - max_lines
    } else {
        0
    };
    lines[start..].join("\n")
}

fn print_help() {
    println!(
        "🤖 SysPilot: Operating System Reasoning Agent (Rust Edition)\n\n\
        Usage: syspilot <command> [options]\n\n\
        Commands:\n\
          setup [--tui|--line|--check]  Guided terminal configuration
          install                       Create local configuration and shell hook
          install --binary [--force]    Copy this binary to ~/.local/bin
          uninstall --binary            Remove only the user-local binary\n\
          uninstall                     Remove terminal hooks\n\
          status                        Check integration status\n\
          doctor                        Diagnose capabilities, configuration, storage, and daemon health\n\
          evidence [--pid <pid/name>]   Capture and store a deterministic offline evidence bundle\n\
          cases list                    List retained diagnostic cases\n\
          cases show <id>               Display a diagnostic case\n\
          cases export <id> [path]      Export a case as JSON\n\
          cases delete <id>             Delete a diagnostic case\n\
          alerts list                   List persistent alert lifecycle state\n\
          alerts acknowledge <id>       Acknowledge a firing alert\n\
          alerts resolve <id>            Resolve an alert explicitly\n\
          alerts suppress <id>           Suppress an alert\n\
          support bundle create [path]   Create an inspectable redacted local support bundle\n\
          config telemetry preview [pid/name]  Preview the redacted envelope that would leave this host\n\
          completions <bash|zsh|fish>    Print a shell completion script\n\
          fleet enroll <https-endpoint> <node-id> [token]  Explicitly enroll in hosted fleet ingestion\n\
          fleet status                  Show enrollment, policy, scope, and acknowledgement state\n\
          fleet disable                 Disable hosted upload and clear its credential\n\
          daemon                        Start the background SysPilot netlink daemon\n\
          events                        Show recent daemon lifecycle events and exit reasons\n\
          monitor                       Open the real-time diagnostic TUI\n\
          provider <name>               Set active AI provider (gemini, ollama, syspilot)\n\
          model <name>                  Set active model name\n\
          pull <model> [--set-active]   Pull a model using Ollama\n\
          index [--force]               Index current codebase for vector search\n\
          config telemetry enable <endpoint> <node-id> [token]  Configure distributed export\n\
          config alert add <id> <exact|prefix> <process-name>  Add process alert\n\
          config <action>               Manage settings\n\
             set-key <provider> <key>   Set provider API key\n\
             set-url <provider> <url>   Set provider API endpoint URL\n\
             set <option> <value>       Set option (chunk_strategy, embedding_model)\n\
             rollback                   Restore the immutable pre-migration configuration backup\n\
          ask \"<question>\" [options]    Ask general tech or codebase questions\n\
             --file <path>              Provide file content context\n\
             --no-index                 Skip codebase vector search\n\
          explain [options]             Analyze system telemetry and execution\n\
             --pid <pid/name>           Target active process telemetry & profiling\n\
             --deep                     Include perf CPU profiler / system telemetry\n\
             --ebpf                     Enable real-time eBPF event tracing (requires root)\n\
             --causal                   Run real-time causal graph reasoning (CausalTrace)\n\
             --number <N>               Use N-th last failed command (default: 1)\n\
             --no-index                 Skip codebase vector search\n\n\
        Examples:\n\
          syspilot explain --pid my_server --causal\n\
          syspilot explain --pid my_server --deep\n\
          syspilot explain\n\
          syspilot ask \"why does my DB query block?\""
    );
}

fn load_config_or_exit() -> config::Config {
    match config::load_checked() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("❌ Could not load SysPilot configuration: {}", error);
            std::process::exit(1);
        }
    }
}

fn save_config_or_exit(config: &config::Config) {
    if let Err(error) = config::save(config) {
        eprintln!("❌ Could not save SysPilot configuration: {}", error);
        std::process::exit(1);
    }
}

fn json_or_exit<T: serde::Serialize + ?Sized>(value: &T, context: &str) -> String {
    match output::pretty(value) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("❌ Could not render {context}: {error}");
            std::process::exit(1);
        }
    }
}

fn json_value_or_exit(document: &str, context: &'static str) -> serde_json::Value {
    match output::parse_value(document, context) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("❌ {error}");
            std::process::exit(1);
        }
    }
}

fn envelope_json_or_exit<T: serde::Serialize>(
    command: &str,
    outcome: output::Outcome,
    data: T,
) -> String {
    match output::OutputEnvelopeV1::new(command, outcome, data, Vec::new()) {
        Ok(envelope) => json_or_exit(&envelope, command),
        Err(error) => {
            eprintln!("❌ Could not create {command} output: {error}");
            std::process::exit(1);
        }
    }
}

fn create_support_bundle_or_exit(args: &[String], json_requested: bool) {
    if args.get(2).map(String::as_str) != Some("bundle")
        || args.get(3).map(String::as_str) != Some("create")
        || args.len() > 5
    {
        eprintln!("❌ Usage: syspilot support bundle create [path]");
        std::process::exit(1);
    }
    let destination = args.get(4).map(std::path::Path::new);
    match support::create(
        &config::get_syspilot_dir(),
        &config::daemon_health_path(),
        destination,
    ) {
        Ok(result) => {
            let outcome = if result.bundle.complete {
                output::Outcome::Ok
            } else {
                output::Outcome::Degraded
            };
            if json_requested {
                println!(
                    "{}",
                    envelope_json_or_exit(
                        "support.bundle.create",
                        outcome,
                        serde_json::json!({"path": &result.path, "bundle": &result.bundle})
                    )
                );
            } else {
                println!("✅ Support bundle written to {}", result.path.display());
                for component in &result.bundle.components {
                    println!(
                        "{:?} {}: {}",
                        component.state, component.name, component.detail
                    );
                }
                println!("Redacted fields: {}", result.bundle.redaction.len());
            }
            if !result.bundle.complete {
                eprintln!("❌ Support bundle is partial; one or more components were unavailable or failed. Inspect the component records before sharing it.");
                std::process::exit(2);
            }
        }
        Err(error) => {
            eprintln!("❌ Support bundle creation failed before a safe artifact could be written: {error}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let typed_cli = syspilot::cli::Cli::parse();
    let mut args: Vec<String> = std::env::args().collect();
    let json_requested = typed_cli.json;
    args.retain(|argument| argument != "--json");
    if args.len() < 2 {
        print_help();
        return;
    }
    if matches!(args[1].as_str(), "version" | "--version" | "-V") {
        println!("syspilot {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if json_requested
        && !matches!(
            args[1].as_str(),
            "status"
                | "doctor"
                | "evidence"
                | "cases"
                | "alerts"
                | "config"
                | "events"
                | "support"
                | "fleet"
        )
    {
        eprintln!(
            "❌ Stable JSON output is not implemented for '{}'; supported commands: doctor, evidence, cases, alerts, config telemetry show/preview",
            args[1]
        );
        std::process::exit(1);
    }
    if json_requested
        && args[1] == "config"
        && !(args.get(2).map(String::as_str) == Some("telemetry")
            && matches!(
                args.get(3).map(String::as_str).unwrap_or("show"),
                "show" | "preview"
            ))
    {
        eprintln!("❌ Stable JSON output for this configuration action is not implemented; use config telemetry show/preview --json");
        std::process::exit(1);
    }
    if args[1] == "completions" {
        let Some(shell) = args.get(2) else {
            eprintln!("❌ Usage: syspilot completions <bash|zsh|fish>");
            std::process::exit(1);
        };
        match completions::generate(shell) {
            Ok(script) => print!("{script}"),
            Err(error) => {
                eprintln!("❌ {error}");
                std::process::exit(1);
            }
        }
        return;
    }

    if args[1] == "config" && args.get(2).map(String::as_str) == Some("rollback") {
        match config::rollback_previous() {
            Ok(backup) => println!(
                "✅ Restored configuration from {}. Restart SysPilot with the compatible version.",
                backup.display()
            ),
            Err(error) => {
                eprintln!("❌ Configuration rollback failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args[1] == "support" {
        create_support_bundle_or_exit(&args, json_requested);
        return;
    }
    if args[1] == "setup" {
        let mode = match args.get(2).map(String::as_str) {
            None => setup::InterfaceMode::Auto,
            Some("--tui") if args.len() == 3 => setup::InterfaceMode::Tui,
            Some("--line") if args.len() == 3 => setup::InterfaceMode::Line,
            Some("--check") if args.len() == 3 => setup::InterfaceMode::Check,
            _ => {
                eprintln!("❌ Usage: syspilot setup [--tui|--line|--check]");
                std::process::exit(1);
            }
        };
        if !setup::run(mode) {
            std::process::exit(1);
        }
        return;
    }

    let mut conf = load_config_or_exit();
    let cmd = args[1].as_str();

    match cmd {
        "install" => {
            let binary = args.iter().any(|arg| arg == "--binary");
            let force = args.iter().any(|arg| arg == "--force");
            if binary {
                install::install_user_binary(force);
            } else {
                install::install();
            }
        }
        "uninstall" => {
            if args.iter().any(|arg| arg == "--binary") {
                install::remove_user_binary();
            } else {
                install::uninstall();
            }
        }
        "status" => {
            if json_requested {
                match install::collect_status() {
                    Ok(report) => {
                        println!("{}", json_or_exit(&report, "status"));
                        if report.outcome != output::Outcome::Ok {
                            std::process::exit(report.outcome.exit_code().into());
                        }
                    }
                    Err(error) => {
                        eprintln!("❌ Status failed: {error}");
                        std::process::exit(1);
                    }
                }
            } else {
                install::status();
            }
        }
        "doctor" => match doctor::collect() {
            Ok(report) => {
                if json_requested {
                    println!("{}", json_or_exit(&report, "doctor report"));
                } else {
                    doctor::render_human(&report);
                }
                if report.outcome != output::Outcome::Ok {
                    std::process::exit(report.outcome.exit_code().into());
                }
            }
            Err(error) => {
                eprintln!("❌ Doctor failed: {error}");
                std::process::exit(1);
            }
        },
        "evidence" => {
            let mut target = None;
            let mut index = 2;
            while index < args.len() {
                if args[index] == "--pid" && index + 1 < args.len() {
                    target = Some(args[index + 1].as_str());
                    index += 2;
                } else {
                    eprintln!("❌ Usage: syspilot evidence [--pid <pid/name>]");
                    std::process::exit(1);
                }
            }
            match evidence::capture(target).and_then(|bundle| {
                evidence::CaseStore::default().save(&bundle)?;
                Ok(bundle)
            }) {
                Ok(bundle) => {
                    let outcome = if bundle.missing_evidence.is_empty() {
                        output::Outcome::Ok
                    } else {
                        output::Outcome::Degraded
                    };
                    if json_requested {
                        println!("{}", envelope_json_or_exit("evidence", outcome, &bundle));
                    } else {
                        println!("✅ Stored evidence case {}", bundle.case_id);
                        println!(
                            "Observations: {}  Findings: {}  Missing evidence: {}",
                            bundle.observations.len(),
                            bundle.findings.len(),
                            bundle.missing_evidence.len()
                        );
                    }
                    if outcome == output::Outcome::Degraded {
                        std::process::exit(2);
                    }
                }
                Err(error) => {
                    eprintln!("❌ Could not capture evidence: {error}");
                    std::process::exit(1);
                }
            }
        }
        "cases" => {
            let store = evidence::CaseStore::default();
            let action = args.get(2).map(String::as_str).unwrap_or("");
            let outcome = match action {
                "list" => store.list().map(|cases| {
                    if json_requested {
                        println!(
                            "{}",
                            envelope_json_or_exit("cases.list", output::Outcome::Ok, &cases)
                        );
                    } else {
                        if cases.is_empty() {
                            println!("No diagnostic cases stored.");
                        }
                        for case in cases {
                            println!(
                                "{}  {} bytes  {} findings  captured_ns={}",
                                case.case_id,
                                case.bytes,
                                case.findings,
                                case.captured_at_unix_nanos
                            );
                        }
                    }
                }),
                "show" if args.len() == 4 => store.load(&args[3]).and_then(|bundle| {
                    if json_requested {
                        println!(
                            "{}",
                            envelope_json_or_exit("cases.show", output::Outcome::Ok, &bundle)
                        );
                    } else {
                        println!("{}", output::pretty(&bundle)?);
                    }
                    Ok(())
                }),
                "export" if args.len() == 4 || args.len() == 5 => {
                    let destination = args
                        .get(4)
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from(format!("{}.json", args[3])));
                    store.export(&args[3], &destination).map(|_| {
                        if json_requested {
                            println!(
                                "{}",
                                envelope_json_or_exit(
                                    "cases.export",
                                    output::Outcome::Ok,
                                    serde_json::json!({"case_id": args[3], "destination": destination})
                                )
                            );
                        } else {
                            println!("✅ Exported {}", destination.display());
                        }
                    })
                }
                "delete" if args.len() == 4 => store.delete(&args[3]).map(|_| {
                    if json_requested {
                        println!(
                            "{}",
                            envelope_json_or_exit(
                                "cases.delete",
                                output::Outcome::Ok,
                                serde_json::json!({"case_id": args[3], "deleted": true})
                            )
                        );
                    } else {
                        println!("✅ Deleted case {}", args[3]);
                    }
                }),
                _ => {
                    eprintln!("❌ Usage: syspilot cases <list|show|export|delete> [id] [path]");
                    std::process::exit(1);
                }
            };
            if let Err(error) = outcome {
                eprintln!("❌ Case operation failed: {error}");
                std::process::exit(1);
            }
        }
        "fleet" => match args.get(2).map(String::as_str).unwrap_or("") {
            "enroll" if args.len() == 5 || args.len() == 6 => {
                let credential = std::env::var("SYSPILOT_FLEET_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .or_else(|| args.get(5).cloned())
                    .unwrap_or_default();
                if credential.is_empty() {
                    eprintln!("❌ Set SYSPILOT_FLEET_TOKEN or provide the token argument.");
                    std::process::exit(1);
                }
                if let Err(error) =
                    fleet::enroll(&mut conf, args[3].clone(), args[4].clone(), credential)
                        .and_then(|_| config::save(&conf))
                {
                    eprintln!("❌ Fleet enrollment failed: {error}");
                    std::process::exit(1);
                }
                if json_requested {
                    println!(
                        "{}",
                        envelope_json_or_exit(
                            "fleet.enroll",
                            output::Outcome::Ok,
                            serde_json::json!({"enabled": true, "node_id": conf.fleet.node_id, "endpoint": conf.fleet.endpoint, "credential_configured": true})
                        )
                    );
                } else {
                    println!(
                        "✅ Fleet enrollment enabled for node {}",
                        conf.fleet.node_id
                    );
                    println!("Restart the daemon to begin redacted hosted telemetry delivery.");
                }
            }
            "status" if args.len() == 3 => {
                if json_requested {
                    match fleet::collect_status(&conf) {
                        Ok(report) => {
                            println!("{}", json_or_exit(&report, "fleet status"));
                            if report.outcome != output::Outcome::Ok {
                                std::process::exit(report.outcome.exit_code().into());
                            }
                        }
                        Err(error) => {
                            eprintln!("❌ Fleet status failed: {error}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    fleet::print_status(&conf);
                }
            }
            "disable" if args.len() == 3 => {
                fleet::disable(&mut conf);
                if let Err(error) = config::save(&conf) {
                    eprintln!("❌ Could not disable fleet enrollment: {error}");
                    std::process::exit(1);
                }
                if json_requested {
                    println!(
                        "{}",
                        envelope_json_or_exit(
                            "fleet.disable",
                            output::Outcome::Ok,
                            serde_json::json!({"enabled": false, "credential_configured": false, "local_diagnostics_available": true})
                        )
                    );
                } else {
                    println!("✅ Fleet enrollment disabled and hosted credential removed.");
                    println!("Local diagnostics remain available.");
                }
            }
            _ => {
                eprintln!("❌ Usage: syspilot fleet <enroll|status|disable>");
                std::process::exit(1);
            }
        },
        "alerts" => {
            let action = args.get(2).map(String::as_str).unwrap_or("list");
            let mut store = match alert::AlertStore::open(alert::default_path()) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("❌ Could not load alert state: {error}");
                    std::process::exit(1);
                }
            };
            match action {
                "list" if args.len() == 2 || args.len() == 3 => {
                    if json_requested {
                        println!(
                            "{}",
                            envelope_json_or_exit("alerts.list", output::Outcome::Ok, store.list())
                        );
                    } else {
                        println!("{}", json_or_exit(&store.list(), "alert state"));
                    }
                }
                "acknowledge" | "resolve" | "suppress" if args.len() == 4 => {
                    let status = match action {
                        "acknowledge" => alert::AlertStatus::Acknowledged,
                        "resolve" => alert::AlertStatus::Resolved,
                        _ => alert::AlertStatus::Suppressed,
                    };
                    let detail = format!("operator action: {action}");
                    match store.set_status(&args[3], status, detail, alert::current_time_ns()) {
                        Ok(record) => {
                            if json_requested {
                                println!(
                                    "{}",
                                    envelope_json_or_exit(
                                        &format!("alerts.{action}"),
                                        output::Outcome::Ok,
                                        record
                                    )
                                );
                            } else {
                                println!(
                                    "✅ Alert {} is now {:?}",
                                    record.instance_id, record.status
                                )
                            }
                        }
                        Err(error) => {
                            eprintln!("❌ Alert transition failed: {error}");
                            std::process::exit(1);
                        }
                    }
                }
                _ => {
                    eprintln!("❌ Usage: syspilot alerts <list|acknowledge|resolve|suppress> [instance-id]");
                    std::process::exit(1);
                }
            }
        }
        "daemon" => {
            std::process::exit(daemon::run_daemon(conf));
        }
        "events" => match daemon_client::events() {
            Ok(value) => {
                if json_requested {
                    println!(
                        "{}",
                        envelope_json_or_exit("events", output::Outcome::Ok, value)
                    );
                } else {
                    println!("{}", json_or_exit(&value, "daemon events"));
                }
            }
            Err(error) => {
                eprintln!("❌ {}. Start it with `syspilot daemon &` first.", error);
                std::process::exit(1);
            }
        },
        "monitor" => {
            ui::tui::run_monitor();
        }
        "provider" => {
            if args.len() < 3 {
                eprintln!("❌ Expected provider name (disabled, gemini, ollama, syspilot)");
                std::process::exit(1);
            }
            let prov = args[2].to_lowercase();
            if !["disabled", "gemini", "ollama", "syspilot"].contains(&prov.as_str()) {
                eprintln!(
                    "❌ Unknown provider: {}. Use: disabled, gemini, ollama, syspilot.",
                    prov
                );
                std::process::exit(1);
            }
            conf.active_provider = prov.clone();
            save_config_or_exit(&conf);
            println!("✅ Active provider set to {}", prov);
        }
        "model" => {
            if args.len() < 3 {
                eprintln!("❌ Expected model name.");
                std::process::exit(1);
            }
            let model = &args[2];
            match conf.active_provider.as_str() {
                "gemini" => conf.gemini_model = model.clone(),
                "ollama" => conf.ollama_model = model.clone(),
                "syspilot" => conf.syspilot_model = model.clone(),
                _ => {}
            }
            save_config_or_exit(&conf);
            println!("✅ Model set to {} for {}", model, conf.active_provider);
        }
        "pull" => {
            if args.len() < 3 {
                eprintln!("❌ Expected model name.");
                std::process::exit(1);
            }
            let model = &args[2];
            let set_active = args.get(3).map(|a| a == "--set-active").unwrap_or(false);
            if conf.active_provider != "ollama" {
                println!(
                    "⚠️  Current provider is {}. Model pulling only works with Ollama.",
                    conf.active_provider
                );
                print!("Switch to Ollama? [Y/n] ");
                use std::io::Write;
                if let Err(error) = std::io::stdout().flush() {
                    eprintln!("❌ Could not display confirmation prompt: {error}");
                    std::process::exit(1);
                }
                let mut input = String::new();
                match std::io::stdin().read_line(&mut input) {
                    Ok(0) => {
                        eprintln!("❌ Input closed before confirmation; pull aborted.");
                        std::process::exit(1);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("❌ Could not read confirmation: {error}");
                        std::process::exit(1);
                    }
                }
                let trimmed = input.trim().to_lowercase();
                if trimmed != "y" && !trimmed.is_empty() {
                    println!("Pull aborted.");
                    return;
                }
                conf.active_provider = "ollama".to_string();
                save_config_or_exit(&conf);
            }
            if ai::pull_ollama_model(&conf, model) && set_active {
                conf.ollama_model = model.clone();
                save_config_or_exit(&conf);
                println!("🔧 Set as active Ollama model.");
            }
        }
        "index" => {
            let force = args.get(2).map(|a| a == "--force").unwrap_or(false);
            let (pwd, _) = utils::run_command_output("pwd");
            if !codebase::update_index(pwd.trim(), &conf, force) {
                std::process::exit(1);
            }
        }
        "config" => {
            if args.len() < 3 {
                eprintln!("❌ Expected config action (telemetry, alert, set-key, set-url, set)");
                std::process::exit(1);
            }
            match args[2].as_str() {
                "telemetry" => {
                    let action = args.get(3).map(String::as_str).unwrap_or("show");
                    match action {
                        "enable" => {
                            if args.len() < 6 {
                                eprintln!("❌ Usage: syspilot config telemetry enable <endpoint> <node-id> [bearer-token]");
                                std::process::exit(1);
                            }
                            conf.distributed_telemetry.enabled = true;
                            conf.distributed_telemetry.endpoint = args[4].clone();
                            conf.distributed_telemetry.node_id = args[5].clone();
                            conf.distributed_telemetry.bearer_token =
                                args.get(6).cloned().unwrap_or_default();
                            conf.distributed_telemetry.bearer_credential =
                                if conf.distributed_telemetry.bearer_token.is_empty() {
                                    syspilot::credentials::CredentialRef::None
                                } else {
                                    match syspilot::credentials::store_owner_secret(
                                        &config::get_syspilot_dir().join("credentials"),
                                        "telemetry-token",
                                        &conf.distributed_telemetry.bearer_token,
                                    ) {
                                        Ok(reference) => reference,
                                        Err(error) => {
                                            eprintln!(
                                                "❌ Could not store telemetry credential: {error}"
                                            );
                                            std::process::exit(1);
                                        }
                                    }
                                };
                            save_config_or_exit(&conf);
                            println!(
                                "✅ Distributed telemetry enabled for node {}",
                                conf.distributed_telemetry.node_id
                            );
                        }
                        "disable" => {
                            conf.distributed_telemetry.enabled = false;
                            conf.distributed_telemetry.bearer_token.clear();
                            conf.distributed_telemetry.bearer_credential =
                                syspilot::credentials::CredentialRef::None;
                            save_config_or_exit(&conf);
                            println!("✅ Distributed telemetry disabled");
                        }
                        "show" => {
                            let mut view = conf.distributed_telemetry.clone();
                            if !view.bearer_token.is_empty() {
                                view.bearer_token = "[configured]".to_string();
                            }
                            if json_requested {
                                println!(
                                    "{}",
                                    envelope_json_or_exit(
                                        "config.telemetry.show",
                                        output::Outcome::Ok,
                                        &view
                                    )
                                );
                            } else {
                                println!("{}", json_or_exit(&view, "telemetry configuration"));
                            }
                        }
                        "preview" => {
                            let envelope = if let Some(target) = args.get(4) {
                                let pid = telemetry::find_pid_by_name(target);
                                if pid == 0 {
                                    eprintln!("❌ Could not find process PID for: {target}");
                                    std::process::exit(1);
                                }
                                distributed::preview_envelope(
                                    &conf.distributed_telemetry,
                                    distributed::TelemetryKind::ProcessSnapshot,
                                    &telemetry::collect_process_telemetry(pid),
                                )
                            } else {
                                distributed::preview_envelope(
                                    &conf.distributed_telemetry,
                                    distributed::TelemetryKind::SystemSnapshot,
                                    &telemetry::collect_system_telemetry(),
                                )
                            };
                            match envelope {
                                Ok(envelope) => {
                                    if json_requested {
                                        println!(
                                            "{}",
                                            envelope_json_or_exit(
                                                "config.telemetry.preview",
                                                output::Outcome::Ok,
                                                &envelope
                                            )
                                        );
                                    } else {
                                        println!(
                                            "{}",
                                            json_or_exit(&envelope, "telemetry preview")
                                        );
                                    }
                                }
                                Err(error) => {
                                    eprintln!("❌ Could not preview telemetry: {error}");
                                    std::process::exit(1);
                                }
                            }
                        }
                        _ => {
                            eprintln!(
                                "❌ Usage: syspilot config telemetry <enable|disable|show|preview>"
                            );
                            std::process::exit(1);
                        }
                    }
                }
                "alert" => {
                    let action = args.get(3).map(String::as_str).unwrap_or("list");
                    match action {
                        "add" => {
                            if args.len() < 7 {
                                eprintln!("❌ Usage: syspilot config alert add <id> <exact|prefix> <process-name>");
                                std::process::exit(1);
                            }
                            let match_type = match args[5].as_str() {
                                "exact" => distributed::ProcessNameMatch::Exact,
                                "prefix" => distributed::ProcessNameMatch::Prefix,
                                _ => {
                                    eprintln!("❌ Match type must be exact or prefix");
                                    std::process::exit(1);
                                }
                            };
                            let id = args[4].clone();
                            if conf
                                .distributed_telemetry
                                .process_alert_rules
                                .iter()
                                .any(|rule| rule.id == id)
                            {
                                eprintln!("❌ Alert rule {} already exists", id);
                                std::process::exit(1);
                            }
                            conf.distributed_telemetry.process_alert_rules.push(
                                distributed::ProcessAlertRule {
                                    id,
                                    process_name: args[6].clone(),
                                    match_type,
                                    enabled: true,
                                    labels: std::collections::BTreeMap::new(),
                                },
                            );
                            save_config_or_exit(&conf);
                            println!("✅ Process alert rule added");
                        }
                        "remove" => {
                            let Some(id) = args.get(4) else {
                                eprintln!("❌ Usage: syspilot config alert remove <id>");
                                std::process::exit(1);
                            };
                            let before = conf.distributed_telemetry.process_alert_rules.len();
                            conf.distributed_telemetry
                                .process_alert_rules
                                .retain(|rule| rule.id != *id);
                            if before == conf.distributed_telemetry.process_alert_rules.len() {
                                eprintln!("❌ Alert rule {} not found", id);
                                std::process::exit(1);
                            }
                            save_config_or_exit(&conf);
                            println!("✅ Process alert rule removed");
                        }
                        "list" => println!(
                            "{}",
                            json_or_exit(
                                &conf.distributed_telemetry.process_alert_rules,
                                "alert rules"
                            )
                        ),
                        _ => {
                            eprintln!("❌ Usage: syspilot config alert <add|remove|list>");
                            std::process::exit(1);
                        }
                    }
                }
                "set-key" => {
                    if args.len() < 5 {
                        eprintln!("❌ Usage: syspilot config set-key <provider> <key>");
                        std::process::exit(1);
                    }
                    let prov = args[3].to_lowercase();
                    let key = &args[4];
                    if let Err(error) = config::set_provider_credential(&mut conf, &prov, key) {
                        eprintln!("❌ Could not store provider credential: {error}");
                        std::process::exit(1);
                    }
                    save_config_or_exit(&conf);
                    println!("✅ API key set for {}", prov);
                }
                "set-url" => {
                    if args.len() < 5 {
                        eprintln!("❌ Usage: syspilot config set-url <provider> <url>");
                        std::process::exit(1);
                    }
                    let prov = args[3].to_lowercase();
                    if prov != "ollama" {
                        eprintln!("❌ Provider {} does not support custom URLs.", prov);
                        std::process::exit(1);
                    }
                    conf.ollama_url = args[4].clone();
                    save_config_or_exit(&conf);
                    println!("✅ URL set for {}", prov);
                }
                "set" => {
                    if args.len() < 5 {
                        eprintln!("❌ Usage: syspilot config set <option> <value>");
                        std::process::exit(1);
                    }
                    let opt = args[3].to_lowercase();
                    let val = &args[4];
                    match opt.as_str() {
                        "chunk_strategy" | "strategy" => {
                            if val != "syntactic" && val != "line" {
                                eprintln!("❌ Invalid chunk strategy. Options: syntactic, line");
                                std::process::exit(1);
                            }
                            conf.chunk_strategy = val.clone();
                        }
                        "embedding_model" | "model" => {
                            conf.embedding_model = val.clone();
                        }
                        "embedding_provider" | "embedding-provider" => {
                            if !["active", "gemini", "ollama"].contains(&val.as_str()) {
                                eprintln!("❌ Invalid embedding provider. Options: active, gemini, ollama");
                                std::process::exit(1);
                            }
                            conf.embedding_provider = val.clone();
                        }
                        _ => {
                            eprintln!(
                                "❌ Unknown option: {}. Options: chunk_strategy, embedding_model, embedding_provider",
                                opt
                            );
                            std::process::exit(1);
                        }
                    }
                    save_config_or_exit(&conf);
                    println!("✅ Config {} set to {}", opt, val);
                }
                other => {
                    eprintln!("❌ Unknown config action: {}", other);
                    std::process::exit(1);
                }
            }
        }
        "ask" => {
            if args.len() < 3 {
                eprintln!("❌ Expected question.");
                std::process::exit(1);
            }
            let question = &args[2];
            let mut file_path = String::new();
            let mut no_index = false;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--file" if i + 1 < args.len() => {
                        file_path = args[i + 1].clone();
                        i += 2;
                    }
                    "--no-index" => {
                        no_index = true;
                        i += 1;
                    }
                    _ => {
                        i += 1;
                    }
                }
            }

            println!("\n🧠 \x1b[1;32mSysPilot Answer:\x1b[0m\n");

            let (pwd, _) = utils::run_command_output("pwd");
            let (ls, _) = utils::run_command_output("ls -lA 2>/dev/null");
            let mut ctx = serde_json::json!({
                "current_dir": pwd.trim(),
                "file_list":   ls.trim(),
            });

            if utils::file_exists(".git") {
                let (branch, _) =
                    utils::run_command_output("git branch --show-current 2>/dev/null");
                let (status, _) = utils::run_command_output("git status --porcelain 2>/dev/null");
                ctx["git_branch"] = serde_json::json!(branch.trim());
                ctx["git_status"] = serde_json::json!(status.trim());
            }

            if !file_path.is_empty() {
                match utils::read_file_content(&file_path) {
                    Ok(content) => {
                        let lines: Vec<&str> = content.lines().collect();
                        let truncated = if lines.len() > 300 {
                            format!("{}\n... (truncated)", lines[..300].join("\n"))
                        } else {
                            content.clone()
                        };
                        ctx["file_content"] = serde_json::json!(format!(
                            "File '{}' contents:\n{}",
                            file_path, truncated
                        ));
                    }
                    Err(_) => {
                        ctx["file_content"] =
                            serde_json::json!(format!("Could not read file '{}'", file_path));
                    }
                }
            }

            if !no_index {
                println!("🔍 Searching local codebase context...");
                ctx["codebase_context"] =
                    serde_json::json!(codebase::query_context(pwd.trim(), question, &conf));
            }

            let prompt = format!(
                "Terminal Context:\n{}\n\nQuestion: {}",
                json_or_exit(&ctx, "AI request context"),
                question
            );

            let mut streamer = MdStreamer::new();
            if !ai::query_ai_stream(&conf, &prompt, &mut streamer) {
                std::process::exit(1);
            }
            println!("\n\x1b[90m{}\x1b[0m", "-".repeat(60));
        }
        "explain" => {
            let mut target = String::new();
            let mut deep = false;
            let mut ebpf = false;
            let mut causal = false;
            let mut offset = 1usize;
            let mut no_index = false;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--pid" if i + 1 < args.len() => {
                        target = args[i + 1].clone();
                        i += 2;
                    }
                    "--deep" => {
                        deep = true;
                        i += 1;
                    }
                    "--ebpf" => {
                        ebpf = true;
                        i += 1;
                    }
                    "--causal" => {
                        causal = true;
                        i += 1;
                    }
                    "--no-index" => {
                        no_index = true;
                        i += 1;
                    }
                    "--number" if i + 1 < args.len() => {
                        offset = args[i + 1].parse().unwrap_or(1);
                        i += 2;
                    }
                    _ => {
                        i += 1;
                    }
                }
            }

            println!("\n🤖 \x1b[1;36mSysPilot Explanation:\x1b[0m\n");

            if causal && target.is_empty() {
                eprintln!(
                    "❌ --causal requires --pid. Usage: syspilot explain --pid <name|pid> --causal"
                );
                std::process::exit(1);
            }

            let (pwd, _) = utils::run_command_output("pwd");
            let mut ctx = serde_json::json!({ "current_dir": pwd.trim() });

            if !target.is_empty() {
                let pid = telemetry::find_pid_by_name(&target);
                if pid == 0 {
                    eprintln!("❌ Could not find process PID for: {}", target);
                    std::process::exit(1);
                }

                if causal {
                    println!("🌐 Constructing real-time Causal Dependency Graph (CausalTrace)...");
                    let mut graph = CausalGraph::new();
                    graph.build_graph(2, ebpf, pid);

                    let node_id = format!("pid:{}", pid);
                    println!("🔍 Tracing root causes from {} ({})...", target, node_id);
                    let path = graph.trace_root_cause(&node_id);
                    let chain = graph.serialize_chain_to_json(&path);

                    ctx["causal_chain"] = json_value_or_exit(&chain, "causal chain");
                    ctx["analysis_type"] = serde_json::json!("causal_inference_diagnostics");
                    ctx["target_process"] = serde_json::json!(target);
                    ctx["target_pid"] = serde_json::json!(pid);

                    // Save reports
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                    let reports_dir = format!("{}/syspilot_reports", home);
                    if let Err(error) = fs::create_dir_all(&reports_dir) {
                        eprintln!("❌ Could not create report directory {reports_dir}: {error}");
                    }
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let dot_path = format!("{}/causal_graph_{}.dot", reports_dir, ts);
                    let html_path = format!("{}/causal_graph_{}.html", reports_dir, ts);
                    let dot = graph.export_graph_to_dot(&path);
                    let html = graph.export_graph_to_html(&path);
                    let dot_result = syspilot::config_migration::atomic_write(
                        std::path::Path::new(&dot_path),
                        dot.as_bytes(),
                    );
                    let html_result = syspilot::config_migration::atomic_write(
                        std::path::Path::new(&html_path),
                        html.as_bytes(),
                    );
                    match dot_result {
                        Ok(()) => println!("💾 Saved DOT causal graph: {dot_path}"),
                        Err(error) => {
                            eprintln!("❌ Could not write requested DOT report {dot_path}: {error}")
                        }
                    }
                    match html_result {
                        Ok(()) => println!("💾 Saved HTML causal graph: {html_path}"),
                        Err(error) => eprintln!(
                            "❌ Could not write requested HTML report {html_path}: {error}"
                        ),
                    }

                    if !no_index {
                        println!("🔍 Mapping causal nodes back to codebase...");
                        ctx["codebase_context"] =
                            serde_json::json!(codebase::query_context(pwd.trim(), &target, &conf));
                    }
                } else {
                    println!("📊 Gathering telemetry for PID {} ({})...", pid, target);
                    let pt = telemetry::collect_process_telemetry(pid);
                    let st = telemetry::collect_system_telemetry();

                    if ebpf {
                        ctx["ebpf_events"] =
                            serde_json::json!(telemetry::collect_ebpf_telemetry(pid, 5));
                    }

                    println!("🔬 Extracting thread stack traces...");
                    let pr = profiler::profile_process(pid, deep);

                    ctx["telemetry"] =
                        serde_json::from_str(&telemetry::serialize_telemetry_to_json(&pt, &st))
                            .unwrap_or_default();
                    ctx["execution_profile"] =
                        serde_json::from_str(&profiler::serialize_profile_to_json(&pr))
                            .unwrap_or_default();

                    let mut query_terms = pt.name.clone();
                    for (sym, _) in &pr.top_symbols {
                        query_terms.push(' ');
                        query_terms.push_str(sym);
                    }
                    for st in pr.active_stacks.iter().take(3) {
                        for f in st.frames.iter().take(3) {
                            query_terms.push(' ');
                            query_terms.push_str(f);
                        }
                    }

                    if !no_index {
                        println!("🔍 Mapping execution profile back to codebase...");
                        ctx["codebase_context"] = serde_json::json!(codebase::query_context(
                            pwd.trim(),
                            &query_terms,
                            &conf
                        ));
                    }

                    ctx["analysis_type"] = serde_json::json!("process_telemetry_and_profiling");
                    ctx["target_process"] = serde_json::json!(target);
                    ctx["target_pid"] = serde_json::json!(pid);
                }
            } else {
                // Classic terminal command debugging mode
                let recent = read_recent_entries(offset);
                let entry = recent.into_iter().next();
                let (last_cmd, exit_code) = entry
                    .map(|e| (e.command, e.exit_code))
                    .unwrap_or_else(|| ("No recent command found".to_string(), 0));

                ctx["last_command"] = serde_json::json!(last_cmd);
                ctx["exit_code"] = serde_json::json!(exit_code);
                ctx["last_session_snippet"] = serde_json::json!(tail_session(100));

                let (ls, _) = utils::run_command_output("ls -lA 2>/dev/null");
                ctx["file_list"] = serde_json::json!(ls.trim());

                if utils::file_exists(".git") {
                    let (b, _) = utils::run_command_output("git branch --show-current 2>/dev/null");
                    let (s, _) = utils::run_command_output("git status --porcelain 2>/dev/null");
                    ctx["git_branch"] = serde_json::json!(b.trim());
                    ctx["git_status"] = serde_json::json!(s.trim());
                }
                if deep {
                    let (df, _) = utils::run_command_output("df -h 2>/dev/null");
                    let (mem, _) = utils::run_command_output("free -h 2>/dev/null");
                    let (ss, _) = utils::run_command_output("ss -tlnp 2>/dev/null");
                    ctx["disk_usage"] = serde_json::json!(df.trim());
                    ctx["memory_usage"] = serde_json::json!(mem.trim());
                    ctx["open_ports"] = serde_json::json!(ss.trim());
                }
                if !no_index {
                    println!("🔍 Searching codebase for command context...");
                    let q = format!("{}\n{}", last_cmd, tail_session(10));
                    ctx["codebase_context"] =
                        serde_json::json!(codebase::query_context(pwd.trim(), &q, &conf));
                }
                ctx["analysis_type"] = serde_json::json!("command_failure_diagnostics");
            }

            if !conf.distributed_telemetry.process_alert_rules.is_empty() {
                ctx["configured_process_alert_rules"] =
                    serde_json::to_value(&conf.distributed_telemetry.process_alert_rules)
                        .unwrap_or_default();
                match daemon_client::events() {
                    Ok(events) => ctx["recent_lifecycle_events"] = events,
                    Err(error) => {
                        eprintln!("⚠️  Daemon lifecycle events unavailable: {error}. Continuing with explicitly degraded local evidence.");
                        ctx["recent_lifecycle_events_error"] =
                            serde_json::Value::String(error.to_string());
                    }
                }
            }

            let prompt = if causal {
                format!(
                    "You are a senior system reliability engineer performing root-cause analysis.\n\
                    We have built a directed dependency graph of the OS processes and resources in real-time.\n\
                    A reverse BFS was performed from the symptomatic process node to trace back to potential root causes.\n\n\
                    Here is the structured JSON causal path:\n{}\n\n\
                    Please explain:\n\
                    1. User-facing symptom and what is anomalous\n\
                    2. Step-by-step root cause chain\n\
                    3. Which process is the root cause and why\n\
                    4. Recommended actionable mitigation\n\
                    Be extremely specific. Separate observed evidence from inference, identify missing evidence and alternative hypotheses, and provide a Confidence Score (0-100%). Do not claim a root cause when the evidence only supports correlation.",
                    json_or_exit(&ctx, "causal analysis context")
                )
            } else {
                format!(
                    "Perform causal reasoning on the following OS telemetry and execution context to diagnose the system state.\n\n\
                    OS Telemetry and Context:\n{}\n\nStructure the response as: observed evidence, causal hypothesis, competing explanations, confidence, and safe remediation steps. Do not present inference as fact.",
                    json_or_exit(&ctx, "diagnostic context")
                )
            };

            let mut streamer = MdStreamer::new();
            if !ai::query_ai_stream(&conf, &prompt, &mut streamer) {
                std::process::exit(1);
            }
            println!("\n\x1b[90m{}\x1b[0m", "-".repeat(60));
        }
        _ => {
            print_help();
            std::process::exit(1);
        }
    }
}
