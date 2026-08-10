//! Passive process-death forensics.
//!
//! Kernel collection, bounded correlation state, and report classification are
//! deliberately independent so collection failures cannot change causality.

mod capabilities;
mod classifier;
mod correlator;
mod events;

pub use capabilities::{CapabilityState, KernelCapabilities};
pub use classifier::Classifier;
pub use correlator::{Correlator, CorrelatorConfig, IngestOutcome};
pub use events::{
    normalize_raw_event, Cause, Confidence, DeathReport, Evidence, ExitDisposition, KernelEvent,
    OomKind, ProcessIdentity, SignalOrigin,
};
