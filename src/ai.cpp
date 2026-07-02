#include "ai.h"
#include "utils.h"
#include "nlohmann/json.hpp"
#include <iostream>
#include <fstream>
#include <cstdio>
#include <memory>
#include <array>
#include <chrono>
#include <unistd.h>
#include <vector>

using json = nlohmann::json;

namespace ai {

const std::string REASONING_SYSTEM_PROMPT = 
    "You are the SysPilot Operating System Reasoning Agent, an expert in kernel subsystems, hardware architecture, "
    "program analysis, and systems performance engineering.\n"
    "Your job is to analyze low-level operating system telemetry, thread scheduling, memory management, I/O operations, "
    "performance counters, and execution stacks, and explain the underlying causes of the observed behavior.\n"
    "Always seek to answer:\n"
    "1. What happened inside the machine?\n"
    "2. Why did it happen? What was the root cause?\n"
    "3. Which process, thread, function, or source code location triggered it?\n"
    "4. How did the effects propagate through the OS subsystems (scheduler, memory manager, VFS, block layer, network stack) and hardware?\n"
    "5. What was the impact on performance, latency, throughput, stability, and resource utilization?\n"
    "6. What specific recommendations (code modifications, configuration changes, architectural adjustments) will resolve the root cause?\n\n"
    "Connect low-level metrics to high-level software constructs. For instance, do not just say 'cache misses increased'; "
    "explain that a specific data structure or access pattern caused cache thrashing, stalling the pipeline. "
    "Do not just report 'high CPU'; trace it to lock contention or busy waiting.\n\n"
    "IMPORTANT TERMINAL FORMATTING RULES:\n"
    "1. When outputting mathematical formulas or equations, NEVER use raw LaTeX syntax like \\frac or \\sqrt.\n"
    "2. Instead, you MUST use user-friendly, plain-text ASCII math (e.g., (a + b) / c, sqrt(x)).\n"
    "3. Wrap code or math snippets in backticks (`).\n"
    "4. Keep explanations concise, clear, and highly technical.";

static std::string curl_config_quote(const std::string& value) {
    std::string out = "\"";
    for (char c : value) {
        if (c == '\\' || c == '"') out += '\\';
        if (c == '\n' || c == '\r') out += ' ';
        else out += c;
    }
    out += "\"";
    return out;
}

static std::string make_temp_path(const std::string& suffix) {
    std::string dir = utils::get_syspilot_directory() + "/tmp";
    utils::create_directory_private(dir);
    auto now = std::chrono::steady_clock::now().time_since_epoch().count();
    return dir + "/curl_" + std::to_string(getpid()) + "_" +
           std::to_string(now) + suffix;
}

static bool run_curl_configured_stream(
    const std::string& url, const std::vector<std::string>& headers,
    const std::string& payload, std::function<void(const std::string&)> cb,
    int* exit_code) {
    std::string payload_path = make_temp_path(".json");
    std::string config_path = make_temp_path(".conf");

    if (!utils::write_file_content_private(payload_path, payload)) {
        return false;
    }

    std::string config_text;
    config_text += "silent\n";
    config_text += "no-buffer\n";
    config_text += "request = \"POST\"\n";
    config_text += "url = " + curl_config_quote(url) + "\n";
    config_text += "header = \"Content-Type: application/json\"\n";
    for (const auto& header : headers) {
        config_text += "header = " + curl_config_quote(header) + "\n";
    }
    config_text += "data-binary = " + curl_config_quote("@" + payload_path) + "\n";

    if (!utils::write_file_content_private(config_path, config_text)) {
        utils::delete_file(payload_path);
        return false;
    }

    bool ok = utils::run_command_secure_stream(
        {"curl", "--config", config_path}, "", cb, exit_code);
    utils::delete_file(config_path);
    utils::delete_file(payload_path);
    return ok;
}

bool query_ai_stream(const Config& config, const std::string& prompt, MdStreamer& streamer) {
    std::string url = "";
    std::vector<std::string> headers;
    std::string payload = "";
    
    if (config.active_provider == "gemini") {
        if (config.gemini_api_key.empty()) {
            std::cerr << "❌ Gemini API key is not set. Run `syspilot config set-key gemini YOUR_KEY`" << std::endl;
            return false;
        }
        url = "https://generativelanguage.googleapis.com/v1beta/models/" + config.gemini_model + ":streamGenerateContent?alt=sse";
        headers.push_back("x-goog-api-key: " + config.gemini_api_key);
        
        json jreq;
        jreq["contents"] = json::array({ {{"parts", json::array({ {{"text", prompt}} })}} });
        jreq["systemInstruction"] = {{"parts", json::array({ {{"text", REASONING_SYSTEM_PROMPT}} })}};
        payload = jreq.dump();
    } 
    else if (config.active_provider == "ollama") {
        url = config.ollama_url + "/api/chat";
        
        json jreq;
        jreq["model"] = config.ollama_model;
        jreq["messages"] = json::array({
            {{"role", "system"}, {"content", REASONING_SYSTEM_PROMPT}},
            {{"role", "user"}, {"content", prompt}}
        });
        jreq["stream"] = true;
        payload = jreq.dump();
    }
    else if (config.active_provider == "syspilot") {
        if (config.syspilot_api_key.empty()) {
            std::cerr << "❌ SysPilot API key is not set. Run `syspilot config set-key syspilot YOUR_KEY`" << std::endl;
            return false;
        }
        url = "https://api.syspilot.dev/v1/chat/completions";
        headers.push_back("Authorization: Bearer " + config.syspilot_api_key);
        
        json jreq;
        jreq["model"] = config.syspilot_model;
        jreq["messages"] = json::array({
            {{"role", "system"}, {"content", REASONING_SYSTEM_PROMPT}},
            {{"role", "user"}, {"content", prompt}}
        });
        jreq["stream"] = true;
        payload = jreq.dump();
    }
    else {
        std::cerr << "❌ Unknown AI provider: " << config.active_provider << std::endl;
        return false;
    }
    
    std::string line_buffer = "";
    auto stream_cb = [&](const std::string& chunk) {
        line_buffer += chunk;
        size_t newline_pos = 0;
        while ((newline_pos = line_buffer.find('\n')) != std::string::npos) {
            std::string line = utils::trim(line_buffer.substr(0, newline_pos));
            line_buffer = line_buffer.substr(newline_pos + 1);
            
            if (line.empty()) continue;
            
            if (config.active_provider == "gemini") {
                if (utils::starts_with(line, "data: ")) {
                    std::string data = utils::trim(line.substr(6));
                    if (data.empty()) continue;
                    try {
                        json jdata = json::parse(data);
                        if (jdata.contains("candidates") && jdata["candidates"].is_array() && !jdata["candidates"].empty()) {
                            auto& cand = jdata["candidates"][0];
                            if (cand.contains("content") && cand["content"].contains("parts")) {
                                for (auto& part : cand["content"]["parts"]) {
                                    if (part.contains("text") && part["text"].is_string()) {
                                        streamer.print(part["text"].get<std::string>());
                                    }
                                }
                            }
                        }
                    } catch (...) {}
                }
            } 
            else if (config.active_provider == "ollama") {
                try {
                    json jdata = json::parse(line);
                    if (jdata.contains("message") && jdata["message"].contains("content") && jdata["message"]["content"].is_string()) {
                        streamer.print(jdata["message"]["content"].get<std::string>());
                    }
                } catch (...) {}
            }
            else if (config.active_provider == "syspilot") {
                if (utils::starts_with(line, "data: ")) {
                    std::string data = utils::trim(line.substr(6));
                    if (data == "[DONE]") break;
                    if (data.empty()) continue;
                    try {
                        json jdata = json::parse(data);
                        if (jdata.contains("choices") && jdata["choices"].is_array() && !jdata["choices"].empty()) {
                            auto& choice = jdata["choices"][0];
                            if (choice.contains("delta") && choice["delta"].contains("content") && choice["delta"]["content"].is_string()) {
                                streamer.print(choice["delta"]["content"].get<std::string>());
                            }
                        }
                    } catch (...) {}
                }
            }
        }
    };
    
    int exit_code = 0;
    bool success = run_curl_configured_stream(url, headers, payload, stream_cb, &exit_code);
    
    if (!success || exit_code != 0) {
        std::cerr << "❌ Secure curl invocation failed with exit code: " << exit_code << std::endl;
        return false;
    }
    
    if (!line_buffer.empty()) {
        std::string line = utils::trim(line_buffer);
        if (config.active_provider == "ollama") {
            try {
                json jdata = json::parse(line);
                if (jdata.contains("message") && jdata["message"].contains("content") && jdata["message"]["content"].is_string()) {
                    streamer.print(jdata["message"]["content"].get<std::string>());
                }
            } catch (...) {}
        }
    }
    
    streamer.flush();
    std::cout << std::endl;
    return true;
}

bool pull_ollama_model(const Config& config, const std::string& model_name) {
    json jreq;
    jreq["name"] = model_name;
    jreq["stream"] = true;
    std::string payload = jreq.dump();
    
    std::string url = config.ollama_url + "/api/pull";
    
    std::cout << "⬇️ Pulling model '" << model_name << "'..." << std::endl;
    
    std::vector<std::string> curl_args = {
        "curl", "-s", "-N", "-X", "POST",
        "-H", "Content-Type: application/json",
        "-d", "@-",
        url
    };
    
    std::string line_buffer = "";
    auto stream_cb = [&](const std::string& chunk) {
        line_buffer += chunk;
        size_t newline_pos = 0;
        while ((newline_pos = line_buffer.find('\n')) != std::string::npos) {
            std::string line = utils::trim(line_buffer.substr(0, newline_pos));
            line_buffer = line_buffer.substr(newline_pos + 1);
            
            if (line.empty()) continue;
            
            try {
                json jdata = json::parse(line);
                if (jdata.contains("status") && jdata["status"].is_string()) {
                    std::string status = jdata["status"].get<std::string>();
                    
                    if (jdata.contains("completed") && jdata.contains("total")) {
                        double completed = jdata["completed"].get<double>();
                        double total = jdata["total"].get<double>();
                        if (total > 0) {
                            double pct = (completed / total) * 100.0;
                            std::printf("\r%-40s [%.2f%%]", status.c_str(), pct);
                            std::fflush(stdout);
                        } else {
                            std::printf("\r%-50s", status.c_str());
                            std::fflush(stdout);
                        }
                    } else {
                        std::printf("\r%-50s", status.c_str());
                        std::fflush(stdout);
                    }
                }
            } catch (...) {}
        }
    };
    
    int exit_code = 0;
    bool success = utils::run_command_secure_stream(curl_args, payload, stream_cb, &exit_code);
    if (!success || exit_code != 0) {
        std::cerr << "❌ Secure curl invocation failed with exit code: " << exit_code << std::endl;
        return false;
    }
    
    std::cout << std::endl;
    return true;
}

} // namespace ai
