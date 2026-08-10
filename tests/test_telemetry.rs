/// Tests for src/telemetry.rs — /proc parsing, find_pid_by_name, disk stats.
use syspilot::telemetry;

// ── find_pid_by_name ──────────────────────────────────────────────────────────

#[test]
fn find_pid_by_numeric_string() {
    // PID 1 always exists on Linux
    let pid = telemetry::find_pid_by_name("1");
    assert_eq!(pid, 1, "numeric PID '1' should resolve to 1");
}

#[test]
fn find_pid_zero_for_missing_process() {
    let pid = telemetry::find_pid_by_name("this_process_name_cannot_exist_xyz_9999");
    assert_eq!(pid, 0);
}

#[test]
fn find_pid_empty_name_returns_zero() {
    assert_eq!(telemetry::find_pid_by_name(""), 0);
}

// ── collect_process_telemetry ─────────────────────────────────────────────────

#[test]
fn telemetry_for_current_process() {
    let pid = std::process::id() as i32;
    let pt = telemetry::collect_process_telemetry(pid);

    assert_eq!(pt.pid, pid);
    // name must be non-empty
    assert!(!pt.name.is_empty(), "process name should not be empty");
    // state must be one of the standard /proc/stat single-character states
    assert!(
        ["R", "S", "D", "Z", "T", "t", "W", "X", "x", "K", "W", "P"].contains(&pt.state.as_str()),
        "unexpected state: {}",
        pt.state
    );
    // VSZ must be > 0 for a running process
    assert!(pt.vsize_bytes > 0, "vsize_bytes should be > 0");
    // RSS must be > 0
    assert!(pt.rss_bytes > 0, "rss_bytes should be > 0");
}

#[test]
fn telemetry_for_missing_pid_returns_zero_struct() {
    // PID 99999999 should not exist
    let pt = telemetry::collect_process_telemetry(99_999_999);
    assert_eq!(pt.pid, 99_999_999);
    assert_eq!(pt.vsize_bytes, 0);
    assert_eq!(pt.rss_bytes, 0);
    assert!(pt.name.is_empty());
}

#[test]
fn telemetry_ppid_of_current_is_nonzero() {
    let pid = std::process::id() as i32;
    let pt = telemetry::collect_process_telemetry(pid);
    // The test runner has a parent process
    assert!(pt.ppid > 0, "ppid should be > 0 for a normal process");
}

// ── collect_system_telemetry ──────────────────────────────────────────────────

#[test]
fn system_telemetry_fields_populated() {
    let st = telemetry::collect_system_telemetry();

    assert!(!st.load_avg.is_empty(), "load_avg should not be empty");
    assert!(st.mem_total_kb > 0, "mem_total_kb should be > 0");
    assert!(st.mem_free_kb <= st.mem_total_kb, "free <= total");
    assert!(st.mem_available_kb <= st.mem_total_kb);
    // At least one CPU counter should be non-zero
    let cpu_sum = st.cpu_user + st.cpu_nice + st.cpu_system + st.cpu_idle + st.cpu_iowait;
    assert!(cpu_sum > 0, "all CPU ticks cannot be zero on a live system");
}

#[test]
fn load_avg_has_three_components() {
    let st = telemetry::collect_system_telemetry();
    // /proc/loadavg: "0.10 0.15 0.12 1/302 12345"
    let parts: Vec<&str> = st.load_avg.split_whitespace().collect();
    assert!(
        parts.len() >= 3,
        "load_avg should have at least 3 fields, got: '{}'",
        st.load_avg
    );
    // First three parts must be parseable as floats
    for part in &parts[..3] {
        assert!(
            part.parse::<f64>().is_ok(),
            "load avg component '{}' is not a float",
            part
        );
    }
}

// ── serialize_telemetry_to_json ───────────────────────────────────────────────

#[test]
fn serialized_telemetry_is_valid_json() {
    let pid = std::process::id() as i32;
    let pt = telemetry::collect_process_telemetry(pid);
    let st = telemetry::collect_system_telemetry();
    let json_str = telemetry::serialize_telemetry_to_json(&pt, &st);

    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("serialized telemetry should be valid JSON");

    // Top-level keys
    assert!(parsed["process"].is_object());
    assert!(parsed["system"].is_object());

    // Process fields
    assert_eq!(parsed["process"]["pid"].as_i64().unwrap(), pid as i64);
    assert!(parsed["process"]["name"].is_string());
    assert!(parsed["process"]["state"].is_string());

    // System fields
    assert!(parsed["system"]["memory_total_kb"].as_u64().unwrap() > 0);
    assert!(parsed["system"]["cpu_ticks"].is_object());
}

#[test]
fn serialized_telemetry_environment_capped_at_30() {
    let pid = std::process::id() as i32;
    let pt = telemetry::collect_process_telemetry(pid);
    let st = telemetry::collect_system_telemetry();
    let json_str = telemetry::serialize_telemetry_to_json(&pt, &st);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let env_arr = parsed["process"]["environment"].as_array().unwrap();
    assert!(
        env_arr.len() <= 31, // 30 vars + optional truncation notice
        "environment array should be capped: got {} entries",
        env_arr.len()
    );
}

// ── get_open_resources ────────────────────────────────────────────────────────

#[test]
fn open_resources_for_current_process() {
    let pid = std::process::id() as i32;
    let resources = telemetry::get_open_resources(pid);
    // A running process must have at least stdin, stdout, stderr open
    assert!(
        resources.len() >= 3,
        "expected >= 3 open FDs for current process, got {}",
        resources.len()
    );
}

#[test]
fn open_resources_for_missing_pid_is_empty() {
    let resources = telemetry::get_open_resources(99_999_999);
    assert!(resources.is_empty());
}

// ── get_disk_stats ────────────────────────────────────────────────────────────

#[test]
fn disk_stats_returns_map() {
    let stats = telemetry::get_disk_stats();
    // On a machine with at least one disk, there must be entries
    // (even a container will have loop0 or sda or vda)
    // We just check the map is well-formed — keys end in known suffixes.
    for key in stats.keys() {
        assert!(
            key.ends_with("_reads")
                || key.ends_with("_read_bytes")
                || key.ends_with("_writes")
                || key.ends_with("_write_bytes")
                || key.ends_with("_io_time_ms"),
            "unexpected key format: {}",
            key
        );
    }
}
