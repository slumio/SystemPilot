/// Commands that SysPilot will never execute on behalf of the user.
const DENYLIST: &[&str] = &[
    "rm", "sudo", "chmod", "chown", "kill", "pkill", "mv", "cp", "dd", "mkfs", "reboot", "shutdown",
];

/// Returns `true` if the command is safe to run, `false` if it is denied.
pub fn check_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return true;
    }

    // First whitespace-delimited token is the binary name
    let token = trimmed.split_whitespace().next().unwrap_or(trimmed);

    // Strip leading path, e.g. /bin/rm -> rm
    let base = token.rsplit('/').next().unwrap_or(token);
    let base_lower = base.to_lowercase();

    // Exact match against denylist
    if DENYLIST.contains(&base_lower.as_str()) {
        return false;
    }

    // Prefix match: catches mkfs.ext4, mkfs.vfat, etc.
    if DENYLIST
        .iter()
        .any(|&denied| base_lower.starts_with(&format!("{}.", denied)))
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_safe_commands() {
        assert!(check_command("ls -la"));
        assert!(check_command("cat /proc/cpuinfo"));
        assert!(check_command(""));
    }

    #[test]
    fn blocks_denylist() {
        assert!(!check_command("rm -rf /"));
        assert!(!check_command("/bin/rm -rf /"));
        assert!(!check_command("sudo whoami"));
    }
}
