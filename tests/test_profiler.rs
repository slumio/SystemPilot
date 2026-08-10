/// Tests for src/profiler.rs — stack reader, profile serialization.
use syspilot::profiler::{self, ProfileReport, StackTrace};

// ── profile_process — current process ────────────────────────────────────────

#[test]
fn profile_current_process_no_perf_returns_report() {
    let pid = std::process::id() as i32;
    // run_perf=false: no perf binary invocation, just stack reading
    let report = profiler::profile_process(pid, false);
    // pid must be reachable → struct should be returned (may have empty stacks
    // if CAP_SYS_ADMIN is absent, which is expected in CI)
    assert!(!report.perf_available);
    // top_symbols must be empty when perf wasn't run
    assert!(report.top_symbols.is_empty());
}

#[test]
fn profile_missing_pid_returns_empty_report() {
    let report = profiler::profile_process(99_999_999, false);
    assert!(report.active_stacks.is_empty());
    assert!(report.top_symbols.is_empty());
    assert!(report.call_graph_summary.is_empty());
}

// ── serialize_profile_to_json ─────────────────────────────────────────────────

#[test]
fn serialized_profile_is_valid_json() {
    let report = ProfileReport {
        perf_available: true,
        top_symbols: vec![
            ("my_function".to_string(), 42.5),
            ("other_fn".to_string(), 10.0),
        ],
        call_graph_summary: "# Overhead  Command\n  42.50%  myapp\n".to_string(),
        active_stacks: vec![StackTrace {
            tid: 1234,
            frames: vec![
                "pipe_read+0x10".to_string(),
                "do_syscall_64+0x30".to_string(),
            ],
        }],
    };

    let json_str = profiler::serialize_profile_to_json(&report);
    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("serialized profile must be valid JSON");

    assert!(parsed["perf_available"].as_bool().unwrap());
    assert!(parsed["top_symbols"].is_array());
    assert!(parsed["active_stacks"].is_array());
    assert!(parsed["call_graph_summary"].is_string());
}

#[test]
fn serialized_profile_top_symbols_structure() {
    let report = ProfileReport {
        perf_available: false,
        top_symbols: vec![("foo".to_string(), 55.0), ("bar".to_string(), 22.5)],
        call_graph_summary: String::new(),
        active_stacks: vec![],
    };

    let json_str = profiler::serialize_profile_to_json(&report);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let syms = parsed["top_symbols"].as_array().unwrap();
    assert_eq!(syms.len(), 2);
    assert_eq!(syms[0]["symbol"].as_str().unwrap(), "foo");
    assert!((syms[0]["overhead_percent"].as_f64().unwrap() - 55.0).abs() < 1e-9);
    assert_eq!(syms[1]["symbol"].as_str().unwrap(), "bar");
}

#[test]
fn serialized_profile_active_stacks_structure() {
    let report = ProfileReport {
        perf_available: false,
        top_symbols: vec![],
        call_graph_summary: String::new(),
        active_stacks: vec![
            StackTrace {
                tid: 42,
                frames: vec!["frame_a".to_string(), "frame_b".to_string()],
            },
            StackTrace {
                tid: 43,
                frames: vec!["frame_c".to_string()],
            },
        ],
    };

    let json_str = profiler::serialize_profile_to_json(&report);
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let stacks = parsed["active_stacks"].as_array().unwrap();
    assert_eq!(stacks.len(), 2);
    assert_eq!(stacks[0]["tid"].as_i64().unwrap(), 42);
    assert_eq!(stacks[0]["frames"].as_array().unwrap().len(), 2);
    assert_eq!(stacks[1]["tid"].as_i64().unwrap(), 43);
    assert_eq!(stacks[1]["frames"][0].as_str().unwrap(), "frame_c");
}

#[test]
fn serialized_empty_profile_is_valid_json() {
    let report = ProfileReport::default();
    let json_str = profiler::serialize_profile_to_json(&report);
    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("empty profile must produce valid JSON");
    assert!(!parsed["perf_available"].as_bool().unwrap());
    assert_eq!(parsed["top_symbols"].as_array().unwrap().len(), 0);
    assert_eq!(parsed["active_stacks"].as_array().unwrap().len(), 0);
}

// ── StackTrace struct ─────────────────────────────────────────────────────────

#[test]
fn stack_trace_fields_accessible() {
    let st = StackTrace {
        tid: 9999,
        frames: vec!["a".to_string(), "b".to_string(), "c".to_string()],
    };
    assert_eq!(st.tid, 9999);
    assert_eq!(st.frames.len(), 3);
    assert_eq!(st.frames[0], "a");
}
