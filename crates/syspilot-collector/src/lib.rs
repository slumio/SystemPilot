#![deny(unsafe_op_in_unsafe_fn)]
//! Kernel-event ingress and bounded handoff.
//!
//! The intended topology is one ring-buffer reader feeding one correlator
//! thread. The bounded queue remains memory-safe if that topology expands;
//! capacity and drop accounting stay explicit under contention.
/// Canonical fleet-control schema migration, embedded for contract tests and tooling.
pub const FLEET_SCHEMA_V1: &str = include_str!("../../../deploy/fleet/postgres/001_initial.sql");
/// Cloud reasoning, notification, heartbeat, and usage schema migration.
pub const FLEET_SCHEMA_V2: &str =
    include_str!("../../../deploy/fleet/postgres/002_cloud_workloads.sql");
/// Cloud email/webhook delivery leasing and bounded retry migration.
pub const FLEET_SCHEMA_V3: &str =
    include_str!("../../../deploy/fleet/postgres/003_notification_delivery.sql");
/// Content-bound replay integrity migration.
pub const FLEET_SCHEMA_V4: &str =
    include_str!("../../../deploy/fleet/postgres/004_replay_integrity.sql");
/// Worker lease recovery and delivery observability migration.
pub const FLEET_SCHEMA_V5: &str =
    include_str!("../../../deploy/fleet/postgres/005_worker_delivery_hardening.sql");

use std::sync::atomic::{AtomicU64, Ordering};

use syspilot_abi::RawEvent;

/// Telemetry importance determines which bounded queue receives a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPriority {
    Critical,
    High,
    Normal,
    Debug,
}

/// Snapshot-only collector metrics. Atomic reads are intentionally relaxed:
/// metrics are observability data, not correctness inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectorMetrics {
    pub received: u64,
    pub malformed: u64,
    pub critical_dropped: u64,
    pub auxiliary_dropped: u64,
}

struct Counters {
    received: AtomicU64,
    malformed: AtomicU64,
    critical_dropped: AtomicU64,
    auxiliary_dropped: AtomicU64,
}

impl Counters {
    const fn new() -> Self {
        Self {
            received: AtomicU64::new(0),
            malformed: AtomicU64::new(0),
            critical_dropped: AtomicU64::new(0),
            auxiliary_dropped: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> CollectorMetrics {
        CollectorMetrics {
            received: self.received.load(Ordering::Relaxed),
            malformed: self.malformed.load(Ordering::Relaxed),
            critical_dropped: self.critical_dropped.load(Ordering::Relaxed),
            auxiliary_dropped: self.auxiliary_dropped.load(Ordering::Relaxed),
        }
    }
}

/// A fixed-capacity, lock-free queue that performs no allocation after construction.
pub struct SpscRing<T: Copy, const CAPACITY: usize> {
    queue: crossbeam_queue::ArrayQueue<T>,
}

impl<T: Copy, const CAPACITY: usize> SpscRing<T, CAPACITY> {
    pub fn new() -> Self {
        assert!(CAPACITY.is_power_of_two() && CAPACITY >= 2);
        Self {
            queue: crossbeam_queue::ArrayQueue::new(CAPACITY - 1),
        }
    }

    /// Producer-only operation. Returns the event when full, without blocking.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        self.queue.push(value)
    }

    /// Consumer-only operation. Never blocks or allocates.
    pub fn try_pop(&self) -> Option<T> {
        self.queue.pop()
    }
}

impl<T: Copy, const CAPACITY: usize> Default for SpscRing<T, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Separates critical forensic records from lower-value auxiliary telemetry.
pub struct PriorityIngress<const CRITICAL_CAPACITY: usize, const AUXILIARY_CAPACITY: usize> {
    critical: SpscRing<RawEvent, CRITICAL_CAPACITY>,
    auxiliary: SpscRing<RawEvent, AUXILIARY_CAPACITY>,
    counters: Counters,
}

impl<const CRITICAL_CAPACITY: usize, const AUXILIARY_CAPACITY: usize>
    PriorityIngress<CRITICAL_CAPACITY, AUXILIARY_CAPACITY>
{
    pub fn new() -> Self {
        Self {
            critical: SpscRing::new(),
            auxiliary: SpscRing::new(),
            counters: Counters::new(),
        }
    }

    /// Decodes and queues one BPF-ring-buffer record. Invalid bytes never
    /// enter correlation state. No allocation, formatting, lock, or syscall.
    pub fn ingest_bytes(&self, bytes: &[u8], priority: EventPriority) -> bool {
        let Some(event) = RawEvent::decode(bytes) else {
            self.counters.malformed.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        self.push(event, priority)
    }

    /// Producer-only operation for an already validated ABI event.
    pub fn push(&self, event: RawEvent, priority: EventPriority) -> bool {
        self.counters.received.fetch_add(1, Ordering::Relaxed);
        match priority {
            EventPriority::Critical | EventPriority::High => {
                if self.critical.try_push(event).is_ok() {
                    true
                } else {
                    self.counters
                        .critical_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    false
                }
            }
            EventPriority::Normal | EventPriority::Debug => {
                if self.auxiliary.try_push(event).is_ok() {
                    true
                } else {
                    self.counters
                        .auxiliary_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    false
                }
            }
        }
    }

    /// Consumer-only operation. Critical records always win over auxiliary.
    pub fn try_pop(&self) -> Option<RawEvent> {
        self.critical.try_pop().or_else(|| self.auxiliary.try_pop())
    }

    pub fn metrics(&self) -> CollectorMetrics {
        self.counters.snapshot()
    }
}

impl<const CRITICAL_CAPACITY: usize, const AUXILIARY_CAPACITY: usize> Default
    for PriorityIngress<CRITICAL_CAPACITY, AUXILIARY_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syspilot_abi::{
        EventKind, RawEventHeader, RawOomVictim, RawTelemetryLoss, ABI_VERSION,
        RAW_OOM_VICTIM_SIZE, RAW_TELEMETRY_LOSS_SIZE,
    };

    fn loss_event(timestamp_ns: u64) -> RawEvent {
        RawEvent::TelemetryLoss(RawTelemetryLoss {
            header: RawEventHeader {
                abi_version: ABI_VERSION,
                kind: EventKind::TelemetryLoss as u16,
                size: RAW_TELEMETRY_LOSS_SIZE as u32,
                timestamp_ns,
                tgid: 0,
                pid: 0,
                start_boottime_ns: 0,
                cgroup_id: 0,
            },
            dropped_records: 1,
        })
    }

    #[test]
    fn ring_is_bounded_and_preserves_fifo_order() {
        let ring = SpscRing::<u32, 4>::new();
        assert_eq!(ring.try_pop(), None);
        assert!(ring.try_push(1).is_ok());
        assert!(ring.try_push(2).is_ok());
        assert!(ring.try_push(3).is_ok());
        assert_eq!(ring.try_push(4), Err(4));
        assert_eq!(ring.try_pop(), Some(1));
        assert_eq!(ring.try_pop(), Some(2));
        assert_eq!(ring.try_pop(), Some(3));
    }

    #[test]
    fn million_event_handoff_stays_within_throughput_budget() {
        let ring = SpscRing::<u64, 1024>::new();
        let started = std::time::Instant::now();
        for sequence in 0..1_000_000 {
            ring.try_push(sequence)
                .expect("interleaved queue is not full");
            assert_eq!(ring.try_pop(), Some(sequence));
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "one million event handoffs exceeded the 10 second debug-build budget"
        );
    }

    #[test]
    fn critical_records_preempt_auxiliary_records() {
        let ingress = PriorityIngress::<4, 4>::new();
        assert!(ingress.push(loss_event(1), EventPriority::Normal));
        assert!(ingress.push(loss_event(2), EventPriority::Critical));
        assert!(matches!(
            ingress.try_pop(),
            Some(RawEvent::TelemetryLoss(event)) if event.header.timestamp_ns == 2
        ));
    }

    #[test]
    fn malformed_records_are_counted_without_queueing() {
        let ingress = PriorityIngress::<4, 4>::new();
        assert!(!ingress.ingest_bytes(&[0; 3], EventPriority::Critical));
        assert_eq!(ingress.metrics().malformed, 1);
        assert_eq!(ingress.try_pop(), None);
    }

    #[test]
    fn full_auxiliary_queue_does_not_consume_critical_capacity() {
        let ingress = PriorityIngress::<4, 2>::new();
        assert!(ingress.push(loss_event(1), EventPriority::Debug));
        assert!(!ingress.push(loss_event(2), EventPriority::Debug));
        let critical = RawEvent::OomVictim(RawOomVictim {
            header: RawEventHeader {
                abi_version: ABI_VERSION,
                kind: EventKind::OomVictim as u16,
                size: RAW_OOM_VICTIM_SIZE as u32,
                timestamp_ns: 3,
                tgid: 12,
                pid: 12,
                start_boottime_ns: 1,
                cgroup_id: 2,
            },
            oom_kind: 2,
            reserved: 0,
        });
        assert!(ingress.push(critical, EventPriority::Critical));
        assert_eq!(ingress.metrics().auxiliary_dropped, 1);
        assert!(matches!(ingress.try_pop(), Some(RawEvent::OomVictim(_))));
    }
}

#[cfg(test)]
mod fleet_schema_tests {
    use super::{
        FLEET_SCHEMA_V1, FLEET_SCHEMA_V2, FLEET_SCHEMA_V3, FLEET_SCHEMA_V4, FLEET_SCHEMA_V5,
    };

    #[test]
    fn every_tenant_table_has_forced_row_level_security() {
        for table in [
            "principals",
            "nodes",
            "enrollment_credentials",
            "telemetry_messages",
            "node_sequence_state",
            "cases",
            "case_annotations",
            "alerts",
            "retention_policies",
            "deletion_requests",
            "audit_events",
        ] {
            assert!(
                FLEET_SCHEMA_V1.contains(&format!("'{}'", table)),
                "RLS list misses {table}"
            );
        }
        assert!(FLEET_SCHEMA_V1.contains("FORCE ROW LEVEL SECURITY"));
        assert!(FLEET_SCHEMA_V1.contains("tenant_id = syspilot_control.current_tenant_id()"));
    }

    #[test]
    fn ingestion_identity_and_sequence_are_tenant_scoped() {
        assert!(FLEET_SCHEMA_V1.contains("PRIMARY KEY (tenant_id, node_id, message_id)"));
        assert!(FLEET_SCHEMA_V1.contains("UNIQUE (tenant_id, node_id, sequence)"));
        assert!(FLEET_SCHEMA_V1.contains("gap_ranges int8multirange"));
    }

    #[test]
    fn secrets_and_audit_history_have_database_guards() {
        assert!(FLEET_SCHEMA_V1.contains("token_hash bytea NOT NULL"));
        assert!(!FLEET_SCHEMA_V1.contains("bearer_token"));
        assert!(FLEET_SCHEMA_V1.contains("audit_events_immutable"));
        assert!(FLEET_SCHEMA_V1.contains("REVOKE UPDATE, DELETE ON syspilot_control.audit_events"));
    }

    #[test]
    fn cloud_workloads_are_tenant_isolated_and_credentials_are_not_returned() {
        for table in [
            "node_heartbeats",
            "active_server_days",
            "reasoning_jobs",
            "reasoning_results",
            "notification_deliveries",
            "alert_destinations",
        ] {
            assert!(FLEET_SCHEMA_V2.contains(&format!("'{table}'")));
        }
        assert!(FLEET_SCHEMA_V2.contains("SECURITY DEFINER"));
        assert!(FLEET_SCHEMA_V2.contains("FOR UPDATE SKIP LOCKED"));
        assert!(FLEET_SCHEMA_V3.contains("lease_notification_delivery"));
        assert!(FLEET_SCHEMA_V4.contains("envelope_digest bytea"));
        assert!(FLEET_SCHEMA_V4.contains("octet_length(envelope_digest) = 32"));
        assert!(FLEET_SCHEMA_V5.contains("delivery.leased_until < clock_timestamp()"));
        assert!(FLEET_SCHEMA_V5.contains("worker_delivery_health"));
        assert!(FLEET_SCHEMA_V5.contains("oldest_queue_age_seconds"));
        assert!(FLEET_SCHEMA_V2.contains("RETURNS TABLE(tenant_id uuid, node_id text)"));
        assert!(!FLEET_SCHEMA_V2.contains("RETURNS TABLE(tenant_id uuid, node_id text, token"));
    }
}
