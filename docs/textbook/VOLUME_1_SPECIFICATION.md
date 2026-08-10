> **Historical documentation notice**
>
> This document is retained as historical design reference. It is not valid for the current Rust application. Use the current [documentation index](../README.md), [project README](../../README.md), and [architecture guide](../../ARCHITECTURE.md) for build, configuration, deployment, and behavior.

# VOLUME 1: LINUX KERNEL INTERNALS, TRACING MECHANICS & eBPF CO-RE SUBSYSTEM

---

## 1.1 Executive System Architectural Framework

The **SysPilot Infrastructure Observability Platform (SIOP)** operates at the lowest levels of the Linux kernel to deliver sub-microsecond event visibility with guaranteed host non-interference. This volume specifies the kernel telemetry subsystem, eBPF bytecode execution mechanics, Compile Once - Run Everywhere (CO-RE) relocations, and BPF Type Format (BTF) type specifications.

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                              LINUX KERNEL BOUNDARY                                               │
│                                                                                                                  │
│  ┌───────────────────────────┐   ┌───────────────────────────┐   ┌───────────────────────────────────────────┐  │
│  │ eBPF Static Tracepoints   │   │ eBPF Dynamic Trampolines  │   │ Hardware PMU Events (perf_event_open)     │  │
│  │ - tp/sched/sched_switch   │   │ - fexit/vfs_read          │   │ - PEBS (Precise Event-Based Sampling)    │  │
│  │ - tp/syscalls/sys_enter_* │   │ - fexit/vfs_write         │   │ - LBR (Last Branch Record) @ 99Hz         │  │
│  │ - tp/syscalls/sys_exit_*  │   │ - fexit/tcp_v4_connect    │   │ - LLC Cache Misses & Branch Mispredicts   │  │
│  └─────────────┬─────────────┘   └─────────────┬─────────────┘   └─────────────────────┬─────────────────────┘  │
│                │                               │                                       │                        │
│                ▼                               ▼                                       ▼                        │
│  ┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                                 bpf_ringbuf_reserve() & bpf_ringbuf_submit()                             │  │
│  └─────────────────────────────────────────────────────┬──────────────────────────────────────────────────────┘  │
│                                                        │                                                        │
│                                                        ▼                                                        │
│  ┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                           BPF_MAP_TYPE_RINGBUF (16 MiB Page-Aligned Shared Ring Buffer)                    │  │
│  └─────────────────────────────────────────────────────┬──────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────┼────────────────────────────────────────────────────────┘
                                                         │ epoll_wait() zero-copy read
                                                         ▼
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                       USERSPACE EDGE DAEMON (syspilotd)                                          │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 1.2 Kernel Attachment Points & Probe Specifications

### 1.2.1 Scheduler Tracepoints (`tp/sched/sched_switch`)
Captures context switches between tasks on every CPU core.
- **Probe Signature**: `SEC("tp/sched/sched_switch") int handle_sched_switch(struct trace_event_raw_sched_switch *ctx)`
- **Payload Extracted**: Prev PID, Prev TID, Prev Task State (`R`, `S`, `D`, `Z`), Next PID, Next TID, CPU Core ID.
- **Overhead**: ~12 nanoseconds per invocation. Zero memory allocations.

### 1.2.2 Syscall Entry Tracepoints (`tp/syscalls/sys_enter_*`)
Intercepts all system call entries across target process namespaces.
- **Probe Signature**: `SEC("tp/syscalls/sys_enter_openat") int handle_sys_enter_openat(struct trace_event_raw_sys_enter *ctx)`
- **Payload Extracted**: Directory File Descriptor, Filename Pointer, Flags, Mode, Caller PID/TID/UID/GID.

### 1.2.3 Dynamic Trampoline Probes (`SEC("fexit/vfs_read")`)
Uses Linux 5.5+ `fexit` BPF trampolines to attach directly to kernel routine returns without CPU trap overhead.
```c
SEC("fexit/vfs_read")
int BPF_PROG(trace_vfs_read_exit, struct file *file, char __user *buf, size_t count, loff_t *pos, ssize_t ret)
{
    uint64_t pid_tgid = bpf_get_current_pid_tgid();
    uint32_t pid = pid_tgid >> 32;
    uint32_t tid = (uint32_t)pid_tgid;

    if (is_pid_filtered(pid))
        return 0;

    struct bpftrace_event_t *ev;
    ev = bpf_ringbuf_reserve(&events_ringbuf, sizeof(*ev), 0);
    if (!ev) {
        increment_drop_counter(EVENT_TYPE_VFS_READ);
        return 0;
    }

    ev->timestamp_ns = bpf_ktime_get_ns();
    ev->pid          = pid;
    ev->tid          = tid;
    ev->ppid         = get_parent_pid();
    ev->uid          = bpf_get_current_uid_gid();
    ev->cgroup_id    = bpf_get_current_cgroup_id();
    ev->event_type   = EVENT_TYPE_VFS_READ;
    ev->ret_val      = ret;
    ev->arg1         = count;
    ev->arg2         = (uint64_t)file;
    ev->stack_id     = bpf_get_stackid(ctx, &stack_traces, BPF_F_FAST_STACK_CMP);

    bpf_ringbuf_submit(ev, 0);
    return 0;
}
```

---

## 1.3 64-Byte Cache-Line Aligned Struct Layout & Offsets

```
 0                   1               2               3               4
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                       timestamp_ns (uint64_t)                      |  Offset 0..7
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|          pid (uint32_t)       |          tid (uint32_t)       |  Offset 8..15
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|         ppid (uint32_t)       |          uid (uint32_t)       |  Offset 16..23
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                       cgroup_id (uint64_t)                         |  Offset 24..31
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|  event_type (uint16)  |cpu|flg|       ret_val (int64_t)           |  Offset 32..43
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         arg1 (uint64_t)                            |  Offset 44..51
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                         arg2 (uint64_t)                            |  Offset 52..59
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|      stack_id (uint32_t)      |         PADDING (4 bytes)         |  Offset 60..63
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

| Field Name | Data Type | Byte Offset | Size (Bytes) | Field Description |
|---|---|---|---|---|
| `timestamp_ns` | `uint64_t` | 0 | 8 | Monotonic kernel time nanoseconds (`bpf_ktime_get_ns()`) |
| `pid` | `uint32_t` | 8 | 4 | Process ID (Thread Group ID in kernel) |
| `tid` | `uint32_t` | 12 | 4 | Thread ID |
| `ppid` | `uint32_t` | 16 | 4 | Parent Process ID |
| `uid` | `uint32_t` | 20 | 4 | User Identifier |
| `cgroup_id` | `uint64_t` | 24 | 8 | Inode identifier for cgroup v2 hierarchy |
| `event_type` | `uint16_t` | 32 | 2 | Unique telemetry event identifier |
| `cpu_id` | `uint8_t` | 34 | 1 | CPU Core ID event originated on |
| `flags` | `uint8_t` | 35 | 1 | Bitwise status flags (`0x1` drop, `0x2` anomaly) |
| `ret_val` | `int64_t` | 36 | 8 | Syscall return value or error code |
| `arg1` | `uint64_t` | 44 | 8 | Generic payload parameter 1 (FD, address, byte count) |
| `arg2` | `uint64_t` | 52 | 8 | Generic payload parameter 2 (address, port, flags) |
| `stack_id` | `uint32_t` | 60 | 4 | Kernel stack map key |
