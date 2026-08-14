use crate::distributed::DistributedTelemetryConfig;
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_provider")]
    pub active_provider: String,

    #[serde(default)]
    pub gemini_api_key: String,

    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,

    #[serde(default)]
    pub syspilot_api_key: String,

    #[serde(default = "default_syspilot_model")]
    pub syspilot_model: String,

    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,

    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,

    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,

    /// Provider used exclusively for vector embeddings. `active` preserves the
    /// legacy behaviour of using the chat/explanation provider.
    #[serde(default = "default_embedding_provider")]
    pub embedding_provider: String,

    #[serde(default = "default_chunk_strategy")]
    pub chunk_strategy: String,

    #[serde(default = "default_ai_request_timeout_seconds")]
    pub ai_request_timeout_seconds: u64,

    #[serde(default = "default_ai_connect_timeout_seconds")]
    pub ai_connect_timeout_seconds: u64,

    #[serde(default = "default_syspilot_url")]
    pub syspilot_url: String,

    #[serde(default)]
    pub distributed_telemetry: DistributedTelemetryConfig,
}

fn default_provider() -> String {
    "gemini".to_string()
}
fn default_gemini_model() -> String {
    "gemini-3.6-flash".to_string()
}
fn default_syspilot_model() -> String {
    "syspilot-1".to_string()
}
fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}
fn default_ollama_model() -> String {
    "llama3".to_string()
}
fn default_embedding_model() -> String {
    "text-embedding-004".to_string()
}
fn default_embedding_provider() -> String {
    "active".to_string()
}
fn default_chunk_strategy() -> String {
    "syntactic".to_string()
}
fn default_ai_request_timeout_seconds() -> u64 {
    120
}
fn default_ai_connect_timeout_seconds() -> u64 {
    15
}
fn default_syspilot_url() -> String {
    "https://api.syspilot.dev/v1/chat/completions".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            active_provider: default_provider(),
            gemini_api_key: String::new(),
            gemini_model: default_gemini_model(),
            syspilot_api_key: String::new(),
            syspilot_model: default_syspilot_model(),
            ollama_url: default_ollama_url(),
            ollama_model: default_ollama_model(),
            embedding_model: default_embedding_model(),
            embedding_provider: default_embedding_provider(),
            chunk_strategy: default_chunk_strategy(),
            ai_request_timeout_seconds: default_ai_request_timeout_seconds(),
            ai_connect_timeout_seconds: default_ai_connect_timeout_seconds(),
            syspilot_url: default_syspilot_url(),
            distributed_telemetry: DistributedTelemetryConfig::default(),
        }
    }
}

pub fn get_syspilot_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("SYSPILOT_HOME").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".syspilot")
}

/// Directory shared by the daemon and CLI for its Unix socket and heartbeat.
/// A packaged system daemon uses `/run/syspilot`; source and user installs get
/// a per-user directory under XDG_RUNTIME_DIR (or a UID-scoped /tmp fallback).
pub fn daemon_runtime_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("SYSPILOT_RUNTIME_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let system_dir = PathBuf::from("/run/syspilot");
    if system_dir.is_dir() {
        return system_dir;
    }
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(runtime_dir).join("syspilot");
    }
    PathBuf::from(format!("/tmp/syspilot-{}", unsafe { libc::geteuid() }))
}

pub fn daemon_socket_path() -> PathBuf {
    daemon_runtime_dir().join("syspilot.sock")
}

/// User daemons use the Linux abstract namespace, avoiding stale files and
/// filesystem races. Explicit/system runtime directories retain pathname
/// sockets so packaged deployments can enforce group permissions.
pub fn daemon_socket_is_abstract() -> bool {
    std::env::var_os("SYSPILOT_RUNTIME_DIR")
        .filter(|path| !path.is_empty())
        .is_none()
        && !PathBuf::from("/run/syspilot").is_dir()
}

pub fn daemon_socket_addr() -> io::Result<SocketAddr> {
    if daemon_socket_is_abstract() {
        SocketAddr::from_abstract_name(
            format!("syspilot-{}", unsafe { libc::geteuid() }).as_bytes(),
        )
    } else {
        SocketAddr::from_pathname(daemon_socket_path())
    }
}

pub fn daemon_socket_label() -> String {
    if daemon_socket_is_abstract() {
        format!("@syspilot-{}", unsafe { libc::geteuid() })
    } else {
        daemon_socket_path().display().to_string()
    }
}

pub fn connect_daemon() -> io::Result<UnixStream> {
    UnixStream::connect_addr(&daemon_socket_addr()?)
}

pub fn daemon_health_path() -> PathBuf {
    daemon_runtime_dir().join("health.json")
}

pub fn validate(cfg: &Config) -> AppResult<()> {
    cfg.distributed_telemetry.validate()?;
    if cfg.ai_request_timeout_seconds == 0 || cfg.ai_connect_timeout_seconds == 0 {
        return Err(AppError::Validation(
            "AI request and connect timeouts must be greater than zero".into(),
        ));
    }
    reqwest::Url::parse(&cfg.ollama_url)
        .map_err(|error| AppError::Validation(format!("invalid Ollama URL: {error}")))?;
    reqwest::Url::parse(&cfg.syspilot_url)
        .map_err(|error| AppError::Validation(format!("invalid SysPilot API URL: {error}")))?;
    Ok(())
}

pub fn load_checked() -> AppResult<Config> {
    let path = get_syspilot_dir().join("config.json");
    let mut cfg = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|error| AppError::io("could not read SysPilot configuration", error))?;
        serde_json::from_str(&content).map_err(|source| AppError::ConfigParse {
            path: path.clone(),
            source,
        })?
    } else {
        Config::default()
    };

    // Environment variable overrides
    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        if !key.is_empty() {
            cfg.gemini_api_key = key;
        }
    }
    if let Ok(key) = std::env::var("SYSPILOT_API_KEY") {
        if !key.is_empty() {
            cfg.syspilot_api_key = key;
        }
    }

    // Sanitize gemini model name
    if cfg.gemini_model.is_empty()
        || cfg.gemini_model == "gemini"
        || cfg.gemini_model == "gemini-2.0-flash"
        || (!cfg.gemini_model.contains('/') && !cfg.gemini_model.contains('-'))
    {
        cfg.gemini_model = default_gemini_model();
    }

    if !["active", "gemini", "ollama"].contains(&cfg.embedding_provider.as_str()) {
        return Err(AppError::Validation(format!(
            "invalid embedding provider '{}'; use active, gemini, or ollama",
            cfg.embedding_provider
        )));
    }

    validate(&cfg)?;

    Ok(cfg)
}

/// Backward-compatible convenience loader for interactive paths. New command
/// boundaries should prefer `load_checked` when a bad config must stop work.
pub fn load() -> Config {
    match load_checked() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("⚠️  {}. Using default configuration.", error);
            Config::default()
        }
    }
}

pub fn save(cfg: &Config) -> AppResult<()> {
    let dir = get_syspilot_dir();
    std::fs::create_dir_all(&dir).map_err(|error| {
        AppError::io("could not create SysPilot configuration directory", error)
    })?;
    let path = dir.join("config.json");
    let json = serde_json::to_string_pretty(cfg).map_err(|error| {
        AppError::Protocol(format!("could not encode SysPilot configuration: {error}"))
    })?;
    std::fs::write(&path, json)
        .map_err(|error| AppError::io("could not write SysPilot configuration", error))
}
