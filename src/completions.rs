const COMMANDS: &str = "setup install uninstall status doctor evidence cases alerts support daemon events monitor provider model pull index config ask explain completions version";

fn bash() -> String {
    format!(
        r#"_syspilot() {{
    local cur prev
    COMPREPLY=()
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    case "$prev" in
        provider) COMPREPLY=( $(compgen -W "gemini ollama syspilot" -- "$cur") ); return ;;
        completions) COMPREPLY=( $(compgen -W "bash zsh fish" -- "$cur") ); return ;;
        cases) COMPREPLY=( $(compgen -W "list show export delete" -- "$cur") ); return ;;
        alerts) COMPREPLY=( $(compgen -W "list acknowledge resolve suppress" -- "$cur") ); return ;;
        support) COMPREPLY=( $(compgen -W "bundle" -- "$cur") ); return ;;
        bundle) COMPREPLY=( $(compgen -W "create" -- "$cur") ); return ;;
        config) COMPREPLY=( $(compgen -W "telemetry alert set-key set-url set rollback" -- "$cur") ); return ;;
        telemetry) COMPREPLY=( $(compgen -W "enable disable show preview" -- "$cur") ); return ;;
        alert) COMPREPLY=( $(compgen -W "add remove list" -- "$cur") ); return ;;
        explain) COMPREPLY=( $(compgen -W "--pid --deep --ebpf --causal --number --no-index" -- "$cur") ); return ;;
        evidence) COMPREPLY=( $(compgen -W "--pid" -- "$cur") ); return ;;
    esac
    if [[ $COMP_CWORD -eq 1 ]]; then COMPREPLY=( $(compgen -W "{COMMANDS}" -- "$cur") ); fi
}}
complete -F _syspilot syspilot
"#
    )
}

fn zsh() -> String {
    format!(
        r#"#compdef syspilot
_syspilot() {{
  local -a commands
  commands=({})
  if (( CURRENT == 2 )); then _describe 'command' commands; return; fi
  case $words[2] in
    provider) _values 'provider' gemini ollama syspilot ;;
    completions) _values 'shell' bash zsh fish ;;
    cases) _values 'action' list show export delete ;;
    alerts) _values 'action' list acknowledge resolve suppress ;;
    support) _values 'action' bundle ;;
    bundle) _values 'action' create ;;
    config) _values 'action' telemetry alert set-key set-url set rollback ;;
    explain) _arguments '--pid[process PID or name]:process' '--deep' '--ebpf' '--causal' '--number[index]:index' '--no-index' ;;
    evidence) _arguments '--pid[process PID or name]:process' ;;
  esac
}}
_syspilot
"#,
        COMMANDS
            .split_whitespace()
            .map(|v| format!("'{v}:{v}'"))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn fish() -> String {
    let mut output = String::from("complete -c syspilot -f\n");
    for command in COMMANDS.split_whitespace() {
        output.push_str(&format!(
            "complete -c syspilot -n '__fish_use_subcommand' -a '{command}'\n"
        ));
    }
    output.push_str("complete -c syspilot -n '__fish_seen_subcommand_from provider' -a 'gemini ollama syspilot'\n");
    output.push_str(
        "complete -c syspilot -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish'\n",
    );
    output.push_str("complete -c syspilot -n '__fish_seen_subcommand_from cases' -a 'list show export delete'\n");
    output.push_str("complete -c syspilot -n '__fish_seen_subcommand_from alerts' -a 'list acknowledge resolve suppress'\n");
    output.push_str("complete -c syspilot -n '__fish_seen_subcommand_from support' -a 'bundle'\n");
    output.push_str("complete -c syspilot -n '__fish_seen_subcommand_from bundle' -a 'create'\n");
    output.push_str("complete -c syspilot -n '__fish_seen_subcommand_from config' -a 'telemetry alert set-key set-url set rollback'\n");
    output
}

pub fn generate(shell: &str) -> Result<String, String> {
    match shell {
        "bash" => Ok(bash()),
        "zsh" => Ok(zsh()),
        "fish" => Ok(fish()),
        _ => Err(format!(
            "unsupported shell '{shell}'; use bash, zsh, or fish"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_shell_contains_current_commands() {
        for shell in ["bash", "zsh", "fish"] {
            let value = generate(shell).unwrap();
            for command in [
                "doctor", "evidence", "cases", "alerts", "support", "config", "explain",
            ] {
                assert!(value.contains(command), "{shell} misses {command}");
            }
            assert!(value.contains("rollback"), "{shell} misses config rollback");
        }
    }
    #[test]
    fn rejects_unknown_shell() {
        assert!(generate("powershell").is_err());
    }
}
