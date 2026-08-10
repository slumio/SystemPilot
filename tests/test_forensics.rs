use syspilot::forensics::{
    normalize_raw_event, CapabilityState, Cause, Confidence, Correlator, CorrelatorConfig,
    IngestOutcome, KernelCapabilities, KernelEvent, OomKind, ProcessIdentity,
};

fn identity(pid: u32, started: u64) -> ProcessIdentity {
    ProcessIdentity {
        tgid: pid,
        start_boottime_ns: started,
    }
}
fn correlator() -> Correlator {
    Correlator::new(CorrelatorConfig {
        max_processes: 8,
        max_events_per_process: 8,
    })
}

#[test]
fn memcg_oom_requires_direct_victim_evidence() {
    let mut c = correlator();
    let p = identity(1200, 11);
    assert_eq!(
        c.ingest(KernelEvent::OomVictim {
            timestamp_ns: 100,
            victim: p,
            cgroup_id: 88,
            kind: OomKind::Memcg
        }),
        IngestOutcome::Accepted
    );
    let report = match c.ingest(KernelEvent::ProcessExit {
        timestamp_ns: 101,
        identity: p,
        pid: 1200,
        ppid: 1,
        cgroup_id: 88,
        exit_code: 9,
        is_group_leader: true,
    }) {
        IngestOutcome::Death(report) => report,
        other => panic!("expected report, got {other:?}"),
    };
    assert_eq!(report.cause, Cause::CgroupOom { cgroup_id: 88 });
    assert_eq!(report.confidence, Confidence::Confirmed);
}

#[test]
fn sigkill_without_corroboration_is_not_called_oom() {
    let mut c = correlator();
    let p = identity(1200, 12);
    let report = match c.ingest(KernelEvent::ProcessExit {
        timestamp_ns: 101,
        identity: p,
        pid: 1200,
        ppid: 1,
        cgroup_id: 88,
        exit_code: 9,
        is_group_leader: true,
    }) {
        IngestOutcome::Death(report) => report,
        other => panic!("expected report, got {other:?}"),
    };
    assert!(matches!(report.cause, Cause::FatalSignal { signal: 9, .. }));
}

#[test]
fn pid_reuse_does_not_join_old_evidence() {
    let mut c = correlator();
    let old = identity(1200, 1);
    let new = identity(1200, 2);
    let _ = c.ingest(KernelEvent::OomVictim {
        timestamp_ns: 100,
        victim: old,
        cgroup_id: 88,
        kind: OomKind::Memcg,
    });
    let report = match c.ingest(KernelEvent::ProcessExit {
        timestamp_ns: 101,
        identity: new,
        pid: 1200,
        ppid: 1,
        cgroup_id: 88,
        exit_code: 9,
        is_group_leader: true,
    }) {
        IngestOutcome::Death(report) => report,
        other => panic!("expected report, got {other:?}"),
    };
    assert!(matches!(report.cause, Cause::FatalSignal { signal: 9, .. }));
}

#[test]
fn ordinary_exit_is_confirmed_without_signal_inference() {
    let mut c = correlator();
    let p = identity(1200, 22);
    let report = match c.ingest(KernelEvent::ProcessExit {
        timestamp_ns: 101,
        identity: p,
        pid: 1200,
        ppid: 1,
        cgroup_id: 0,
        exit_code: 42 << 8,
        is_group_leader: true,
    }) {
        IngestOutcome::Death(report) => report,
        other => panic!("expected report, got {other:?}"),
    };
    assert_eq!(report.cause, Cause::NormalExit { status: 42 });
    assert_eq!(report.confidence, Confidence::Confirmed);
}

#[test]
fn stale_oom_evidence_is_not_treated_as_causal() {
    let mut c = correlator();
    let p = identity(1200, 23);
    let _ = c.ingest(KernelEvent::OomVictim {
        timestamp_ns: 1,
        victim: p,
        cgroup_id: 88,
        kind: OomKind::Memcg,
    });
    let report = match c.ingest(KernelEvent::ProcessExit {
        timestamp_ns: 5_000_000_002,
        identity: p,
        pid: 1200,
        ppid: 1,
        cgroup_id: 88,
        exit_code: 9,
        is_group_leader: true,
    }) {
        IngestOutcome::Death(report) => report,
        other => panic!("expected report, got {other:?}"),
    };
    assert!(matches!(report.cause, Cause::FatalSignal { signal: 9, .. }));
}

#[test]
fn abi_oom_record_normalizes_without_losing_identity() {
    let raw = syspilot_abi::RawEvent::OomVictim(syspilot_abi::RawOomVictim {
        header: syspilot_abi::RawEventHeader {
            abi_version: syspilot_abi::ABI_VERSION,
            kind: syspilot_abi::EventKind::OomVictim as u16,
            size: syspilot_abi::RAW_OOM_VICTIM_SIZE as u32,
            timestamp_ns: 7,
            tgid: 1200,
            pid: 1201,
            start_boottime_ns: 99,
            cgroup_id: 88,
        },
        oom_kind: 2,
        reserved: 0,
    });
    assert_eq!(
        normalize_raw_event(raw),
        KernelEvent::OomVictim {
            timestamp_ns: 7,
            victim: identity(1200, 99),
            cgroup_id: 88,
            kind: OomKind::Memcg,
        }
    );
}

#[test]
fn capability_detection_degrades_each_probe_independently() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let events = temp.path().join("events");
    std::fs::create_dir_all(events.join("sched/sched_process_exit")).expect("sched tracepoint");
    std::fs::create_dir_all(events.join("oom/mark_victim")).expect("oom tracepoint");
    let btf = temp.path().join("vmlinux");
    std::fs::write(&btf, []).expect("btf marker");

    let capabilities = KernelCapabilities::detect_at(&events, &btf);
    assert_eq!(capabilities.btf, CapabilityState::Supported);
    assert_eq!(capabilities.process_exit, CapabilityState::Supported);
    assert_eq!(capabilities.oom_victim, CapabilityState::Supported);
    assert_eq!(capabilities.signal_origin, CapabilityState::Unsupported);
    assert!(capabilities.can_collect_deaths());
}
