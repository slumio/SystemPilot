// main.rs — binary entry point.
// The global mimalloc allocator is declared in lib.rs.

use syspilot::{
    ai, causal_engine, codebase, config, daemon, distributed, install, profiler, telemetry, ui,
    utils,
};

use causal_engine::CausalGraph;

use std::fs;
use std::io::{BufRead, Read, Write};
use std::time::Duration;
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

fn request_daemon_events() -> Result<String, String> {
    let mut stream = crate::config::connect_daemon()
        .map_err(|error| format!("could not connect to the daemon: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(750)))
        .map_err(|error| format!("could not set daemon read timeout: {error}"))?;
    stream
        .write_all(br#"{"request":"events"}"#)
        .map_err(|error| format!("could not request daemon events: {error}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("could not read daemon events: {error}"))?;
    if response.is_empty() {
        Err("daemon returned an empty event response".to_string())
    } else {
        Ok(response)
    }
}

fn print_help() {
    println!(
        "🤖 SysPilot: Operating System Reasoning Agent (Rust Edition)\n\n\
        Usage: syspilot <command> [options]\n\n\
        Commands:\n\
          setup                         Guided first-run setup
\
          install                       Create local configuration and shell hook
\
          install --binary [--force]    Copy this binary to ~/.local/bin
\
          uninstall --binary            Remove only the user-local binary\n\
          uninstall                     Remove terminal hooks\n\
          status                        Check integration status\n\
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_help();
        return;
    }
    if matches!(args[1].as_str(), "version" | "--version" | "-V") {
        println!("syspilot {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let mut conf = load_config_or_exit();
    let cmd = args[1].as_str();

    match cmd {
        "setup" => {
            install::setup();
        }
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
            install::status();
        }
        "daemon" => {
            std::process::exit(daemon::run_daemon(conf));
        }
        "events" => match request_daemon_events() {
            Ok(events) => match serde_json::from_str::<serde_json::Value>(&events) {
                Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap_or(events)),
                Err(_) => println!("{}", events),
            },
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
                eprintln!("❌ Expected provider name (gemini, ollama, syspilot)");
                std::process::exit(1);
            }
            let prov = args[2].to_lowercase();
            if !["gemini", "ollama", "syspilot"].contains(&prov.as_str()) {
                eprintln!(
                    "❌ Unknown provider: {}. Use: gemini, ollama, syspilot.",
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
                let _ = std::io::stdout().flush();
                let mut input = String::new();
                let _ = std::io::stdin().read_line(&mut input);
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
                            save_config_or_exit(&conf);
                            println!(
                                "✅ Distributed telemetry enabled for node {}",
                                conf.distributed_telemetry.node_id
                            );
                        }
                        "disable" => {
                            conf.distributed_telemetry.enabled = false;
                            save_config_or_exit(&conf);
                            println!("✅ Distributed telemetry disabled");
                        }
                        "show" => {
                            let mut view = conf.distributed_telemetry.clone();
                            if !view.bearer_token.is_empty() {
                                view.bearer_token = "[configured]".to_string();
                            }
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&view).unwrap_or_default()
                            );
                        }
                        _ => {
                            eprintln!("❌ Usage: syspilot config telemetry <enable|disable|show>");
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
                            serde_json::to_string_pretty(
                                &conf.distributed_telemetry.process_alert_rules
                            )
                            .unwrap_or_default()
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
                    match prov.as_str() {
                        "gemini" => conf.gemini_api_key = key.clone(),
                        "syspilot" => conf.syspilot_api_key = key.clone(),
                        _ => {
                            eprintln!("❌ Provider {} does not use API keys.", prov);
                            std::process::exit(1);
                        }
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
                serde_json::to_string_pretty(&ctx).unwrap_or_default(),
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

                    ctx["causal_chain"] = serde_json::from_str(&chain).unwrap_or_default();
                    ctx["analysis_type"] = serde_json::json!("causal_inference_diagnostics");
                    ctx["target_process"] = serde_json::json!(target);
                    ctx["target_pid"] = serde_json::json!(pid);

                    // Save reports
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                    let reports_dir = format!("{}/syspilot_reports", home);
                    let _ = fs::create_dir_all(&reports_dir);
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let dot_path = format!("{}/causal_graph_{}.dot", reports_dir, ts);
                    let html_path = format!("{}/causal_graph_{}.html", reports_dir, ts);
                    let _ = fs::write(&dot_path, graph.export_graph_to_dot(&path));
                    let _ = fs::write(&html_path, graph.export_graph_to_html(&path));
                    println!("💾 Saved causal graph to:");
                    println!("   - DOT:  {}", dot_path);
                    println!("   - HTML: {}\n", html_path);

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
                if let Ok(events) = request_daemon_events() {
                    ctx["recent_lifecycle_events"] = serde_json::from_str(&events)
                        .unwrap_or_else(|_| serde_json::json!({"raw": events}));
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
                    serde_json::to_string_pretty(&ctx).unwrap_or_default()
                )
            } else {
                format!(
                    "Perform causal reasoning on the following OS telemetry and execution context to diagnose the system state.\n\n\
                    OS Telemetry and Context:\n{}\n\nStructure the response as: observed evidence, causal hypothesis, competing explanations, confidence, and safe remediation steps. Do not present inference as fact.",
                    serde_json::to_string_pretty(&ctx).unwrap_or_default()
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
