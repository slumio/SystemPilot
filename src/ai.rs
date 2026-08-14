use crate::config::Config;
use crate::ui::streamer::MdStreamer;
use crate::utils;

pub const REASONING_SYSTEM_PROMPT: &str = "You are the SysPilot Operating System Reasoning Agent, \
an expert in kernel subsystems, hardware architecture, program analysis, and systems performance engineering.\n\
Your job is to analyze low-level operating system telemetry, thread scheduling, memory management, I/O operations, \
performance counters, and execution stacks, and explain the underlying causes of the observed behavior.\n\
Always seek to answer:\n\
1. What happened inside the machine?\n\
2. Why did it happen? What was the root cause?\n\
3. Which process, thread, function, or source code location triggered it?\n\
4. How did the effects propagate through the OS subsystems (scheduler, memory manager, VFS, block layer, network stack) and hardware?\n\
5. What was the impact on performance, latency, throughput, stability, and resource utilization?\n\
6. What specific recommendations (code modifications, configuration changes, architectural adjustments) will resolve the root cause?\n\n\
Connect low-level metrics to high-level software constructs.\n\n\
IMPORTANT TERMINAL FORMATTING RULES:\n\
1. When outputting mathematical formulas or equations, NEVER use raw LaTeX syntax like \\frac or \\sqrt.\n\
2. Instead, use user-friendly, plain-text ASCII math (e.g., (a + b) / c, sqrt(x)).\n\
3. Wrap code or math snippets in backticks (`).\n\
4. Keep explanations concise, clear, and highly technical.\n\
5. Clearly label observations, hypotheses, confidence, and recommended next steps.\n\
6. Never invent telemetry, commands, files, or source locations. If evidence is missing, say what is missing and how to collect it.\n\
7. Prefer safe, reversible checks before disruptive remediation.";

/// Stream AI response to terminal via MdStreamer.
pub fn query_ai_stream(config: &Config, prompt: &str, streamer: &mut MdStreamer) -> bool {
    let (url, headers, payload) = match config.active_provider.as_str() {
        "gemini" => {
            if config.gemini_api_key.is_empty() {
                eprintln!(
                    "❌ Gemini API key is not set. Run `syspilot config set-key gemini YOUR_KEY`"
                );
                return false;
            }
            let model = if config.gemini_model.is_empty()
                || config.gemini_model == "gemini"
                || (!config.gemini_model.contains('/') && !config.gemini_model.contains('-'))
            {
                "gemini-2.0-flash"
            } else {
                &config.gemini_model
            };

            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
                model
            );
            let headers = vec![format!("x-goog-api-key: {}", config.gemini_api_key)];
            let payload = serde_json::json!({
                "contents": [{"parts": [{"text": prompt}]}],
                "systemInstruction": {"parts": [{"text": REASONING_SYSTEM_PROMPT}]}
            })
            .to_string();
            (url, headers, payload)
        }
        "ollama" => {
            let url = format!("{}/api/chat", config.ollama_url);
            let payload = serde_json::json!({
                "model": config.ollama_model,
                "messages": [
                    {"role": "system", "content": REASONING_SYSTEM_PROMPT},
                    {"role": "user", "content": prompt}
                ],
                "stream": true
            })
            .to_string();
            (url, Vec::new(), payload)
        }
        "syspilot" => {
            if config.syspilot_api_key.is_empty() {
                eprintln!("❌ SysPilot API key is not set. Run `syspilot config set-key syspilot YOUR_KEY`");
                return false;
            }
            let url = config.syspilot_url.clone();
            let headers = vec![format!("Authorization: Bearer {}", config.syspilot_api_key)];
            let payload = serde_json::json!({
                "model": config.syspilot_model,
                "messages": [
                    {"role": "system", "content": REASONING_SYSTEM_PROMPT},
                    {"role": "user", "content": prompt}
                ],
                "stream": true
            })
            .to_string();
            (url, headers, payload)
        }
        other => {
            eprintln!("❌ Unknown AI provider: {}", other);
            return false;
        }
    };

    // Build curl args — no shell injection, all args passed directly to execvp
    let mut curl_args: Vec<String> = vec![
        "curl".into(),
        "-sS".into(),
        "--fail-with-body".into(),
        "-N".into(),
        "-X".into(),
        "POST".into(),
        "--max-time".into(),
        config.ai_request_timeout_seconds.to_string(),
        "--connect-timeout".into(),
        config.ai_connect_timeout_seconds.to_string(),
        "-H".into(),
        "Content-Type: application/json".into(),
    ];
    for h in &headers {
        curl_args.push("-H".into());
        curl_args.push(h.clone());
    }
    curl_args.push("-d".into());
    curl_args.push("@-".into()); // read payload from stdin
    curl_args.push(url.clone());

    // Use UnsafeCell to allow mutation of streamer inside the FnMut callback.
    // This is safe because the callback is called single-threadedly by
    // run_command_secure_stream on the current thread.
    let streamer_cell = std::cell::UnsafeCell::new(streamer);
    let done_cell = std::cell::Cell::new(false);
    let received_content = std::cell::Cell::new(false);
    let mut line_buffer2 = String::new();
    let provider2 = config.active_provider.clone();

    let (ok, code) = utils::run_command_secure_stream(&curl_args, payload, |chunk: &str| {
        if done_cell.get() {
            return;
        }
        line_buffer2.push_str(chunk);
        while let Some(nl) = line_buffer2.find('\n') {
            let line = line_buffer2[..nl].trim().to_string();
            line_buffer2.drain(..=nl);
            if line.is_empty() {
                continue;
            }
            // SAFETY: single-threaded callback, no aliasing
            let s: &mut MdStreamer = unsafe { &mut *streamer_cell.get() };

            match provider2.as_str() {
                "gemini" => {
                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data.is_empty() {
                            continue;
                        }
                        if let Ok(j) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(candidates) = j["candidates"].as_array() {
                                for c in candidates {
                                    if let Some(parts) = c["content"]["parts"].as_array() {
                                        for p in parts {
                                            if let Some(text) = p["text"].as_str() {
                                                received_content.set(true);
                                                s.print(text);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                "ollama" => {
                    if let Ok(j) = serde_json::from_str::<serde_json::Value>(&line) {
                        if let Some(text) = j["message"]["content"].as_str() {
                            received_content.set(true);
                            s.print(text);
                        }
                    }
                }
                "syspilot" => {
                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data == "[DONE]" {
                            done_cell.set(true);
                            return;
                        }
                        if let Ok(j) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(choices) = j["choices"].as_array() {
                                for ch in choices {
                                    if let Some(text) = ch["delta"]["content"].as_str() {
                                        received_content.set(true);
                                        s.print(text);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    });

    // Compatible APIs do not always terminate the final event with a newline.
    if !line_buffer2.trim().is_empty() {
        let line = line_buffer2.trim();
        let s: &mut MdStreamer = unsafe { &mut *streamer_cell.get() };
        match provider2.as_str() {
            "ollama" => {
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(text) = j["message"]["content"]
                        .as_str()
                        .or_else(|| j["response"].as_str())
                    {
                        received_content.set(true);
                        s.print(text);
                    }
                }
            }
            "syspilot" => {
                let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(text) = j["choices"][0]["delta"]["content"]
                        .as_str()
                        .or_else(|| j["choices"][0]["message"]["content"].as_str())
                    {
                        received_content.set(true);
                        s.print(text);
                    }
                }
            }
            "gemini" => {
                if let Some(data) = line.strip_prefix("data:").map(str::trim) {
                    if let Ok(j) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(text) =
                            j["candidates"][0]["content"]["parts"][0]["text"].as_str()
                        {
                            received_content.set(true);
                            s.print(text);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let s: &mut MdStreamer = unsafe { &mut *streamer_cell.get() };
    s.flush();
    println!();

    if !ok || code != 0 {
        eprintln!("❌ AI provider request failed with exit code: {}", code);
        return false;
    }
    if !received_content.get() {
        eprintln!("❌ AI provider returned no usable streamed content.");
        return false;
    }

    true
}

pub fn pull_ollama_model(config: &Config, model_name: &str) -> bool {
    let url = format!("{}/api/pull", config.ollama_url);
    let payload = serde_json::json!({ "name": model_name, "stream": true }).to_string();

    println!("⬇️  Pulling model '{}'...", model_name);

    let curl_args: Vec<String> = vec![
        "curl".into(),
        "-s".into(),
        "-N".into(),
        "-X".into(),
        "POST".into(),
        "-H".into(),
        "Content-Type: application/json".into(),
        "-d".into(),
        "@-".into(),
        url,
    ];

    let mut line_buf = String::new();

    let (ok, code) = utils::run_command_secure_stream(&curl_args, payload, |chunk: &str| {
        line_buf.push_str(chunk);
        while let Some(nl) = line_buf.find('\n') {
            let line = line_buf[..nl].trim().to_string();
            line_buf.drain(..=nl);
            if line.is_empty() {
                continue;
            }
            if let Ok(j) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(status) = j["status"].as_str() {
                    if j["completed"].is_number() && j["total"].is_number() {
                        let done = j["completed"].as_f64().unwrap_or(0.0);
                        let total = j["total"].as_f64().unwrap_or(1.0);
                        let pct = if total > 0.0 {
                            done / total * 100.0
                        } else {
                            0.0
                        };
                        print!("\r{:<40} [{:.2}%]", status, pct);
                    } else {
                        print!("\r{:<50}", status);
                    }
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                }
            }
        }
    });

    println!();
    if !ok || code != 0 {
        eprintln!("❌ curl failed with exit code: {}", code);
        return false;
    }
    true
}
