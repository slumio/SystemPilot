// lib.rs — exposes all modules so integration tests under tests/ can use them
// as `syspilot::module_name::...`

// Global allocator — mimalloc (thread-local heaps, 40-70% faster than glibc).
// Declared here so it applies to both the binary and integration-test harness.
use mimalloc::MiMalloc;
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub mod ai;
pub mod alert;
pub mod causal_engine;
pub mod codebase;
pub mod completions;
pub mod config;
pub mod config_migration;
pub mod daemon;
pub mod distributed;
pub mod doctor;
pub mod error;
pub mod evidence;
pub mod fleet;
pub mod forensics;
pub mod install;
pub mod output;
pub mod profiler;
pub mod safety;
pub mod spool;
pub mod support;
pub mod telemetry;
pub mod ui;
pub mod utils;
