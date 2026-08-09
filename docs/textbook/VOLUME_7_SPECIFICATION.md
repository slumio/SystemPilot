> **Historical documentation notice**
>
> This document describes the retired C++ implementation. It is retained for historical reference only. It is not valid for the current Rust application. Use the current [documentation index](../README.md), [project README](../../README.md), and [architecture guide](../../ARCHITECTURE.md) for build, configuration, deployment, and behavior.

# VOLUME 7: INVERTED INDEXING & VECTORIZED QUERY ENGINE

---

## 7.1 Roaring Bitmap Index Implementation

```cpp
namespace syspilot::storage {

class RoaringBitmapIndex {
private:
    tsl::robin_map<std::string, roaring::Roaring> index_;

public:
    void insert(const std::string& tag, uint32_t row_id) {
        index_[tag].add(row_id);
    }

    roaring::Roaring query_and(const std::string& tag1, const std::string& tag2) const {
        auto it1 = index_.find(tag1);
        auto it2 = index_.find(tag2);

        if (it1 == index_.end() || it2 == index_.end()) {
            return roaring::Roaring();
        }

        roaring::Roaring result = it1->second;
        result &= it2->second;
        return result;
    }
};

} // namespace syspilot::storage
```
