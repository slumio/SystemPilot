# 🤖 SysPilot

> **High-performance Operating System Reasoning Agent** — real-time causal diagnostics, microsecond-latency telemetry, AI-powered root-cause analysis, and a zero-dependency terminal UI.

<div align="center">

![Language](https://img.shields.io/badge/language-Rust%202021-orange?style=for-the-badge&logo=rust)
![Platform](https://img.shields.io/badge/platform-Linux%20x86__64-orange?style=for-the-badge&logo=linux)
![License](https://img.shields.io/badge/license-MIT-green?style=for-the-badge)
![Build](https://img.shields.io/badge/build-passing-brightgreen?style=for-the-badge)

**Rust · Tokio · reqwest · DashMap · crossbeam-channel · mimalloc · Linux Netlink**

</div>

---

## What is SysPilot?

SysPilot is a **systems-level diagnostic suite** for Linux that combines three things in one binary:

1. **`syspilotd` Daemon** — A zero-polling background service that subscribes to the Linux kernel's Netlink Process Connector (`cn_proc`) to receive process lifecycle events (fork, exec, exit) in real time. It maintains a lock-free, in-memory process tree and exposes it over a UNIX domain socket at sub-100µs latency.

2. **CausalTrace Engine** — A directed multigraph reasoning engine that constructs a dependency graph of processes and system resources (files, sockets, block devices, pipes) from two time-stamped `/proc` snapshots. It performs a reverse-BFS traversal to trace observable symptoms (high I/O wait, zombie processes, mutex contention) back to their root causes.

3. **AI Reasoning Layer** — Serializes the causal chain to a structured JSON context payload and submits it to Gemini or a local Ollama instance to generate human-readable, technically precise root-cause reports with actionable remediation steps.

---

## ✨ Features

| Feature | Description |
|---|---|
| **Zero-polling telemetry** | Netlink `cn_proc` events — kernel pushes process events instead of polling `/proc` |
| **CausalTrace BFS** | Reverse-BFS multigraph traversal from symptom to root cause |
| **AI diagnostics** | Gemini & Ollama integration with real-time streaming responses |
| **Low-overhead live monitor** | 1 Hz `/proc` sampling and ANSI redraws; keyboard input remains responsive at 100 ms polling |
| **eBPF tracing** | Optional `bpftrace` syscall tracing for open/connect/execve events |
| **Vector codebase index** | AVX2 SIMD cosine similarity search maps causal graph nodes to source files |
| **Terminal markdown** | Custom streaming ANSI renderer for bold, code blocks, and color |
| **mimalloc global allocator** | Thread-local heaps; 40–70% faster allocation vs glibc |

---

## 📦 Installation

### Prerequisites

| Dependency | Version | Purpose |
|---|---|---|
| Rust toolchain | stable, edition 2021 | Build and run SysPilot |
| `rust-analyzer` | optional | IDE diagnostics and navigation |
| Linux | required for daemon/telemetry | `/proc` and Netlink Process Connector |

Install Rust with [rustup](https://rustup.rs/) if it is not already installed. `rust-analyzer` is detected automatically by editors that support it once the repository root (the directory containing `Cargo.toml`) is opened.

### Build

```bash
git clone https://github.com/yourusername/syspilot.git
cd syspilot
./build_rust.sh
```

Or use Cargo directly:

```bash
cargo build --release
```

The release binary is `./target/release/syspilot`. `build_rust.sh` opts into `target-cpu=native`; use the plain Cargo command when producing a binary to run on a different CPU.

> The C++ sources and `xmake.lua` are retained as a legacy implementation. `./build.sh` builds that variant and requires Xmake plus its C++ dependencies; it is not the Rust build path.

### Install

```bash
./target/release/syspilot install
```

This creates `~/.syspilot/` with:
- `config.json` — provider settings, API keys, model selection
- `syspilot.sh` — shell hook for capturing command history and exit codes

Add to your `~/.bashrc` or `~/.zshrc`:
```bash
source ~/.syspilot/syspilot.sh
```

---

## 🚀 Usage

### Start the Daemon
```bash
./target/release/syspilot daemon &
```
The daemon subscribes to Netlink `cn_proc`, initializes an in-memory process tree, and listens on `/tmp/syspilot.sock`. The monitor refreshes process data once per second to keep its overhead low; CPU use is workload- and system-dependent, so an absolute CPU ceiling cannot be guaranteed.

### Live TUI Monitor
```bash
./target/release/syspilot monitor
```

| Key | Action |
|---|---|
| `Tab` | Cycle sort: CPU% → I/O Rate → PID |
| `↑` / `↓` or `j` / `k` | Navigate process list |
| `e` or `Enter` | AI root-cause explanation for selected process |
| `s` | Send `SIGSTOP` (suspend) |
| `r` | Send `SIGCONT` (resume) |
| `x` | Send `SIGKILL` (terminate) |
| `q` | Quit |

### Explain Last Failed Command
```bash
./target/release/syspilot explain
```

### Causal Diagnostic by PID
```bash
# Standard procfs snapshot
./target/release/syspilot explain --pid 4582 --causal

# With eBPF syscall tracing (requires root or CAP_BPF)
sudo ./target/release/syspilot explain --pid 4582 --causal --ebpf

# With deep perf CPU profiling
./target/release/syspilot explain --pid 4582 --causal --deep
```

### Ask a General Question
```bash
./target/release/syspilot ask "why is vm.dirty_ratio causing write stalls under my workload?"
```

### Configure AI Provider
```bash
# Gemini
./target/release/syspilot config set-key gemini YOUR_API_KEY

# Local Ollama
./target/release/syspilot provider ollama
./target/release/syspilot config set-url ollama http://localhost:11434
```

### Check Status / Uninstall
```bash
./target/release/syspilot status
./target/release/syspilot uninstall
```

For daemon heartbeat checks and automatic crash restart, see [Daemon Reliability](docs/RELIABILITY.md).

---

## 🏗️ Architecture (Brief)

```
Linux Kernel (cn_proc)
      │ Netlink push events (fork/exec/exit)
      ▼
syspilotd daemon
  ├─ concurrent_hash_map<pid, ProcessNode>   (Intel TBB)
  ├─ ConcurrentQueue<ProcessEventRecord>     (Moodycamel, lock-free)
  └─ UNIX socket /tmp/syspilot.sock          (simdjson in, fmt out)
      │
      ▼
CausalTrace Engine
  ├─ take_proc_snapshot() → tsl::robin_map<pid, GraphNode>
  ├─ build_graph() → directed multigraph
  └─ trace_root_cause() → reverse-BFS with tsl::robin_set
      │
      ▼
AI Layer (Gemini / Ollama)
  └─ streaming JSON → MdStreamer → ANSI terminal
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full deep-dive.

---

## 📁 Repository Structure

```
syspilot/
├── build.sh                  # Build script (-Ofast -flto -march=native)
├── src/
│   ├── main.cpp              # CLI entry point & command router
│   ├── daemon.cpp/h          # syspilotd: Netlink + UNIX socket server
│   ├── causal_engine.cpp/h   # CausalTrace: multigraph + BFS + export
│   ├── telemetry.cpp/h       # /proc parser & system snapshot collector
│   ├── ai.cpp/h              # Gemini/Ollama API + MdStreamer renderer
│   ├── codebase.cpp/h        # Vector DB + SIMD cosine similarity search
│   ├── profiler.cpp/h        # perf CPU profiler integration
│   ├── config.cpp/h          # JSON config read/write (~/.syspilot/)
│   ├── safety.cpp/h          # Command safety allowlist
│   ├── utils.cpp/h           # String, file, shell utilities
│   ├── install.cpp/h         # Shell hook installer
│   ├── ui/
│   │   ├── tui.cpp/h         # Raw ANSI terminal UI (no ncurses)
│   │   └── streamer.cpp/h    # Real-time Markdown→ANSI renderer
│   ├── vendor/
│   │   ├── concurrentqueue.h  # Moodycamel ConcurrentQueue (vendored)
│   │   └── tsl/               # tsl::robin_map / robin_set (vendored)
│   └── nlohmann/              # nlohmann/json (vendored)
├── ARCHITECTURE.md
├── developer_guide.md
└── CONTRIBUTING.md
```

---

## 📄 License

MIT — see [LICENSE](LICENSE).
