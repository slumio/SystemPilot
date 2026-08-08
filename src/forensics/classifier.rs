use super::{
    Cause, Confidence, DeathReport, Evidence, ExitDisposition, KernelEvent, OomKind,
    ProcessIdentity, SignalOrigin,
};

/// Pure causal rules. No collector or filesystem dependency exists here.
pub struct Classifier;

/// Older records are context, not causal proof of a later exit.
const MAX_CAUSAL_DISTANCE_NS: u64 = 5_000_000_000;

impl Classifier {
    pub(crate) fn classify(
        identity: ProcessIdentity,
        events: &[KernelEvent],
        evidence_incomplete: bool,
    ) -> Option<DeathReport> {
        let (timestamp_ns, cgroup_id, exit_code) =
            events.iter().rev().find_map(|event| match *event {
                KernelEvent::ProcessExit {
                    timestamp_ns,
                    identity: event_identity,
                    cgroup_id,
                    exit_code,
                    is_group_leader: true,
                    ..
                } if event_identity == identity => Some((timestamp_ns, cgroup_id, exit_code)),
                _ => None,
            })?;
        let disposition = ExitDisposition::from_exit_code(exit_code);
        let mut evidence = vec![Evidence::Exit { disposition }];
        let terminal_signal = match disposition {
            ExitDisposition::Signalled { signal, .. } => Some(signal as i32),
            ExitDisposition::Exited { .. } => None,
        };
        let generated_signal = terminal_signal.and_then(|expected| {
            events.iter().rev().find_map(|event| match *event {
                KernelEvent::SignalGenerated {
                    timestamp_ns: event_timestamp_ns,
                    target,
                    signal,
                    si_code,
                    sender_pid,
                    sender_uid,
                    sender_is_kernel,
                    ..
                } if target == identity
                    && signal == expected
                    && event_timestamp_ns <= timestamp_ns
                    && timestamp_ns - event_timestamp_ns <= MAX_CAUSAL_DISTANCE_NS =>
                {
                    Some((
                        signal,
                        SignalOrigin::from_parts(sender_pid, sender_uid, sender_is_kernel),
                        si_code,
                    ))
                }
                _ => None,
            })
        });
        if let Some((signal, origin, si_code)) = &generated_signal {
            evidence.push(Evidence::Signal {
                signal: *signal,
                origin: origin.clone(),
                si_code: *si_code,
            });
        }
        let oom = events.iter().rev().find_map(|event| match *event {
            KernelEvent::OomVictim {
                timestamp_ns: event_timestamp_ns,
                victim,
                cgroup_id,
                kind,
                ..
            } if victim == identity
                && event_timestamp_ns <= timestamp_ns
                && timestamp_ns - event_timestamp_ns <= MAX_CAUSAL_DISTANCE_NS =>
            {
                Some((kind, cgroup_id))
            }
            _ => None,
        });
        if let Some((kind, oom_cgroup_id)) = oom {
            evidence.push(Evidence::OomVictim {
                kind,
                cgroup_id: oom_cgroup_id,
            });
        }
        if evidence_incomplete {
            evidence.push(Evidence::TelemetryLoss { dropped_records: 0 });
        }

        let (cause, confidence) = match (terminal_signal, oom) {
            (Some(9), Some((OomKind::Memcg, oom_cgroup_id))) if oom_cgroup_id == cgroup_id => {
                (Cause::CgroupOom { cgroup_id }, Confidence::Confirmed)
            }
            (Some(9), Some((OomKind::Global, _))) => (Cause::GlobalOom, Confidence::High),
            (Some(9), Some((OomKind::Unknown, _))) => (Cause::Unknown, Confidence::Medium),
            (None, _) => match disposition {
                ExitDisposition::Exited { status } => {
                    (Cause::NormalExit { status }, Confidence::Confirmed)
                }
                ExitDisposition::Signalled { .. } => {
                    unreachable!("a signalled exit has a terminal signal")
                }
            },
            (Some(signal), _) => {
                let origin = generated_signal
                    .map(|(_, origin, _)| origin)
                    .unwrap_or(SignalOrigin::Unknown);
                let confidence = if matches!(origin, SignalOrigin::Unknown) {
                    Confidence::Medium
                } else {
                    Confidence::High
                };
                (
                    Cause::FatalSignal {
                        signal: signal as u8,
                        origin,
                    },
                    confidence,
                )
            }
        };
        Some(DeathReport {
            identity,
            timestamp_ns,
            cgroup_id,
            disposition,
            cause,
            confidence: if evidence_incomplete {
                confidence.min(Confidence::Medium)
            } else {
                confidence
            },
            evidence,
            evidence_incomplete,
        })
    }
}

impl SignalOrigin {
    fn from_parts(sender_pid: u32, sender_uid: u32, sender_is_kernel: bool) -> Self {
        if sender_is_kernel {
            Self::Kernel
        } else if sender_pid != 0 {
            Self::Process {
                pid: sender_pid,
                uid: sender_uid,
            }
        } else {
            Self::Unknown
        }
    }
}
