/// Tests for src/safety.rs — command denylist enforcement.
use syspilot::safety;

// ── Allow-list ────────────────────────────────────────────────────────────────

#[test]
fn allows_empty_command() {
    assert!(safety::check_command(""));
    assert!(safety::check_command("   "));
}

#[test]
fn allows_common_read_only_commands() {
    for cmd in &[
        "ls -la",
        "cat /etc/hostname",
        "echo hello",
        "ps aux",
        "top -b -n 1",
    ] {
        assert!(
            safety::check_command(cmd),
            "expected '{}' to be allowed",
            cmd
        );
    }
}

#[test]
fn allows_build_tools() {
    for cmd in &[
        "cargo build",
        "make",
        "gcc -o out main.c",
        "python3 script.py",
    ] {
        assert!(
            safety::check_command(cmd),
            "expected '{}' to be allowed",
            cmd
        );
    }
}

// ── Deny-list ─────────────────────────────────────────────────────────────────

#[test]
fn blocks_rm() {
    assert!(!safety::check_command("rm -rf /"));
    assert!(!safety::check_command("rm file.txt"));
}

#[test]
fn blocks_sudo() {
    assert!(!safety::check_command("sudo rm -rf /"));
    assert!(!safety::check_command("sudo apt install vim"));
}

#[test]
fn blocks_chmod_chown() {
    assert!(!safety::check_command("chmod 777 /etc/passwd"));
    assert!(!safety::check_command("chown root:root /tmp/x"));
}

#[test]
fn blocks_kill_pkill() {
    assert!(!safety::check_command("kill -9 1"));
    assert!(!safety::check_command("pkill myapp"));
}

#[test]
fn blocks_destructive_disk_ops() {
    assert!(!safety::check_command("dd if=/dev/zero of=/dev/sda"));
    assert!(!safety::check_command("mkfs.ext4 /dev/sdb1"));
}

#[test]
fn blocks_system_control() {
    assert!(!safety::check_command("reboot"));
    assert!(!safety::check_command("shutdown -h now"));
}

// ── Path-prefix stripping ─────────────────────────────────────────────────────

#[test]
fn blocks_absolute_path_to_denied_binary() {
    assert!(!safety::check_command("/bin/rm -f /tmp/x"));
    assert!(!safety::check_command("/usr/bin/sudo whoami"));
    assert!(!safety::check_command("/sbin/reboot"));
}

// ── Case insensitivity ────────────────────────────────────────────────────────

#[test]
fn blocks_uppercase_variants() {
    // Binary names on Linux are lowercase, but our check is case-insensitive
    // to future-proof against user input normalisation mistakes.
    assert!(!safety::check_command("RM file.txt"));
    assert!(!safety::check_command("SUDO whoami"));
}
