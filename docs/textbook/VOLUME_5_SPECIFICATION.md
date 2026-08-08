# VOLUME 5: SIMD VECTORIZED EVENT PIPELINE & DEDUPLICATION

---

## 5.1 SIMD Vectorized Event Matching (AVX2 Execution)

```cpp
namespace syspilot::pipeline {

struct alignas(64) EventBatch {
    static constexpr size_t BATCH_CAPACITY = 1024;
    bpftrace_event_t events[BATCH_CAPACITY];
    size_t count{0};
};

class SIMDFilter {
public:
    // SIMD AVX2 vectorized PID filter across 8 events per iteration
    size_t filter_pids_avx2(EventBatch& batch, uint32_t target_pid) noexcept {
        size_t write_idx = 0;
        __m256i target_vec = _mm256_set1_epi32(static_cast<int>(target_pid));

        for (size_t i = 0; i < batch.count; i += 8) {
            int pids[8] = {
                static_cast<int>(batch.events[i+0].pid), static_cast<int>(batch.events[i+1].pid),
                static_cast<int>(batch.events[i+2].pid), static_cast<int>(batch.events[i+3].pid),
                static_cast<int>(batch.events[i+4].pid), static_cast<int>(batch.events[i+5].pid),
                static_cast<int>(batch.events[i+6].pid), static_cast<int>(batch.events[i+7].pid)
            };
            __m256i loaded_pids = _mm256_loadu_si256(reinterpret_cast<const __m256i*>(pids));
            __m256i cmp_mask = _mm256_cmpeq_epi32(loaded_pids, target_vec);
            int mask = _mm256_movemask_epi8(cmp_mask);

            for (size_t j = 0; j < 8 && (i + j) < batch.count; ++j) {
                if (batch.events[i + j].pid == target_pid || target_pid == 0) {
                    batch.events[write_idx++] = batch.events[i + j];
                }
            }
        }
        batch.count = write_idx;
        return write_idx;
    }
};

} // namespace syspilot::pipeline
```
