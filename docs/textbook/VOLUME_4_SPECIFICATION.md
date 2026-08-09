> **Historical documentation notice**
>
> This document describes the retired C++ implementation. It is retained for historical reference only. It is not valid for the current Rust application. Use the current [documentation index](../README.md), [project README](../../README.md), and [architecture guide](../../ARCHITECTURE.md) for build, configuration, deployment, and behavior.

# VOLUME 4: ZERO-ALLOCATION MEMORY ENGINE & CONCURRENCY SPECIFICATIONS

---

## 4.1 Memory Architecture: Zero-Allocation `StringArena`

To guarantee that hot-path event processing emits zero heap allocations (`malloc`/`free`), SIOP uses a pre-allocated slab bump-allocator (`StringArena`).

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
    StringArena() {
        chunks_.push_back(std::make_unique<Chunk>());
    }

    std::string_view allocate(std::string_view str) {
        size_t len = str.size();
        if (current_offset_ + len > CHUNK_SIZE) {
            current_chunk_++;
            if (current_chunk_ >= chunks_.size()) {
                chunks_.push_back(std::make_unique<Chunk>());
            }
            current_offset_ = 0;
        }

        char* dest = &chunks_[current_chunk_]->data[current_offset_];
        std::memcpy(dest, str.data(), len);
        current_offset_ += len;
        return std::string_view(dest, len);
    }

    void reset() noexcept {
        current_chunk_ = 0;
        current_offset_ = 0;
    }
};

} // namespace syspilot::memory
```
