# SysPilot Master Architecture Specification: Volume 3
## Columnar TSDB Storage Engine, Encoding Algorithms & Inverted Indexing

---

## 1. Executive Storage Engine Architecture

The central storage engine of SIOP is designed for massive write throughput, high compression ratios, and fast vectorized analytics. Modeled after Log-Structured Merge-Tree (LSM-Tree) columnar databases (such as ClickHouse and Apache Parquet), data transitions from in-memory pre-allocated write buffers into immutable columnar disk chunks indexed by Roaring Bitmaps.

```
                               Ingest Stream (Kafka / gRPC)
                                            │
                                            ▼
                           ┌──────────────────────────────────┐
                           │   MemTable In-Memory Write Sink  │
                           │   (Gorilla + Double-Delta Buffers│
                           └────────────────┬─────────────────┘
                                            │ Flush (5s or 64MB)
                                            ▼
                           ┌──────────────────────────────────┐
                           │   Immutable Columnar SSTable     │
                           │   - Timestamps: Double-Delta     │
                           │   - Metrics: Gorilla / Chimp     │
                           │   - Tags: Dictionary + RLE       │
                           │   - Index: Roaring Bitmaps       │
                           └────────────────┬─────────────────┘
                                            │
                     ┌──────────────────────┴──────────────────────┐
                     ▼                                             ▼
         ┌───────────────────────┐                     ┌───────────────────────┐
         │ Sparse Primary Index  │                     │ Inverted Tag Index    │
         │ (Granule Range Index) │                     │ (Roaring Bitmaps)     │
         └───────────────────────┘                     └───────────────────────┘
```

---

## 2. Columnar Encoding Specifications

### 2.1 Gorilla Floating-Point Encoding Engine (`src/storage/gorilla_encoder.hpp`)

The Gorilla compression algorithm compresses 64-bit floating-point metrics (such as CPU utilization percentages and I/O rates) by calculating XOR differences between consecutive metric values.

```cpp
namespace syspilot::storage {

class GorillaEncoder {
private:
    uint64_t             last_val_bits_{0};
    uint32_t             last_leading_zeros_{0xFFFFFFFF};
    uint32_t             last_trailing_zeros_{0};
    std::vector<uint8_t> buffer_;
    size_t               bit_offset_{0};

public:
    GorillaEncoder() {
        buffer_.reserve(64 * 1024); // Reserve 64 KiB buffer
    }

    void encode_double(double value) {
        uint64_t val_bits;
        std::memcpy(&val_bits, &value, sizeof(double));

        if (buffer_.empty()) {
            // Write first value uncompressed (64 bits)
            append_bits(val_bits, 64);
            last_val_bits_ = val_bits;
            return;
        }

        uint64_t xor_val = val_bits ^ last_val_bits_;

        if (xor_val == 0) {
            // Control bit 0: Value identical to previous value
            append_bit(0);
        } else {
            // Control bit 1: Value differs
            append_bit(1);
            uint32_t leading = __builtin_clzll(xor_val);
            uint32_t trailing = __builtin_ctzll(xor_val);

            if (leading >= last_leading_zeros_ && trailing >= last_trailing_zeros_) {
                // Control bit 0: Reuse previous leading/trailing zero boundaries
                append_bit(0);
                uint32_t bits_to_write = 64 - last_leading_zeros_ - last_trailing_zeros_;
                append_bits(xor_val >> last_trailing_zeros_, bits_to_write);
            } else {
                // Control bit 1: Write new leading and length bounds
                append_bit(1);
                last_leading_zeros_ = leading;
                last_trailing_zeros_ = trailing;
                append_bits(leading, 5); // 5 bits for leading zero count (0-31)
                uint32_t length = 64 - leading - trailing;
                append_bits(length, 6);  // 6 bits for length (0-63)
                append_bits(xor_val >> trailing, length);
            }
        }
        last_val_bits_ = val_bits;
    }

private:
    void append_bit(uint8_t bit) {
        // Bitwise packing implementation
    }
    void append_bits(uint64_t val, uint8_t num_bits) {
        // Bitwise packing implementation
    }
};

} // namespace syspilot::storage
```

---

## 3. Inverted Indexing via Roaring Bitmaps

SIOP maintains multi-dimensional tag search capabilities (e.g. querying across specific container IDs, host names, or error exit codes) using compressed **Roaring Bitmaps**.

```cpp
namespace syspilot::storage {

class RoaringBitmapIndex {
private:
    // Map tag string -> bitmap of matching row IDs
    tsl::robin_map<std::string, roaring::Roaring> index_;

public:
    void insert(const std::string& tag, uint32_t row_id) {
        index_[tag].add(row_id);
    }

    // Fast set operations across multiple criteria
    roaring::Roaring query_and(const std::string& tag1, const std::string& tag2) const {
        auto it1 = index_.find(tag1);
        auto it2 = index_.find(tag2);

        if (it1 == index_.end() || it2 == index_.end()) {
            return roaring::Roaring();
        }

        roaring::Roaring result = it1->second;
        result &= it2->second; // Fast SIMD bitmap AND operation
        return result;
    }
};

} // namespace syspilot::storage
```

---

## 4. Columnar Disk Layout & Part Compaction

Columnar disk chunks are organized into immutable files containing block header metadata, primary key range sparse indexes, encoded column streams, and bitmap indexes:

```
┌─────────────────────────────────────────────────────────────────────────────────────────────────┐
│ COLUMN CHUNK FILE (.part)                                                                       │
├─────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 1. Header (Magic Bytes: 'SIOP', Version: 2, Total Rows: 8,192, Compression: Zstd-3)           │
├─────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 2. Sparse Index Block (Granule Range Index mapping Row 0, 8192, 16384 to file offsets)        │
├─────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 3. Column Stream: timestamp.col  (Double-Delta Compressed)                                      │
│ 4. Column Stream: pid.col        (Varint Encoded)                                               │
│ 5. Column Stream: cpu_pct.col    (Gorilla Double Encoded)                                        │
│ 6. Column Stream: tag_dict.col   (Dictionary Symbol Table + RLE)                                │
├─────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 7. Inverted Index Block (Roaring Bitmaps for fast multi-attribute filter scans)                 │
└─────────────────────────────────────────────────────────────────────────────────────────────────┘
```
