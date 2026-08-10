> **Historical documentation notice**
>
> This document describes the retired C++ implementation. It is retained for historical reference only. It is not valid for the current Rust application. Use the current [documentation index](../README.md), [project README](../../README.md), and [architecture guide](../../ARCHITECTURE.md) for build, configuration, deployment, and behavior.

# VOLUME 2: HARDWARE PMU PROFILING & NETLINK PROCESS TOPOLOGY

---

## 2.1 Hardware Performance Monitoring Unit (PMU) Subsystem

Modern CPUs expose Hardware PMU counters that provide instruction-level performance insights without modifying executable binaries. SIOP configures `perf_event_open` to profile host hardware counters.

### 2.1.1 `perf_event_open` Configuration Specification
```c
struct perf_event_attr pe_attr = {
    .type           = PERF_TYPE_HARDWARE,
    .size           = sizeof(struct perf_event_attr),
    .config         = PERF_COUNT_HW_INSTRUCTIONS,
    .sample_freq    = 99,                   // 99 Hz sampling rate
    .sample_type    = PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_CALLCHAIN | PERF_SAMPLE_CPU,
    .read_format    = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING,
    .disabled       = 1,
    .pinned         = 1,
    .exclude_kernel = 0,                   // Profile both user and kernel space
    .exclude_hv     = 1,                   // Exclude hypervisor execution
    .precise_ip     = 2,                   // Enable PEBS (Precise Event-Based Sampling) on Intel
    .mmap           = 1,
    .comm           = 1,
    .task           = 1,
};
```

---

## 2.2 Netlink Process Connector Multicast Architecture

To track process execution lifecycle events without high-overhead `/proc` polling loops, SIOP binds to the Linux kernel's multicast Netlink Connector interface (`NETLINK_CONNECTOR`, group `CN_IDX_PROC`).

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                           LINUX KERNEL PROCESS MANAGER                                   │
│  - do_fork()                       - sys_execve()                       - do_exit()                      │
└─────────────────────────────────────────────────┬────────────────────────────────────────────────────────┘
                                                  │ Kernel Multicast Event Push
                                                  ▼
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                NETLINK_CONNECTOR (CN_IDX_PROC Group)                                     │
└─────────────────────────────────────────────────┬────────────────────────────────────────────────────────┘
                                                  │ recvmsg() Non-Blocking
                                                  ▼
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                 NetlinkConnector (syspilotd Worker)                                      │
│  - Receives PROC_EVENT_FORK, PROC_EVENT_EXEC, PROC_EVENT_EXIT in < 5 microseconds                        │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 2.3 Netlink Event Handling Logic
```cpp
namespace syspilot::collector {

void NetlinkConnector::handle_netlink_event() noexcept {
    alignas(64) char buf[8192];
    ssize_t len = recv(nl_fd_, buf, sizeof(buf), 0);
    if (len <= 0) return;

    auto* nlh = reinterpret_cast<struct nlmsghdr*>(buf);
    while (NLMSG_OK(nlh, len)) {
        if (nlh->nlmsg_type == NLMSG_DONE) break;
        
        auto* cn_msg = reinterpret_cast<struct cn_msg*>(NLMSG_DATA(nlh));
        auto* ev = reinterpret_cast<struct proc_event*>(cn_msg->data);

        switch (ev->what) {
        case proc_event::PROC_EVENT_FORK:
            on_process_fork(ev->event_data.fork.parent_pid, ev->event_data.fork.child_pid);
            break;
        case proc_event::PROC_EVENT_EXEC:
            on_process_exec(ev->event_data.exec.process_pid);
            break;
        case proc_event::PROC_EVENT_EXIT:
            on_process_exit(ev->event_data.exit.process_pid, ev->event_data.exit.exit_code);
            break;
        }
        nlh = NLMSG_NEXT(nlh, len);
    }
}

} // namespace syspilot::collector
```
