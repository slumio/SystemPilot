#![deny(unsafe_op_in_unsafe_fn)]
//! Kernel-event ingress and bounded handoff.
//!
//! One producer owns `push` and one consumer owns `pop`. This matches the
//! intended topology of a ring-buffer reader feeding one correlator thread.
//! Multiple producers require one instance per producer; they must not share a
//! ring because that would invalidate the SPSC memory-ordering invariant.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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

/// A fixed-capacity, allocation-free, single-producer/single-consumer ring.
///
/// Its indices are isolated on separate cache lines to prevent producer and
/// consumer traffic from false-sharing under sustained event rates.
pub struct SpscRing<T: Copy, const CAPACITY: usize> {
    producer: CacheLine<AtomicUsize>,
    consumer: CacheLine<AtomicUsize>,
    slots: [std::cell::UnsafeCell<std::mem::MaybeUninit<T>>; CAPACITY],
}

/// `UnsafeCell` only protects independent slots. Producer and consumer access
/// a slot exclusively according to acquire/release index ownership.
///
/// # Safety
/// Callers must maintain the SPSC contract: exactly one producer calls
/// `try_push`, and exactly one consumer calls `try_pop`. The public API cannot
/// encode this runtime topology, so the queue is the sole audited unsafe core.
unsafe impl<T: Copy + Send, const CAPACITY: usize> Send for SpscRing<T, CAPACITY> {}
/// See the type-level invariant above; only one producer and one consumer may
/// invoke the respective methods concurrently.
unsafe impl<T: Copy + Send, const CAPACITY: usize> Sync for SpscRing<T, CAPACITY> {}

#[repr(align(64))]
struct CacheLine<T>(T);

impl<T: Copy, const CAPACITY: usize> SpscRing<T, CAPACITY> {
    pub fn new() -> Self {
        assert!(CAPACITY.is_power_of_two() && CAPACITY >= 2);
        Self {
            producer: CacheLine(AtomicUsize::new(0)),
            consumer: CacheLine(AtomicUsize::new(0)),
            slots: std::array::from_fn(|_| {
                std::cell::UnsafeCell::new(std::mem::MaybeUninit::uninit())
            }),
        }
    }

    /// Producer-only operation. Returns the event when full, without blocking.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let head = self.producer.0.load(Ordering::Relaxed);
        let next = (head + 1) & (CAPACITY - 1);
        if next == self.consumer.0.load(Ordering::Acquire) {
            return Err(value);
        }
        // SAFETY: the producer exclusively owns `head` until publishing `next`
        // with Release. The consumer cannot observe this slot before that store.
        unsafe { (*self.slots[head].get()).write(value) };
        self.producer.0.store(next, Ordering::Release);
        Ok(())
    }

    /// Consumer-only operation. Never blocks or allocates.
    pub fn try_pop(&self) -> Option<T> {
        let tail = self.consumer.0.load(Ordering::Relaxed);
        if tail == self.producer.0.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: producer published this slot with Release, observed above by
        // Acquire. The consumer exclusively owns `tail` until it advances it.
        let value = unsafe { (*self.slots[tail].get()).assume_init_read() };
        self.consumer
            .0
            .store((tail + 1) & (CAPACITY - 1), Ordering::Release);
        Some(value)
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
