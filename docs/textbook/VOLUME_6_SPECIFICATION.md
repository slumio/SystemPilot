> **Historical documentation notice**
>
> This document describes the retired C++ implementation. It is retained for historical reference only. It is not valid for the current Rust application. Use the current [documentation index](../README.md), [project README](../../README.md), and [architecture guide](../../ARCHITECTURE.md) for build, configuration, deployment, and behavior.

# VOLUME 6: COLUMNAR TSDB STORAGE ENGINE & COMPRESSION MATH

---

## 6.1 Gorilla Floating-Point Encoding Engine Implementation

```cpp
namespace syspilot::storage {

class GorillaEncoder {
private:
    uint64_t             last_val_bits_{0};
    uint32_t             last_leading_zeros_{0xFFFFFFFF};
    uint32_t             last_trailing_zeros_{0};
    std::vector<uint8_t> buffer_;

public:
    void encode_double(double value) {
        uint64_t val_bits;
        std::memcpy(&val_bits, &value, sizeof(double));

        if (buffer_.empty()) {
            append_bits(val_bits, 64);
            last_val_bits_ = val_bits;
            return;
        }

        uint64_t xor_val = val_bits ^ last_val_bits_;

        if (xor_val == 0) {
            append_bit(0);
        } else {
            append_bit(1);
            uint32_t leading = __builtin_clzll(xor_val);
            uint32_t trailing = __builtin_ctzll(xor_val);

            if (leading >= last_leading_zeros_ && trailing >= last_trailing_zeros_) {
                append_bit(0);
                append_bits(xor_val >> last_trailing_zeros_, 64 - last_leading_zeros_ - last_trailing_zeros_);
            } else {
                append_bit(1);
                last_leading_zeros_ = leading;
                last_trailing_zeros_ = trailing;
                append_bits(leading, 5);
                uint32_t length = 64 - leading - trailing;
                append_bits(length, 6);
                append_bits(xor_val >> trailing, length);
            }
        }
        last_val_bits_ = val_bits;
    }

private:
    void append_bit(uint8_t bit) { /* Bitwise buffer write */ }
    void append_bits(uint64_t val, uint8_t num_bits) { /* Bitwise buffer write */ }
};

} // namespace syspilot::storage
```
