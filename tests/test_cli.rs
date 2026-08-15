use clap::Parser;
use syspilot::cli::Cli;

#[test]
fn existing_commands_and_global_json_positions_parse() {
    let commands = [
        vec!["syspilot", "status"],
        vec!["syspilot", "--json", "doctor"],
        vec!["syspilot", "events", "--json"],
        vec!["syspilot", "setup", "--line"],
        vec!["syspilot", "explain", "--pid", "123", "--causal"],
        vec!["syspilot", "ask", "why is load high?", "--no-index"],
        vec!["syspilot", "config", "telemetry"],
        vec!["syspilot", "config", "set-key", "gemini"],
        vec!["syspilot", "cases", "export", "case-1", "/tmp/case.json"],
        vec!["syspilot", "support", "bundle", "create"],
    ];
    for command in commands {
        Cli::try_parse_from(&command)
            .unwrap_or_else(|error| panic!("failed to parse {command:?}: {error}"));
    }
}

#[test]
fn invalid_and_ambiguous_usage_is_rejected() {
    assert!(Cli::try_parse_from(["syspilot", "setup", "--tui", "--line"]).is_err());
    assert!(Cli::try_parse_from(["syspilot", "explain", "--causal"]).is_err());
    assert!(Cli::try_parse_from(["syspilot", "unknown-command"]).is_err());
    assert!(Cli::try_parse_from(["syspilot", "install", "--force"]).is_err());
    assert!(Cli::try_parse_from(["syspilot", "config", "set-key", "gemini", "secret"]).is_err());
}
