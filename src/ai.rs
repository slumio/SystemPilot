use crate::config::Config;
use crate::ui::streamer::MdStreamer;
use crate::utils;
use std::io::{BufRead, BufReader, Read};

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
                "gemini-3.6-flash"
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

    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(
            config.ai_connect_timeout_seconds,
        ))
        .timeout(std::time::Duration::from_secs(
            config.ai_request_timeout_seconds,
        ))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("❌ Could not initialize the AI HTTP client: {error}");
            return false;
        }
    };
    let mut request = client.post(&url).header("Content-Type", "application/json");
    for header in headers {
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        request = request.header(name.trim(), value.trim());
    }
    let response = match request.body(payload).send() {
        Ok(response) => response,
        Err(error) => {
            let kind = if error.is_timeout() {
                "timed out"
            } else if error.is_connect() {
                "could not connect"
            } else {
                "failed"
            };
            eprintln!("❌ AI request {kind}: {error}");
            return false;
        }
    };
    let status = response.status();
    if !status.is_success() {
        let mut body = String::new();
        let mut response = response;
        let _ = response.read_to_string(&mut body);
        let message = provider_error_message(&body).unwrap_or_else(|| body.trim().to_string());
        eprintln!(
            "❌ AI provider returned HTTP {}: {}",
            status.as_u16(),
            if message.is_empty() {
                status.canonical_reason().unwrap_or("request failed")
            } else {
                &message
            }
        );
        if status.as_u16() == 404 && config.active_provider == "gemini" {
            eprintln!("   The configured Gemini model may not exist. Run: syspilot model gemini-3.6-flash");
        }
        return false;
    }

    let mut received_content = false;
    for line in BufReader::new(response).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("❌ AI response stream failed: {error}");
                return false;
            }
        };
        for text in streamed_text(&config.active_provider, &line) {
            received_content = true;
            streamer.print(&text);
        }
    }
    streamer.flush();
    println!();
    if !received_content {
        eprintln!("❌ AI provider returned no usable streamed content.");
        return false;
    }

    true
}

fn json_data(line: &str) -> &str {
    line.trim()
        .strip_prefix("data:")
        .map(str::trim)
        .unwrap_or_else(|| line.trim())
}

fn provider_error_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json_data(body)).ok()?;
    value["error"]["message"]
        .as_str()
        .or_else(|| value["message"].as_str())
        .map(ToOwned::to_owned)
}

fn streamed_text(provider: &str, line: &str) -> Vec<String> {
    let data = json_data(line);
    if data.is_empty() || data == "[DONE]" {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return Vec::new();
    };
    match provider {
        "gemini" => value["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|candidate| candidate["content"]["parts"].as_array())
            .flatten()
            .filter_map(|part| part["text"].as_str().map(ToOwned::to_owned))
            .collect(),
        "ollama" => value["message"]["content"]
            .as_str()
            .or_else(|| value["response"].as_str())
            .map(|s| vec![s.to_owned()])
            .unwrap_or_default(),
        "syspilot" => value["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|choice| {
                choice["delta"]["content"]
                    .as_str()
                    .or_else(|| choice["message"]["content"].as_str())
                    .map(ToOwned::to_owned)
            })
            .collect(),
        _ => Vec::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::{provider_error_message, streamed_text};

    #[test]
    fn parses_supported_stream_formats() {
        assert_eq!(
            streamed_text(
                "gemini",
                r#"data:{"candidates":[{"content":{"parts":[{"text":"gemini"}]}}]}"#,
            ),
            ["gemini"]
        );
        assert_eq!(
            streamed_text("ollama", r#"{"message":{"content":"ollama"}}"#),
            ["ollama"]
        );
        assert_eq!(
            streamed_text(
                "syspilot",
                r#"data: {"choices":[{"delta":{"content":"compatible"}}]}"#,
            ),
            ["compatible"]
        );
        assert!(streamed_text("syspilot", "data: [DONE]").is_empty());
    }

    #[test]
    fn extracts_provider_error_body() {
        let body = r#"{"error":{"code":404,"message":"model not found"}}"#;
        assert_eq!(
            provider_error_message(body).as_deref(),
            Some("model not found")
        );
    }
}
