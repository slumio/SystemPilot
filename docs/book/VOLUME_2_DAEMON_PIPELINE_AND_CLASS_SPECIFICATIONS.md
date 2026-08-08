# SysPilot Master Architecture Specification: Volume 2
## Edge Daemon Architecture, Processing Pipeline & C++ Class Catalog

---

## 1. Executive Daemon Subsystem Architecture

The edge daemon (`syspilotd`) is a high-performance C++20 userspace process responsible for ingesting raw eBPF ring buffer byte streams, filtering and deduplicating events using SIMD instructions, enriching records with cgroup/container metadata, and publishing processed telemetry to local and central targets.

```
┌─────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                       syspilotd DAEMON ENGINE                                           │
│                                                                                                         │
│  ┌───────────────────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                                 CollectorManager (Epoll Reactor)                                  │  │
│  │   - RingBufferPoller           - NetlinkConnector           - PMUCollector                    │  │
│  └─────────────────────────────────────────────────┬─────────────────────────────────────────────────┘  │
│                                                    ▼                                                    │
│  ┌───────────────────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                              Lock-Free MPMC Ring (moodycamel Queue)                               │  │
│  └─────────────────────────────────────────────────┬─────────────────────────────────────────────────┘  │
│                                                    ▼                                                    │
│  ┌───────────────────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                                      Hot-Path Pipeline Worker                                     │  │
│  │   - SIMDFilter (AVX2)         - XXHashDeduplicator          - AdaptiveSampler                     │  │
│  │   - StringArena (Bump Alloc)   - MetadataEnricher            - DWARFUnwinder (Async)               │  │
│  └────────────────────────┬─────────────────────────────────────────────────┬────────────────────────┘  │
│                           │ Local IPC                                       │ Remote IPC                │
│                           ▼                                                 ▼                           │
│  ┌─────────────────────────────────┐               ┌─────────────────────────────────────────────────┐  │
│  │  SharedMemoryPublisher (/dev/shm)│               │  gRPCStreamer (Zstd Protobuf Stream)            │  │
│  └─────────────────────────────────┘               └─────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Complete C++ Class Catalog & Interface Specifications

### 2.1 Collection Layer Interfaces & Classes

#### `ICollector` Interface (`src/collector/icollector.hpp`)
```cpp
namespace syspilot::collector {

class ICollector {
public:
    virtual ~ICollector() = default;

    // Initializes collector hardware/kernel resources
    virtual bool start() = 0;

    // Stops collection loop and unbinds probes/sockets
    virtual void stop() = 0;

    // Returns file descriptor for epoll integration (-1 if poll-free)
    virtual int get_fd() const noexcept = 0;

    // Human-readable collector identifier
    virtual std::string_view name() const noexcept = 0;
};

} // namespace syspilot::collector
```

#### `RingBufferPoller` Class (`src/collector/ring_buffer_poller.hpp`)
```cpp
namespace syspilot::collector {

class alignas(64) RingBufferPoller final : public ICollector {
private:
    int                                        ring_fd_{-1};
    struct ring_buffer*                        rb_{nullptr};
    moodycamel::ConcurrentQueue<bpftrace_event_t>* target_queue_{nullptr};
    std::atomic<uint64_t>                      events_polled_{0};
    std::atomic<uint64_t>                      events_dropped_{0};

public:
    explicit RingBufferPoller(moodycamel::ConcurrentQueue<bpftrace_event_t>* queue);
    ~RingBufferPoller() override;

    bool start() override;
    void stop() override;
    int get_fd() const noexcept override { return ring_fd_; }
    std::string_view name() const noexcept override { return "RingBufferPoller"; }

    // Polls eBPF ring buffer pages and enqueues records (lockless)
    int poll(int timeout_ms) noexcept;
    
    uint64_t total_polled() const noexcept { return events_polled_.load(std::memory_order_relaxed); }
};

} // namespace syspilot::collector
```

#### `NetlinkConnector` Class (`src/collector/netlink_connector.hpp`)
```cpp
namespace syspilot::collector {

class NetlinkConnector final : public ICollector {
private:
    int                 nl_fd_{-1};
    struct sockaddr_nl  local_addr_{};
    alignas(64) char    recv_buf_[8192];

public:
    NetlinkConnector();
    ~NetlinkConnector() override;

    bool start() override;
    void stop() override;
    int get_fd() const noexcept override { return nl_fd_; }
    std::string_view name() const noexcept override { return "NetlinkConnector"; }

    // Processes incoming process fork/exec/exit events from kernel socket
    void handle_netlink_event() noexcept;
};

} // namespace syspilot::collector
```

---

### 2.2 Processing Pipeline Classes

#### `StringArena` Bump Allocator (`src/memory/string_arena.hpp`)
```cpp
namespace syspilot::memory {

class StringArena {
private:
    static constexpr size_t CHUNK_SIZE = 256 * 1024; // 256 KiB Slabs
    struct Chunk {
        alignas(64) char data[CHUNK_SIZE];
    };

    std::vector<std::unique_ptr<Chunk>> chunks_;
    size_t                              current_chunk_{0};
    size_t                              current_offset_{0};

public:
    StringArena();
    ~StringArena() = default;

    // Fast bump-allocation for strings (O(1) amortized, zero malloc)
    std::string_view allocate(std::string_view str);

    // Recycles all memory chunks in O(1) time
    void reset() noexcept;

    size_t total_allocated_bytes() const noexcept;
};

} // namespace syspilot::memory
```

#### `SIMDFilter` Vector Engine (`src/pipeline/simd_filter.hpp`)
```cpp
namespace syspilot::pipeline {

struct alignas(64) EventBatch {
    static constexpr size_t BATCH_CAPACITY = 1024;
    bpftrace_event_t events[BATCH_CAPACITY];
    size_t count{0};
};

class SIMDFilter {
private:
    uint32_t target_pid_filter_{0};

public:
    explicit SIMDFilter(uint32_t target_pid = 0) : target_pid_filter_(target_pid) {}

    // SIMD AVX2 vectorized PID filtering across 8 events per iteration
    size_t filter_pids_avx2(EventBatch& batch, uint32_t target_pid) noexcept;

    // Fallback scalar filter for non-AVX2 CPUs
    size_t filter_pids_scalar(EventBatch& batch, uint32_t target_pid) noexcept;
};

} // namespace syspilot::pipeline
```

#### `AdaptiveSampler` Load Shedding Class (`src/pipeline/adaptive_sampler.hpp`)
```cpp
namespace syspilot::pipeline {

class AdaptiveSampler {
private:
    double   sample_rate_{1.0};
    uint64_t total_seen_{0};
    uint64_t total_sampled_{0};

public:
    AdaptiveSampler() = default;

    // Evaluates event sampling status based on host CPU usage and event flags
    bool should_sample(const bpftrace_event_t& ev, double host_cpu_pct) noexcept;

    double current_sample_rate() const noexcept { return sample_rate_; }
};

} // namespace syspilot::pipeline
```

---

### 2.3 Memory Alignment & Threading Models

| Class Name | Threading Model | Memory Alignment | Primary Allocation Strategy |
|---|---|---|---|
| `RingBufferPoller` | Single-Producer Thread | 64-byte Cache Aligned | Stack buffers + Mmap ring |
| `CollectorManager` | Main Reactor Loop (`epoll`) | Standard | Member vector initialization |
| `StringArena` | Thread-Local / Single Worker | 64-byte Cache Aligned | Pre-allocated 256 KiB Chunk Slabs |
| `SIMDFilter` | Worker Pipeline Threads | 64-byte Cache Aligned | In-place register mutation |
| `SharedMemoryPublisher` | Non-blocking Lockless | 4096-byte Page Aligned | Memory mapped file (`/dev/shm`) |
