/// Integration tests for src/utils.rs
/// Covers: string helpers, file I/O helpers, command runner.
use std::fs;

// Pull the crate in as a library dependency
use syspilot::utils;

// ── String helpers ────────────────────────────────────────────────────────────

#[test]
fn trim_strips_whitespace() {
    assert_eq!(utils::trim("  hello  "), "hello");
    assert_eq!(utils::trim("\n\ttab\n"), "tab");
    assert_eq!(utils::trim(""), "");
    assert_eq!(utils::trim("no_space"), "no_space");
}

#[test]
fn trim_owned_returns_string() {
    assert_eq!(utils::trim_owned("  hi  ".to_string()), "hi");
}

#[test]
fn to_lower_converts_ascii() {
    assert_eq!(utils::to_lower("GEMINI"), "gemini");
    assert_eq!(utils::to_lower("MixedCase123"), "mixedcase123");
    assert_eq!(utils::to_lower("already"), "already");
}

#[test]
fn starts_with_and_ends_with() {
    assert!(utils::starts_with("data: hello", "data: "));
    assert!(!utils::starts_with("data: hello", "foo"));
    assert!(utils::ends_with("file.rs", ".rs"));
    assert!(!utils::ends_with("file.rs", ".py"));
}

#[test]
fn split_char_basic() {
    let parts = utils::split_char("a|b|c", '|');
    assert_eq!(parts, vec!["a", "b", "c"]);
}

#[test]
fn split_char_no_delimiter() {
    let parts = utils::split_char("abc", '|');
    assert_eq!(parts, vec!["abc"]);
}

#[test]
fn split_char_empty_string() {
    let parts = utils::split_char("", ',');
    assert_eq!(parts, vec![""]);
}

#[test]
fn split_str_multi_char_delimiter() {
    let parts = utils::split_str("a | b | c", " | ");
    assert_eq!(parts, vec!["a", "b", "c"]);
}

#[test]
fn replace_all_replaces_every_occurrence() {
    assert_eq!(utils::replace_all("aabbaa", "aa", "X"), "XbbX");
    assert_eq!(utils::replace_all("hello", "z", "q"), "hello");
}

// ── File helpers ──────────────────────────────────────────────────────────────

#[test]
fn file_exists_works() {
    // /proc/version always exists on Linux
    assert!(utils::file_exists("/proc/version"));
    assert!(!utils::file_exists("/this_path_does_not_exist_xyz_123"));
}

#[test]
fn is_directory_works() {
    assert!(utils::is_directory("/tmp"));
    assert!(!utils::is_directory("/proc/version"));
}

#[test]
fn write_read_delete_roundtrip() {
    let path = "/tmp/syspilot_test_roundtrip.txt";
    let content = "hello syspilot\nline2";
    utils::write_file_content(path, content).expect("write failed");
    assert!(utils::file_exists(path));
    let read_back = utils::read_file_content(path).expect("read failed");
    assert_eq!(read_back, content);
    assert!(utils::delete_file(path));
    assert!(!utils::file_exists(path));
}

#[test]
fn get_file_size_matches_actual() {
    let path = "/tmp/syspilot_test_size.txt";
    let content = "12345"; // 5 bytes
    utils::write_file_content(path, content).unwrap();
    assert_eq!(utils::get_file_size(path), 5);
    utils::delete_file(path);
}

#[test]
fn get_last_modified_time_nonzero_for_existing_file() {
    let path = "/tmp/syspilot_test_mtime.txt";
    utils::write_file_content(path, "x").unwrap();
    let t = utils::get_last_modified_time(path);
    assert!(t > 0, "mtime should be > 0 for a freshly written file");
    utils::delete_file(path);
}

#[test]
fn get_last_modified_time_zero_for_missing_file() {
    assert_eq!(utils::get_last_modified_time("/no/such/file/xyz"), 0);
}

#[test]
fn create_directory_recursive_creates_nested() {
    let path = "/tmp/syspilot_test_dir/a/b/c";
    assert!(utils::create_directory_recursive(path));
    assert!(utils::is_directory(path));
    fs::remove_dir_all("/tmp/syspilot_test_dir").ok();
}

#[test]
fn list_directory_finds_rs_files() {
    // Use the src directory of this project — must contain .rs files
    let src = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let files = utils::list_directory(src, true);
    assert!(
        !files.is_empty(),
        "should find at least one .rs file in src/"
    );
    assert!(
        files.iter().any(|f| f.ends_with(".rs")),
        "at least one file should be .rs"
    );
}

#[test]
fn list_directory_non_recursive_no_subdirs() {
    let src = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let files = utils::list_directory(src, false);
    // Non-recursive: no file should be deeper than one level under src/
    for f in &files {
        let rel = f.strip_prefix(src).unwrap_or(f);
        assert!(
            !rel.trim_start_matches('/').contains('/'),
            "non-recursive list returned a nested path: {}",
            f
        );
    }
}

// ── Process / Shell helpers ───────────────────────────────────────────────────

#[test]
fn run_command_output_captures_stdout() {
    let (out, code) = utils::run_command_output("echo hello");
    assert_eq!(code, 0);
    assert!(out.contains("hello"));
}

#[test]
fn run_command_output_returns_nonzero_on_failure() {
    let (_, code) = utils::run_command_output("exit 42");
    assert_eq!(code, 42);
}

#[test]
fn run_command_secure_echo() {
    let args: Vec<String> = vec!["echo".into(), "secure_test".into()];
    let (out, code) = utils::run_command_secure(&args, "");
    assert_eq!(code, 0);
    assert!(out.contains("secure_test"));
}

#[test]
fn run_command_secure_reads_stdin() {
    // `cat` with no args reads stdin and writes it to stdout
    let args: Vec<String> = vec!["cat".into()];
    let (out, code) = utils::run_command_secure(&args, "stdin_payload");
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "stdin_payload");
}

#[test]
fn run_command_secure_stream_collects_chunks() {
    let args: Vec<String> = vec!["printf".into(), "chunk1\nchunk2\nchunk3\n".into()];
    let mut collected = String::new();
    let (ok, code) = utils::run_command_secure_stream(&args, String::new(), |chunk| {
        collected.push_str(chunk);
    });
    assert!(ok);
    assert_eq!(code, 0);
    assert!(collected.contains("chunk1"));
    assert!(collected.contains("chunk3"));
}

#[test]
fn run_command_secure_empty_args_returns_error() {
    let (out, code) = utils::run_command_secure(&[], "");
    assert_eq!(code, -1);
    assert!(out.is_empty());
}
