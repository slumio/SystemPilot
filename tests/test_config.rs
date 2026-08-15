use std::{fs, sync::Mutex};
/// Tests for src/config.rs — defaults, serde round-trip, env-var overrides.
use syspilot::config::{self, Config};

// These tests temporarily change process-wide environment variables.  Cargo may
// run them concurrently, so serialize just the tests that touch that state.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn restore_env_var(name: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => std::env::set_var(name, value),
        None => std::env::remove_var(name),
    }
}

// ── Defaults ──────────────────────────────────────────────────────────────────

#[test]
fn default_config_fields() {
    let c = Config::default();
    assert_eq!(c.schema_version, config::config_schema_version());
    assert_eq!(c.active_provider, "gemini");
    assert_eq!(c.gemini_model, "gemini-3.6-flash");
    assert_eq!(c.ollama_url, "http://localhost:11434");
    assert_eq!(c.ollama_model, "llama3");
    assert_eq!(c.embedding_model, "text-embedding-004");
    assert_eq!(c.embedding_provider, "active");
    assert_eq!(c.chunk_strategy, "syntactic");
    assert!(c.gemini_api_key.is_empty());
    assert!(c.syspilot_api_key.is_empty());
}

// ── Serde round-trip ──────────────────────────────────────────────────────────

#[test]
fn config_serialises_and_deserialises() {
    let original = Config {
        schema_version: config::config_schema_version(),
        active_provider: "ollama".to_string(),
        gemini_api_key: "key123".to_string(),
        gemini_model: "gemini-3.6-flash".to_string(),
        syspilot_api_key: String::new(),
        syspilot_model: "syspilot-1".to_string(),
        ollama_url: "http://localhost:11434".to_string(),
        ollama_model: "llama3".to_string(),
        embedding_model: "text-embedding-004".to_string(),
        embedding_provider: "ollama".to_string(),
        chunk_strategy: "line".to_string(),
        ai_request_timeout_seconds: 120,
        ai_connect_timeout_seconds: 15,
        syspilot_url: "https://api.syspilot.dev/v1/chat/completions".to_string(),
        distributed_telemetry: Default::default(),
        fleet: Default::default(),
    };

    let json = serde_json::to_string_pretty(&original).unwrap();
    let restored: Config = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.active_provider, original.active_provider);
    assert_eq!(restored.gemini_api_key, original.gemini_api_key);
    assert_eq!(restored.chunk_strategy, original.chunk_strategy);
    assert_eq!(restored.ollama_model, original.ollama_model);
    assert_eq!(restored.embedding_provider, original.embedding_provider);
}

#[test]
fn partial_json_fills_missing_fields_with_defaults() {
    let json = r#"{"active_provider":"ollama"}"#;
    let c: Config = serde_json::from_str(json).unwrap();
    assert_eq!(c.active_provider, "ollama");
    // Fields not in JSON should revert to serde defaults
    assert_eq!(c.gemini_model, "gemini-3.6-flash");
    assert_eq!(c.ollama_url, "http://localhost:11434");
}

#[test]
fn empty_json_object_uses_all_defaults() {
    let c: Config = serde_json::from_str("{}").unwrap();
    assert_eq!(c.active_provider, "gemini");
    assert_eq!(c.chunk_strategy, "syntactic");
}

// ── Save / load round-trip ────────────────────────────────────────────────────

#[test]
fn save_and_load_roundtrip() {
    // Write to a temp dir by temporarily pointing get_syspilot_dir() via
    // the HOME env var.
    use std::env;

    let _env_lock = ENV_LOCK.lock().expect("environment lock poisoned");

    let tmp = tempfile::tempdir().expect("could not create temp dir");
    let orig_home = env::var_os("HOME");
    let orig_gemini_key = env::var_os("GEMINI_API_KEY");
    env::set_var("HOME", tmp.path());
    env::remove_var("GEMINI_API_KEY");

    let cfg = Config {
        active_provider: "syspilot".to_string(),
        gemini_api_key: "abc".to_string(),
        ollama_model: "mistral".to_string(),
        ..Config::default()
    };
    config::save(&cfg).expect("save failed");

    let loaded = config::load_checked().unwrap();
    assert_eq!(loaded.active_provider, "syspilot");
    assert_eq!(loaded.gemini_api_key, "abc");
    assert_eq!(loaded.ollama_model, "mistral");

    restore_env_var("HOME", orig_home);
    restore_env_var("GEMINI_API_KEY", orig_gemini_key);
}

// ── Environment variable overrides ───────────────────────────────────────────

#[test]
fn gemini_api_key_env_override() {
    use std::env;
    use tempfile::tempdir;

    let _env_lock = ENV_LOCK.lock().expect("environment lock poisoned");
    let tmp = tempdir().unwrap();
    let orig_home = env::var_os("HOME");
    let orig_gemini_key = env::var_os("GEMINI_API_KEY");
    env::set_var("HOME", tmp.path());
    env::set_var("GEMINI_API_KEY", "env_key_xyz");

    let cfg = config::load_checked().unwrap();
    assert_eq!(cfg.gemini_api_key, "env_key_xyz");

    restore_env_var("HOME", orig_home);
    restore_env_var("GEMINI_API_KEY", orig_gemini_key);
}

// ── Gemini model migration and validation ─────────────────────────────────────

#[test]
fn invalid_model_in_current_schema_fails_closed() {
    let config = Config {
        gemini_model: "gemini".into(),
        ..Config::default()
    };
    let error = config::validate(&config).unwrap_err().to_string();
    assert!(error.contains("invalid Gemini model"));
}

#[test]
fn valid_gemini_model_unchanged() {
    let config = Config {
        gemini_model: "gemini-1.5-pro".into(),
        ..Config::default()
    };
    config::validate(&config).unwrap();
    assert_eq!(config.gemini_model, "gemini-1.5-pro");
}

#[test]
fn retired_gemini_model_is_migrated_when_loaded() {
    use std::env;

    let _env_lock = ENV_LOCK.lock().expect("environment lock poisoned");
    let directory = tempfile::tempdir().unwrap();
    let old_home = env::var_os("SYSPILOT_HOME");
    let old_key = env::var_os("GEMINI_API_KEY");
    env::set_var("SYSPILOT_HOME", directory.path());
    env::remove_var("GEMINI_API_KEY");
    fs::write(
        directory.path().join("config.json"),
        r#"{"gemini_model":"gemini-2.0-flash"}"#,
    )
    .unwrap();

    let loaded = config::load_checked().unwrap();
    assert_eq!(loaded.gemini_model, "gemini-3.6-flash");
    let persisted: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(persisted["gemini_model"], "gemini-3.6-flash");
    assert_eq!(
        fs::read(directory.path().join("config.pre-v1.json")).unwrap(),
        br#"{"gemini_model":"gemini-2.0-flash"}"#
    );

    restore_env_var("SYSPILOT_HOME", old_home);
    restore_env_var("GEMINI_API_KEY", old_key);
}

#[test]
fn saved_configuration_and_directory_are_owner_only() {
    use std::env;
    use std::os::unix::fs::PermissionsExt;

    let _env_lock = ENV_LOCK.lock().expect("environment lock poisoned");
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("syspilot");
    let old_home = env::var_os("SYSPILOT_HOME");
    env::set_var("SYSPILOT_HOME", &home);

    config::save(&Config::default()).unwrap();
    assert_eq!(
        fs::metadata(home.join("config.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&home).unwrap().permissions().mode() & 0o777,
        0o700
    );

    restore_env_var("SYSPILOT_HOME", old_home);
}

#[test]
fn unversioned_config_is_backed_up_and_migrated_before_env_overrides() {
    let _env_lock = ENV_LOCK.lock().expect("environment lock poisoned");
    let directory = tempfile::tempdir().unwrap();
    let old_home = std::env::var_os("SYSPILOT_HOME");
    let old_key = std::env::var_os("GEMINI_API_KEY");
    std::env::set_var("SYSPILOT_HOME", directory.path());
    std::env::set_var("GEMINI_API_KEY", "runtime-only-key");
    let original = br#"{"active_provider":"ollama","gemini_api_key":"disk-key"}"#;
    fs::write(directory.path().join("config.json"), original).unwrap();

    let loaded = config::load_checked().unwrap();
    assert_eq!(loaded.schema_version, config::config_schema_version());
    assert_eq!(loaded.gemini_api_key, "runtime-only-key");
    let backup = directory.path().join("config.pre-v1.json");
    assert_eq!(fs::read(&backup).unwrap(), original);
    let migrated: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(migrated["schema_version"], config::config_schema_version());
    assert_eq!(migrated["gemini_api_key"], "disk-key");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(backup).unwrap().permissions().mode() & 0o777,
        0o600
    );

    restore_env_var("SYSPILOT_HOME", old_home);
    restore_env_var("GEMINI_API_KEY", old_key);
}

#[test]
fn newer_config_schema_fails_closed() {
    let _env_lock = ENV_LOCK.lock().expect("environment lock poisoned");
    let directory = tempfile::tempdir().unwrap();
    let old_home = std::env::var_os("SYSPILOT_HOME");
    std::env::set_var("SYSPILOT_HOME", directory.path());
    fs::write(
        directory.path().join("config.json"),
        r#"{"schema_version":999}"#,
    )
    .unwrap();
    let error = config::load_checked().unwrap_err().to_string();
    assert!(error.contains("newer than supported"));
    assert!(!directory.path().join("config.pre-v1.json").exists());
    restore_env_var("SYSPILOT_HOME", old_home);
}

#[test]
fn rollback_preserves_current_file_and_restores_pre_migration_bytes() {
    let _env_lock = ENV_LOCK.lock().expect("environment lock poisoned");
    let directory = tempfile::tempdir().unwrap();
    let old_home = std::env::var_os("SYSPILOT_HOME");
    let old_key = std::env::var_os("GEMINI_API_KEY");
    std::env::set_var("SYSPILOT_HOME", directory.path());
    std::env::remove_var("GEMINI_API_KEY");
    let original = br#"{"active_provider":"ollama"}"#;
    let active = directory.path().join("config.json");
    fs::write(&active, original).unwrap();
    let mut migrated = config::load_checked().unwrap();
    migrated.active_provider = "gemini".into();
    config::save(&migrated).unwrap();
    let current_before_rollback = fs::read(&active).unwrap();

    config::rollback_previous().unwrap();
    assert_eq!(fs::read(&active).unwrap(), original);
    assert_eq!(
        fs::read(directory.path().join("config.pre-rollback-v1.json")).unwrap(),
        current_before_rollback
    );

    restore_env_var("SYSPILOT_HOME", old_home);
    restore_env_var("GEMINI_API_KEY", old_key);
}
