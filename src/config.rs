use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
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

    #[serde(default = "default_chunk_strategy")]
    pub chunk_strategy: String,
}

fn default_provider() -> String {
    "gemini".to_string()
}
fn default_gemini_model() -> String {
    "gemini-2.0-flash".to_string()
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
fn default_chunk_strategy() -> String {
    "syntactic".to_string()
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
            chunk_strategy: default_chunk_strategy(),
        }
    }
}

pub fn get_syspilot_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".syspilot")
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
        || (!cfg.gemini_model.contains('/') && !cfg.gemini_model.contains('-'))
    {
        cfg.gemini_model = default_gemini_model();
    }

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
