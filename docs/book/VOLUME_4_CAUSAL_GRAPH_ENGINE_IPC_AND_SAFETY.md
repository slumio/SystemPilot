# SysPilot Master Architecture Specification: Volume 4
## Multimodal Causal Graph Engine, IPC Mechanics & Production Safety Controls

---

## 1. Executive Causal Reasoning Architecture

The Causal Reasoning Engine constructs a dynamic topological Directed Acyclic Graph (DAG) linking processes, open file descriptors, network sockets, cgroup containers, and system resources. When a latency spike or error anomaly occurs, the engine traverses this graph to isolate root-cause paths across multiple telemetry sources.

```
       Telemetry Stream (Kernel Events, Traces, PMUs, Metrics)
                                  │
                                  ▼
               ┌─────────────────────────────────────┐
               │    Causal Multigraph Engine (DAG)   │
               │  - Nodes: Processes, Sockets, Files │
               │  - Edges: SPAWNED, READS, BLOCKED_ON│
               └──────────────────┬──────────────────┘
                                  │
                                  ▼
               ┌─────────────────────────────────────┐
               │   Anomaly Propagation Algorithm     │
               │ - Reverse BFS Root-Cause Traversal  │
               │ - Cross-Telemetry Evidence Scoring  │
               └──────────────────┬──────────────────┘
                                  │
                                  ▼
               ┌─────────────────────────────────────┐
               │ Isolated Root-Cause Subgraph Path   │
               │ (Provided to AI Engine & UI Client) │
               └─────────────────────────────────────┘
```

---

## 2. Causal Graph C++ Class Specifications

### 2.1 Graph Node & Edge Data Structures (`src/causal/graph_types.hpp`)

```cpp
namespace syspilot::causal {

enum class NodeType : uint8_t { PROCESS, FILE, SOCKET, CGROUP, DEVICE };
enum class EdgeType : uint8_t { SPAWNED_BY, READS_FROM, WRITES_TO, BLOCKED_ON, CONTENDS_WITH };

struct GraphNode {
    std::string_view id;          // e.g. "pid:4582" or "resource:/dev/sda"
    NodeType         type;
    pid_t            pid{0};
    double           cpu_pct{0.0};
    double           read_rate_kb{0.0};
    double           write_rate_kb{0.0};
    bool             is_anomalous{false};
    std::string_view anomaly_reason;
};

struct GraphEdge {
    std::string_view from_id;
    std::string_view to_id;
    EdgeType         type;
    uint64_t         latency_ns{0};
    std::string_view details;
};

class CausalGraph {
private:
    tsl::robin_map<std::string_view, GraphNode> nodes_;
    std::vector<GraphEdge>                     edges_;
    memory::StringArena                        arena_;

public:
    CausalGraph() = default;

    void add_node(GraphNode node) {
        node.id = arena_.allocate(node.id);
        nodes_[node.id] = node;
    }

    void add_edge(GraphEdge edge) {
        edge.from_id = arena_.allocate(edge.from_id);
        edge.to_id   = arena_.allocate(edge.to_id);
        edges_.push_back(edge);
    }

    // Reverse BFS Root-Cause Traversal Algorithm
    std::vector<std::string_view> trace_root_cause(std::string_view symptom_id) {
        std::vector<std::string_view> path;
        tsl::robin_set<std::string_view> visited;
        std::queue<std::string_view> q;

        q.push(symptom_id);
        visited.insert(symptom_id);

        while (!q.empty()) {
            auto curr = q.front();
            q.pop();
            path.push_back(curr);

            for (const auto& edge : edges_) {
                if (edge.from_id == curr && visited.find(edge.to_id) == visited.end()) {
                    visited.insert(edge.to_id);
                    q.push(edge.to_id);
                }
            }
        }
        return path;
    }

    // Calculates composite anomaly score across correlated metrics
    double calculate_node_anomaly_score(const GraphNode& node, double pmu_cache_miss_rate) const noexcept {
        double w1 = 0.35; // CPU usage weight
        double w2 = 0.25; // PMU cache miss weight
        double w3 = 0.25; // I/O wait rate weight
        double w4 = 0.15; // Error count weight

        double cpu_score = node.cpu_pct / 100.0;
        double io_score  = (node.read_rate_kb + node.write_rate_kb) / 10000.0;
        
        return (w1 * cpu_score) + (w2 * pmu_cache_miss_rate) + (w3 * io_score);
    }

    void clear() noexcept {
        nodes_.clear();
        edges_.clear();
        arena_.reset();
    }
};

} // namespace syspilot::causal
```

---

## 3. Inter-Process Communication (IPC) Specifications

SIOP layers specialized IPC mechanisms according to latency and throughput requirements:

```
┌─────────────────┬────────────────────────────────┬──────────────────────────┬─────────────────────────────┐
│ IPC Subsystem   │ Transport Mechanism            │ Latency SLA              │ Target Use Case             │
├─────────────────┼────────────────────────────────┼──────────────────────────┼─────────────────────────────┤
│ Kernel → Daemon │ `BPF_MAP_TYPE_RINGBUF` (mmap)  │ < 100 nanoseconds        │ Raw kernel telemetry events │
│ Daemon → Local  │ `/dev/shm/syspilot_metrics.ring`│ < 50 nanoseconds         │ Live TUI metric rendering   │
│ Daemon → Client │ UNIX Domain Socket (UDS stream)│ < 500 microseconds       │ Interactive CLI queries     │
│ Daemon → Central│ gRPC over HTTP/2 + Zstd Stream │ < 5 milliseconds         │ Regional/Central Aggregator │
└─────────────────┴────────────────────────────────┴──────────────────────────┴─────────────────────────────┘
```

---

## 4. The 13 Production Safety & Reliability Principles

1. **CPU Limit (< 1.0% Host CPU Core):** Event-driven eBPF tracepoints and `epoll_wait` reactor pattern eliminate polling overhead.
2. **Memory Limit (< 64 MiB Resident RAM):** Pre-allocated slab bump arenas (`StringArena`) and fixed pool allocations prevent memory growth.
3. **Zero Hot-Path Dynamic Allocations:** All records and string views are populated in-place inside pre-allocated slab pages.
4. **Lockless Multithreading:** Lock-free MPMC queue (`moodycamel::ConcurrentQueue`) and open-addressing maps (`tsl::robin_map`).
5. **Context Switch Minimization:** Adaptive epoll wakeup timer high-watermark algorithm keeps wakeups < 100 / sec.
6. **Kernel-Userspace Responsibility Separation:** eBPF probes emit raw struct bytes; expensive DWARF unwinding and string formatting occur in userspace.
7. **Backpressure & Drop Mitigation:** Atomic event drop counters in kernel eBPF ring buffer headers notify userspace of lost events.
8. **Monotonic Event Sequence Tracking:** Every event carries a per-CPU monotonic sequence counter to detect event loss gaps.
9. **Adaptive Dynamic Load Shedding:** Automated token bucket rate limiters dynamic throttle telemetry fidelity when host CPU > 80%.
10. **Kernel Instability Immunity:** All eBPF programs pass strict verification by the Linux kernel verifier. Zero kernel panic risk.
11. **Process Crash Isolation:** Independent daemon sandboxing; daemon failure leaves host application execution untouched.
12. **High-Throughput Vectorization:** AVX2/AVX-512 SIMD pipeline supporting > 5,000,000 events / sec / host.
13. **Resource Cgroup Hard Bounds:** Enforced systemd slice caps (`MemoryMax=128M`, `CPUQuota=5%`).
