# SysPilot Master Architecture Specification: Volume 1
## Kernel Telemetry, eBPF CO-RE Probes & Subsystem Collector Mechanics

---

## 1. Executive Kernel Subsystem Architecture

The kernel telemetry layer of the **SysPilot Infrastructure Observability Platform (SIOP)** provides zero-patch, event-driven visibility into Linux system operations. Operating entirely within kernel space via Extended Berkeley Packet Filter (eBPF) bytecode, hardware Performance Monitoring Units (PMUs), and kernel multicast sockets, SIOP captures process execution, thread scheduling, memory allocations, VFS file I/O, network socket traffic, and hardware counters with nanosecond precision.

```
┌─────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                          LINUX KERNEL SPACE                                             │
│                                                                                                         │
│  ┌─────────────────────────────┐   ┌─────────────────────────────┐   ┌───────────────────────────────┐  │
│  │   eBPF Static Tracepoints   │   │   eBPF Dynamic Probes       │   │  Hardware PMU Profiling       │  │
│  │   - tp/sched/sched_switch   │   │   - fexit/vfs_read          │   │  - perf_event_open (99Hz)     │  │
│  │   - tp/syscalls/sys_enter_* │   │   - fexit/vfs_write         │   │  - PEBS / LBR HW Counters     │  │
│  │   - tp/net/netif_receive_skb│   │   - uprobe/SSL_write        │   │  - LLC & Branch Mispredicts   │  │
│  └──────────────┬──────────────┘   └──────────────┬──────────────┘   └───────────────┬───────────────┘  │
│                 │                                 │                                  │                  │
│                 ▼                                 ▼                                  ▼                  │
│  ┌───────────────────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                              bpf_ringbuf_reserve() & bpf_ringbuf_submit()                            │  │
│  └────────────────────────────────────────────────┬──────────────────────────────────────────────────┘  │
│                                                   │                                                     │
│                                                   ▼                                                     │
│  ┌───────────────────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                       BPF_MAP_TYPE_RINGBUF (16 MiB Shared Kernel-Userspace Page Ring)            │  │
│  └────────────────────────────────────────────────┬──────────────────────────────────────────────────┘  │
└───────────────────────────────────────────────────┼─────────────────────────────────────────────────────┘
                                                    │ epoll_wait() / zero-copy mmap read
                                                    ▼
┌─────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                       USERSPACE DAEMON (syspilotd)                                      │
└─────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. eBPF CO-RE Probe Specifications & Attach Points

### 2.1 Static Tracepoint Attachment Matrix

Static kernel tracepoints present stable, version-independent ABI contracts. They carry zero performance overhead when probes are disabled.

| Subsystem | Tracepoint Attach Path | Trigger Event | Primary Telemetry Extracted |
|---|---|---|---|
| **Scheduler** | `tp/sched/sched_switch` | Task context switch | Prev/Next PID, TID, state, execution CPU, run-queue latency |
| **Syscall Entry** | `tp/syscalls/sys_enter_*` | System call invocation | Syscall ID, input pointers, process credentials (`uid`, `gid`) |
| **Syscall Exit** | `tp/syscalls/sys_exit_*` | System call return | Syscall ID, return value (`int64_t`), execution latency nanoseconds |
| **Networking** | `tp/net/netif_receive_skb` | Packet ingress | Socket buffer length, protocol, interface index, cgroup ID |
| **Block I/O** | `tp/block/block_rq_issue` | Disk request dispatch | Device major/minor, sector size, write/read operation bitmask |

### 2.2 Dynamic Probe & Trampoline Attach Specifications (`fexit` / Kprobes)

For internal kernel functions lacking static tracepoints, SIOP uses eBPF **`fexit` trampolines** (Linux 5.5+), which replace traditional `kretprobes`. `fexit` probes execute inline via direct kernel function trampolines, avoiding the breakpoint interrupt trap cost of legacy kprobes.

```c
// BPF program attached to VFS file read entry and exit via fexit trampoline
SEC("fexit/vfs_read")
int BPF_PROG(trace_vfs_read_exit, struct file *file, char __user *buf, size_t count, loff_t *pos, ssize_t ret)
{
    uint64_t pid_tgid = bpf_get_current_pid_tgid();
    uint32_t pid = pid_tgid >> 32;
    uint32_t tid = (uint32_t)pid_tgid;

    // Filter check: fast out if PID is ignored
    if (is_pid_filtered(pid))
        return 0;

    // Reserve space inside kernel-userspace ring buffer
    struct bpftrace_event_t *ev;
    ev = bpf_ringbuf_reserve(&events_ringbuf, sizeof(*ev), 0);
    if (!ev) {
        // Increment atomic drop counter map on ring buffer full
        increment_drop_counter(EVENT_TYPE_VFS_READ);
        return 0;
    }

    // Populate event fields directly in ring memory (zero-copy)
    ev->timestamp_ns = bpf_ktime_get_ns();
    ev->pid          = pid;
    ev->tid          = tid;
    ev->cgroup_id    = bpf_get_current_cgroup_id();
    ev->event_type   = EVENT_TYPE_VFS_READ;
    ev->ret_val      = ret;
    ev->arg1         = count;
    ev->arg2         = (uint64_t)file;

    bpf_ringbuf_submit(ev, 0);
    return 0;
}
```

---

## 3. Kernel Memory Layouts & Data Structure Contracts

### 3.1 64-Byte Cache-Aligned Kernel Event Struct (`bpftrace_event_t`)

```c
// Explicitly packed and 64-byte aligned to match CPU cache lines and avoid false sharing
struct __attribute__((packed, aligned(64))) bpftrace_event_t {
    uint64_t timestamp_ns;    // Kernel monotonic time (ns) via bpf_ktime_get_ns()
    uint32_t pid;             // Process ID (TGID in kernel terms)
    uint32_t tid;             // Thread ID (PID in kernel terms)
    uint32_t ppid;            // Parent Process ID
    uint32_t uid;             // User ID of process context
    uint64_t cgroup_id;       // Kernel cgroup v2 inode ID
    uint16_t event_type;      // Telemetry event code identifier
    uint8_t  cpu_id;          // Hardware CPU core ID
    uint8_t  flags;           // Bit 0: Dropped; Bit 1: Anomaly; Bit 2: Sampled
    int64_t  ret_val;         // Syscall return status / byte count
    uint64_t arg1;            // Generic payload slot 1 (FD, pointer, bytes)
    uint64_t arg2;            // Generic payload slot 2 (address, port, flags)
    uint64_t stack_id;        // eBPF stack trace map index
};
```

### 3.2 eBPF Map Definitions

```c
// 1. Primary Kernel-Userspace Ring Buffer Map (16 MiB pre-allocated)
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 16 * 1024 * 1024);
} events_ringbuf SEC(".maps");

// 2. Kernel Stack Trace Storage Map (32,768 unique stack call chains)
struct {
    __uint(type, BPF_MAP_TYPE_STACK_TRACE);
    __uint(max_entries, 32768);
    __uint(key_size, sizeof(uint32_t));
    __uint(value_size, 128 * sizeof(uint64_t)); // Up to 128 instruction pointers per stack
} stack_traces SEC(".maps");

// 3. Per-CPU Event Loss & Drop Counters
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 64);
    __type(key, uint32_t);
    __type(value, uint64_t);
} drop_counters SEC(".maps");
```

---

## 4. Hardware PMU & Process Connector Subsystems

### 4.1 PMU Profiler (`perf_event_open` @ 99Hz)
SIOP configures PMU hardware counters to sample instruction addresses and hardware metrics at 99 Hz to prevent harmonic synchronization with 100 Hz timer interrupts.

```c
struct perf_event_attr pe_attr = {
    .type           = PERF_TYPE_HARDWARE,
    .size           = sizeof(struct perf_event_attr),
    .config         = PERF_COUNT_HW_INSTRUCTIONS,
    .sample_freq    = 99, // 99 Hz sampling frequency
    .sample_type    = PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_CALLCHAIN | PERF_SAMPLE_CPU,
    .freq           = 1,
    .disabled       = 1,
    .exclude_kernel = 0, // Profile both user and kernel space
    .precise_ip     = 2, // Enable PEBS (Precise Event-Based Sampling) on Intel
};
```

### 4.2 Netlink Process Lifecycle Listener (`NETLINK_CONNECTOR`)
Listens to `PROC_EVENT_FORK`, `PROC_EVENT_EXEC`, `PROC_EVENT_EXIT`, and `PROC_EVENT_UID` events over a kernel multicast Netlink socket (`CN_IDX_PROC`), achieving zero-polling process execution tracing.
