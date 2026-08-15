use serde_json::Value;
use std::process::Command;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_syspilot"))
}

#[test]
fn invalid_usage_and_version_have_stable_exit_behavior() {
    let invalid = binary()
        .args(["setup", "--tui", "--line"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("cannot be used with"));

    let version = binary().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("syspilot "));
}

#[test]
fn json_placement_and_mutation_output_are_secret_free() {
    let directory = tempfile::tempdir().unwrap();
    let mutation = binary()
        .env("SYSPILOT_HOME", directory.path())
        .args(["config", "set-key", "gemini", "golden-secret"])
        .output()
        .unwrap();
    assert!(mutation.status.success());
    assert!(!String::from_utf8_lossy(&mutation.stdout).contains("golden-secret"));
    let config = std::fs::read_to_string(directory.path().join("config.json")).unwrap();
    assert!(!config.contains("golden-secret"));

    let status = binary()
        .env("SYSPILOT_HOME", directory.path())
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert!(matches!(status.status.code(), Some(0 | 2)));
    let document: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(document["schema_version"], 1);
    assert!(!String::from_utf8_lossy(&status.stdout).contains("golden-secret"));
}

#[test]
fn restricted_terminal_is_explicit_and_never_mutates_setup_state() {
    let directory = tempfile::tempdir().unwrap();
    let result = binary()
        .env("SYSPILOT_HOME", directory.path())
        .env("TERM", "dumb")
        .args(["setup", "--tui"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("Full-screen setup is unavailable"));
    assert!(error.contains("syspilot setup --line"));
    assert!(!directory.path().join("config.json").exists());

    let check = binary()
        .env("TERM", "dumb")
        .args(["setup", "--check"])
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    let capability: Value = serde_json::from_slice(&check.stdout).unwrap();
    assert_eq!(capability["tui_available"], false);
    assert_eq!(capability["recovery_command"], "syspilot setup --line");
}
