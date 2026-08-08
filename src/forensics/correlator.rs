use super::{Classifier, DeathReport, KernelEvent, ProcessIdentity};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy)]
pub struct CorrelatorConfig {
    pub max_processes: usize,
    pub max_events_per_process: usize,
}

impl Default for CorrelatorConfig {
    fn default() -> Self {
        Self {
            max_processes: 16_384,
            max_events_per_process: 16,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    Accepted,
    Death(DeathReport),
    Ignored,
}

/// Single-owner bounded timeline store. A collector sends here through a
/// bounded queue; ingestion never takes a global process-table lock.
pub struct Correlator {
    config: CorrelatorConfig,
    timelines: HashMap<ProcessIdentity, Timeline>,
    insertion_order: VecDeque<ProcessIdentity>,
    telemetry_loss_epoch: u64,
}

struct Timeline {
    events: VecDeque<KernelEvent>,
    loss_epoch_at_creation: u64,
}

impl Correlator {
    pub fn new(config: CorrelatorConfig) -> Self {
        assert!(
            config.max_processes > 0 && config.max_events_per_process > 0,
            "correlator bounds must be nonzero"
        );
        Self {
            config,
            timelines: HashMap::with_capacity(config.max_processes),
            insertion_order: VecDeque::with_capacity(config.max_processes),
            telemetry_loss_epoch: 0,
        }
    }

    pub fn ingest(&mut self, event: KernelEvent) -> IngestOutcome {
        if let KernelEvent::TelemetryLoss {
            dropped_records, ..
        } = event
        {
            if dropped_records != 0 {
                self.telemetry_loss_epoch = self.telemetry_loss_epoch.wrapping_add(1);
            }
            return IngestOutcome::Accepted;
        }
        let identity = match event {
            KernelEvent::ProcessExit { identity, .. } => identity,
            KernelEvent::SignalGenerated { target, .. } => target,
            KernelEvent::OomVictim { victim, .. } => victim,
            KernelEvent::TelemetryLoss { .. } => return IngestOutcome::Ignored,
        };
        self.ensure_capacity(identity);
        let is_death = matches!(
            event,
            KernelEvent::ProcessExit {
                is_group_leader: true,
                ..
            }
        );
        let loss_epoch = self.telemetry_loss_epoch;
        let max_events = self.config.max_events_per_process;
        let timeline = self.timelines.entry(identity).or_insert_with(|| {
            self.insertion_order.push_back(identity);
            Timeline {
                events: VecDeque::with_capacity(max_events),
                loss_epoch_at_creation: loss_epoch,
            }
        });
        if timeline.events.len() == max_events {
            timeline.events.pop_front();
        }
        timeline.events.push_back(event);
        if !is_death {
            return IngestOutcome::Accepted;
        }
        let report = Classifier::classify(
            identity,
            timeline.events.make_contiguous(),
            timeline.loss_epoch_at_creation != self.telemetry_loss_epoch,
        );
        self.timelines.remove(&identity);
        report
            .map(IngestOutcome::Death)
            .unwrap_or(IngestOutcome::Ignored)
    }

    fn ensure_capacity(&mut self, identity: ProcessIdentity) {
        if self.timelines.contains_key(&identity)
            || self.timelines.len() < self.config.max_processes
        {
            return;
        }
        while let Some(evicted) = self.insertion_order.pop_front() {
            if self.timelines.remove(&evicted).is_some() {
                break;
            }
        }
    }
}
