# VOLUME 8: MULTIMODAL CAUSAL REASONING MULTIGRAPH

---

## 8.1 Causal Graph C++ Core Subsystem

```cpp
namespace syspilot::causal {

enum class NodeType : uint8_t { PROCESS, FILE, SOCKET, CGROUP, DEVICE };
enum class EdgeType : uint8_t { SPAWNED_BY, READS_FROM, WRITES_TO, BLOCKED_ON, CONTENDS_WITH };

struct GraphNode {
    std::string_view id;
    NodeType         type;
    pid_t            pid{0};
    double           cpu_pct{0.0};
    double           read_rate_kb{0.0};
    double           write_rate_kb{0.0};
    bool             is_anomalous{false};
    std::string_view anomaly_reason;
};

struct GraphEdge {
    std::string_view from_id;
    std::string_view to_id;
    EdgeType         type;
    uint64_t         latency_ns{0};
    std::string_view details;
};

class CausalGraph {
private:
    tsl::robin_map<std::string_view, GraphNode> nodes_;
    std::vector<GraphEdge>                     edges_;
    memory::StringArena                        arena_;

public:
    void add_node(GraphNode node) {
        node.id = arena_.allocate(node.id);
        nodes_[node.id] = node;
    }

    void add_edge(GraphEdge edge) {
        edge.from_id = arena_.allocate(edge.from_id);
        edge.to_id   = arena_.allocate(edge.to_id);
        edges_.push_back(edge);
    }

    std::vector<std::string_view> trace_root_cause(std::string_view symptom_id) {
        std::vector<std::string_view> path;
        tsl::robin_set<std::string_view> visited;
        std::queue<std::string_view> q;

        q.push(symptom_id);
        visited.insert(symptom_id);

        while (!q.empty()) {
            auto curr = q.front();
            q.pop();
            path.push_back(curr);

            for (const auto& edge : edges_) {
                if (edge.from_id == curr && visited.find(edge.to_id) == visited.end()) {
                    visited.insert(edge.to_id);
                    q.push(edge.to_id);
                }
            }
        }
        return path;
    }
};

} // namespace syspilot::causal
```
