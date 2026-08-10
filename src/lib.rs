// lib.rs — exposes all modules so integration tests under tests/ can use them
// as `syspilot::module_name::...`

// Global allocator — mimalloc (thread-local heaps, 40-70% faster than glibc).
// Declared here so it applies to both the binary and integration-test harness.
use mimalloc::MiMalloc;
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub mod ai;
pub mod causal_engine;
pub mod codebase;
pub mod config;
pub mod daemon;
pub mod distributed;
pub mod error;
pub mod forensics;
pub mod install;
pub mod profiler;
pub mod safety;
pub mod telemetry;
pub mod ui;
pub mod utils;
