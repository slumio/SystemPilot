# VOLUME 9: MULTI-TIER IPC INFRASTRUCTURE & SHARED MEMORY RINGS

---

## 9.1 Shared Memory Publisher Interface (`/dev/shm/syspilot_metrics.ring`)

```cpp
namespace syspilot::ipc {

struct MetricsSnapshot {
    uint64_t timestamp_ns;
    double   cpu_user_pct;
    double   cpu_system_pct;
    uint64_t memory_rss_bytes;
    uint64_t total_events_processed;
    uint64_t total_events_dropped;
};

struct alignas(4096) SharedMetricsRegion {
    static constexpr size_t RING_SIZE = 1024;
    std::atomic<uint64_t> write_head{0};
    MetricsSnapshot       snapshots[RING_SIZE];
};

class SharedMemoryPublisher {
private:
    int fd_{-1};
    SharedMetricsRegion* region_{nullptr};

public:
    bool init() {
        fd_ = shm_open("/syspilot_metrics.ring", O_CREAT | O_RDWR, 0666);
        if (fd_ < 0) return false;
        ftruncate(fd_, sizeof(SharedMetricsRegion));
        region_ = static_cast<SharedMetricsRegion*>(
            mmap(nullptr, sizeof(SharedMetricsRegion), PROT_READ | PROT_WRITE, MAP_SHARED, fd_, 0)
        );
        return region_ != MAP_FAILED;
    }

    void publish(const MetricsSnapshot& snap) {
        if (!region_) return;
        uint64_t head = region_->write_head.load(std::memory_order_relaxed);
        region_->snapshots[head % SharedMetricsRegion::RING_SIZE] = snap;
        region_->write_head.store(head + 1, std::memory_order_release);
    }
};

} // namespace syspilot::ipc
```
