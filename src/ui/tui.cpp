#include "tui.h"
#include "../causal_engine.h"
#include "../telemetry.h"
#include "../utils.h"
#include "../ai.h"
#include "../ui/streamer.h"
#include "../config.h"
#include "../nlohmann/json.hpp"
#include <iostream>
#include <string>
#include <vector>
#include <unordered_map>
#include <chrono>
#include <thread>
#include <termios.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/select.h>
#include <signal.h>
#include <algorithm>
#include <sys/socket.h>
#include <sys/un.h>
#include <cstring>
#include <iomanip>
#include <sstream>
#include <dirent.h>
#include <fstream>

using json = nlohmann::json;

namespace ui {

struct TuiProcess {
    pid_t pid;
    pid_t ppid;
    std::string name;
    std::string state;
    double cpu_usage_pct = 0.0;
    double read_rate_kb  = 0.0;
    double write_rate_kb = 0.0;
    bool   is_anomalous  = false;
};

struct HistoryData {
    uint64_t cpu_ticks  = 0;
    uint64_t read_bytes = 0;
    uint64_t write_bytes = 0;
    std::chrono::steady_clock::time_point last_time;
};

static struct termios orig_termios;
static bool raw_mode_enabled = false;
static std::unordered_map<pid_t, HistoryData> g_history;

static void disable_raw_mode() {
    if (raw_mode_enabled) {
        tcsetattr(STDIN_FILENO, TCSAFLUSH, &orig_termios);
        raw_mode_enabled = false;
        std::cout << "\x1b[?25h\x1b[0m" << std::flush;
    }
}

static void enable_raw_mode() {
    if (!raw_mode_enabled) {
        tcgetattr(STDIN_FILENO, &orig_termios);
        std::atexit(disable_raw_mode);

        struct termios raw = orig_termios;
        raw.c_lflag &= ~(ECHO | ICANON | ISIG | IEXTEN);
        raw.c_iflag &= ~(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
        raw.c_cflag |= (CS8);
        raw.c_cc[VMIN]  = 0;
        raw.c_cc[VTIME] = 1;

        tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw);
        raw_mode_enabled = true;
        std::cout << "\x1b[?25l" << std::flush;
    }
}

// FIX TUI-3: was still using 50ms timeout — bumped to 500ms to match
// the causal_engine fix and give the daemon time to serialize a full tree.
static std::string query_daemon_pids() {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return "";

    struct sockaddr_un addr;
    std::memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    std::strncpy(addr.sun_path, "/tmp/syspilot.sock", sizeof(addr.sun_path) - 1);

    struct timeval tv = {0, 500000}; // 500ms — was 50ms (too tight for large trees)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));

    if (connect(fd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        close(fd); return "";
    }

    std::string req = "{\"request\":\"process_tree\"}";
    if (write(fd, req.c_str(), req.length()) < 0) {
        close(fd); return "";
    }

    std::string res;
    char buf[8192]; // larger read buffer — avoids many small read() calls
    while (true) {
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n <= 0) break;
        res.append(buf, n);
    }
    close(fd);
    return res;
}

// FIX TUI-1: Old code called collect_process_telemetry() for EVERY PID on
// EVERY frame (100 ms). With 300+ processes that's 300+ /proc reads per frame.
// New approach: parse the lightweight daemon response (name/state already there)
// and only open /proc/<pid>/stat + /proc/<pid>/io for rate calculation.
// That drops per-frame /proc reads from ~300 full telemetry collections to
// two targeted file reads per process.
static std::vector<TuiProcess> get_processes() {
    std::vector<TuiProcess> list;
    std::string daemon_res = query_daemon_pids();

    auto now = std::chrono::steady_clock::now();
    long clk_tck = sysconf(_SC_CLK_TCK);
    if (clk_tck <= 0) clk_tck = 100;

    // Fast path: daemon already gives us pid/ppid/name/state
    if (!daemon_res.empty()) {
        try {
            auto j = json::parse(daemon_res);
            if (j.value("status", "") == "ok" && j.contains("processes")) {
                for (const auto& p : j["processes"]) {
                    pid_t pid  = p["pid"];
                    TuiProcess tp;
                    tp.pid   = pid;
                    tp.ppid  = p["ppid"];
                    tp.name  = p["name"].get<std::string>();
                    tp.state = p["state"].get<std::string>();

                    // Read only /proc/<pid>/stat for CPU ticks (one file, ~100 bytes)
                    std::string stat_path = "/proc/" + std::to_string(pid) + "/stat";
                    std::ifstream stat_f(stat_path);
                    uint64_t utime = 0, stime = 0;
                    uint64_t read_bytes = 0, write_bytes = 0;
                    if (stat_f.is_open()) {
                        std::string line; std::getline(stat_f, line);
                        size_t rp = line.rfind(')');
                        if (rp != std::string::npos && rp + 2 < line.size()) {
                            std::istringstream ss(line.substr(rp + 2));
                            std::string tok;
                            int field = 3; // field 3 after ')'
                            while (ss >> tok) {
                                if (field == 14) utime = std::stoull(tok);
                                if (field == 15) { stime = std::stoull(tok); break; }
                                ++field;
                            }
                        }
                    }

                    // Read /proc/<pid>/io for I/O bytes (only if file is accessible)
                    std::string io_path = "/proc/" + std::to_string(pid) + "/io";
                    std::ifstream io_f(io_path);
                    if (io_f.is_open()) {
                        std::string line;
                        while (std::getline(io_f, line)) {
                            if (line.rfind("read_bytes:", 0) == 0)
                                read_bytes = std::stoull(line.substr(11));
                            else if (line.rfind("write_bytes:", 0) == 0)
                                write_bytes = std::stoull(line.substr(12));
                        }
                    }

                    uint64_t total_ticks = utime + stime;
                    auto hist_it = g_history.find(pid);
                    if (hist_it != g_history.end()) {
                        double elapsed = std::chrono::duration<double>(now - hist_it->second.last_time).count();
                        if (elapsed > 0.05) {
                            tp.cpu_usage_pct = ((double)(total_ticks - hist_it->second.cpu_ticks) / (double)clk_tck) / elapsed * 100.0;
                            tp.read_rate_kb  = (double)(read_bytes  - hist_it->second.read_bytes)  / 1024.0 / elapsed;
                            tp.write_rate_kb = (double)(write_bytes - hist_it->second.write_bytes) / 1024.0 / elapsed;
                        }
                    }
                    g_history[pid] = { total_ticks, read_bytes, write_bytes, now };

                    if (tp.state == "D" || tp.cpu_usage_pct > 80.0 || tp.write_rate_kb > 5000.0)
                        tp.is_anomalous = true;

                    list.push_back(tp);
                }
                return list;
            }
        } catch (...) {}
    }

    // Slow fallback path when daemon is not running (unchanged logic)
    DIR* dir = opendir("/proc");
    if (!dir) return list;
    struct dirent* entry;
    while ((entry = readdir(dir)) != nullptr) {
        if (entry->d_type != DT_DIR) continue;
        std::string name = entry->d_name;
        if (!std::all_of(name.begin(), name.end(), ::isdigit)) continue;
        pid_t pid = (pid_t)std::stoi(name);
        ProcessTelemetry pt = telemetry::collect_process_telemetry(pid);
        if (pt.pid == 0) continue;
        TuiProcess tp;
        tp.pid   = pid;
        tp.ppid  = pt.ppid;
        tp.name  = pt.name;
        tp.state = pt.state;
        uint64_t total_ticks = pt.utime + pt.stime;
        auto hist_it = g_history.find(pid);
        if (hist_it != g_history.end()) {
            double elapsed = std::chrono::duration<double>(now - hist_it->second.last_time).count();
            if (elapsed > 0.05) {
                tp.cpu_usage_pct = ((double)(total_ticks - hist_it->second.cpu_ticks) / (double)clk_tck) / elapsed * 100.0;
                tp.read_rate_kb  = (double)(pt.read_bytes  - hist_it->second.read_bytes)  / 1024.0 / elapsed;
                tp.write_rate_kb = (double)(pt.write_bytes - hist_it->second.write_bytes) / 1024.0 / elapsed;
            }
        }
        g_history[pid] = { total_ticks, pt.read_bytes, pt.write_bytes, now };
        if (tp.state == "D" || tp.cpu_usage_pct > 80.0 || tp.write_rate_kb > 5000.0)
            tp.is_anomalous = true;
        list.push_back(tp);
    }
    closedir(dir);
    return list;
}


// ─────────────────────────────────────────────────────────────────────────────
//  render_frame — builds the ENTIRE screen into a single std::string and
//  writes it with one write() call.
//
//  FIX TUI-2: Old code had a print_at() helper that called std::cout.flush()
//  on every row (and even per-cell for the border). With 300 processes that's
//  300+ flush() calls per 100 ms frame — each is a syscall barrier that stalls
//  the kernel TTY driver and empties the stdio buffer.
//
//  FIX TUI-4: Old draw_border() was called unconditionally every loop
//  iteration, redrawing the entire border frame even on frames where nothing
//  changed. Now the border is written only into the frame buffer and the
//  caller controls when to redraw.
// ─────────────────────────────────────────────────────────────────────────────
static void render_frame(int width, int height,
                         const std::vector<TuiProcess>& processes,
                         int selected_idx, int scroll_offset,
                         int sort_column,
                         const SystemTelemetry& st) {
    std::string fb;                          // frame buffer
    fb.reserve(width * height * 8);          // generous pre-alloc

    // ── Clear + move home (single ESC sequence instead of draw_border loop)
    fb += "\x1b[2J\x1b[H";

    // ── Top border
    fb += "\x1b[90m┌";
    for (int i = 0; i < width - 2; ++i) fb += "─";
    fb += "┐\r\n";

    // ── Content rows (pre-filled with side borders)
    for (int r = 0; r < height - 2; ++r) {
        fb += "│";
        fb += std::string(width - 2, ' ');
        fb += "│\r\n";
    }

    // ── Bottom border
    fb += "└";
    for (int i = 0; i < width - 2; ++i) fb += "─";
    fb += "┘\x1b[0m";

    // ── Header row (row 2)
    {
        std::ostringstream hss;
        hss << "🤖 SysPilot Monitor | Load: " << st.load_avg
            << " | Mem: " << ((st.mem_total_kb - st.mem_available_kb) / 1024)
            << "MB / " << (st.mem_total_kb / 1024) << "MB";
        std::string hdr = hss.str();
        fb += "\x1b[2;3H\x1b[1;36m";
        fb += hdr;
        fb += "\x1b[0m";

        std::string sort_label = sort_column == 0 ? "CPU%" : (sort_column == 1 ? "I/O Rate" : "PID");
        fb += "\x1b[2;" + std::to_string(width - 24) + "H\x1b[1;33m[Sorting by: " + sort_label + "]\x1b[0m";
    }

    // ── Column header (row 4)
    fb += "\x1b[4;3H\x1b[1;90m  PID    PPID   STATE   CPU%     DISK READ     DISK WRITE    PROCESS NAME\x1b[0m";
    fb += "\x1b[5;3H\x1b[90m" + std::string(width - 6, '-') + "\x1b[0m";

    // ── Process list rows
    int list_height = height - 8;
    for (int i = 0; i < list_height; ++i) {
        int row      = 6 + i;
        int proc_idx = scroll_offset + i;

        fb += "\x1b[" + std::to_string(row) + ";3H";

        if (proc_idx >= (int)processes.size()) {
            fb += std::string(width - 6, ' ');
            continue;
        }

        const auto& p = processes[proc_idx];
        std::ostringstream ss;
        ss << std::left << std::setw(8) << p.pid
           << std::setw(8) << p.ppid
           << std::setw(8) << p.state;

        std::ostringstream cpu_ss;
        cpu_ss << std::fixed << std::setprecision(1) << p.cpu_usage_pct << "%";
        ss << std::setw(9) << cpu_ss.str();

        std::ostringstream r_ss, w_ss;
        r_ss << std::fixed << std::setprecision(1) << p.read_rate_kb  << " KB/s";
        w_ss << std::fixed << std::setprecision(1) << p.write_rate_kb << " KB/s";
        ss << std::setw(14) << r_ss.str()
           << std::setw(14) << w_ss.str()
           << p.name;

        std::string line = ss.str();
        int avail = width - 6;
        if ((int)line.size() > avail)
            line.resize(avail);
        else
            line += std::string(avail - line.size(), ' ');

        if (proc_idx == selected_idx)
            fb += "\x1b[7m";
        else if (p.is_anomalous)
            fb += "\x1b[1;31m";
        else if (p.state == "R")
            fb += "\x1b[32m";

        fb += line;
        fb += "\x1b[0m";
    }

    // ── Footer
    fb += "\x1b[" + std::to_string(height - 2) + ";3H\x1b[1;90m";
    fb += "[Tab] Sort  [e] AI Explain  [s] SIGSTOP  [r] SIGCONT  [k] SIGKILL  [q] Quit";
    fb += "\x1b[0m";

    // ── Single write() — entire frame in one syscall
    std::cout.write(fb.data(), (std::streamsize)fb.size());
    std::cout.flush();
}

// ─────────────────────────────────────────────────────────────────────────────

void run_monitor() {
    enable_raw_mode();

    int sort_column  = 0;
    int selected_idx = 0;
    int scroll_offset = 0;

    struct winsize w;
    ioctl(STDOUT_FILENO, TIOCGWINSZ, &w);
    int width  = w.ws_col > 10 ? w.ws_col : 80;
    int height = w.ws_row > 10 ? w.ws_row : 24;

    auto last_refresh = std::chrono::steady_clock::now() - std::chrono::seconds(2); // force first refresh
    std::vector<TuiProcess> processes;

    while (true) {
        // ── Terminal resize check
        struct winsize cw;
        ioctl(STDOUT_FILENO, TIOCGWINSZ, &cw);
        width  = cw.ws_col > 10 ? cw.ws_col : 80;
        height = cw.ws_row > 10 ? cw.ws_row : 24;

        // ── Refresh processes every 1 second
        auto now = std::chrono::steady_clock::now();
        if (std::chrono::duration_cast<std::chrono::milliseconds>(now - last_refresh).count() >= 1000) {
            processes = get_processes();
            last_refresh = now;

            if (sort_column == 0)
                std::sort(processes.begin(), processes.end(), [](const TuiProcess& a, const TuiProcess& b) {
                    return a.cpu_usage_pct > b.cpu_usage_pct; });
            else if (sort_column == 1)
                std::sort(processes.begin(), processes.end(), [](const TuiProcess& a, const TuiProcess& b) {
                    return (a.read_rate_kb + a.write_rate_kb) > (b.read_rate_kb + b.write_rate_kb); });
            else
                std::sort(processes.begin(), processes.end(), [](const TuiProcess& a, const TuiProcess& b) {
                    return a.pid < b.pid; });
        }

        // ── Clamp selection
        if (processes.empty()) {
            selected_idx = 0;
        } else {
            if (selected_idx >= (int)processes.size()) selected_idx = (int)processes.size() - 1;
            if (selected_idx < 0) selected_idx = 0;
        }

        int list_height = height - 8;
        if (selected_idx < scroll_offset) scroll_offset = selected_idx;
        else if (selected_idx >= scroll_offset + list_height) scroll_offset = selected_idx - list_height + 1;

        // ── Collect system telemetry (lightweight — just /proc/loadavg + /proc/meminfo)
        SystemTelemetry st = telemetry::collect_system_telemetry();

        // ── Render entire frame as one write()
        render_frame(width, height, processes, selected_idx, scroll_offset, sort_column, st);

        // ── Non-blocking input (100ms poll)
        fd_set fds;
        FD_ZERO(&fds);
        FD_SET(STDIN_FILENO, &fds);
        struct timeval tv = {0, 100000};
        int ret = select(STDIN_FILENO + 1, &fds, nullptr, nullptr, &tv);
        if (ret > 0) {
            char c;
            if (read(STDIN_FILENO, &c, 1) > 0) {
                if (c == 'q') {
                    break;
                } else if (c == 9) { // Tab
                    sort_column = (sort_column + 1) % 3;
                    last_refresh = std::chrono::steady_clock::now() - std::chrono::seconds(2);
                } else if (c == 'j') {
                    selected_idx++;
                } else if (c == 'k') {
                    if (selected_idx > 0) selected_idx--;
                } else if (c == '\x1b') { // Escape sequences (arrow keys)
                    char seq[2];
                    if (read(STDIN_FILENO, &seq[0], 1) > 0 && read(STDIN_FILENO, &seq[1], 1) > 0) {
                        if (seq[0] == '[') {
                            if      (seq[1] == 'A') { if (selected_idx > 0) selected_idx--; }
                            else if (seq[1] == 'B') selected_idx++;
                        }
                    }
                } else if (c == 's') {
                    if (!processes.empty()) kill(processes[selected_idx].pid, SIGSTOP);
                } else if (c == 'r') {
                    if (!processes.empty()) kill(processes[selected_idx].pid, SIGCONT);
                } else if (c == 'k') {
                    if (!processes.empty()) kill(processes[selected_idx].pid, SIGKILL);
                } else if (c == 'e' || c == '\n') { // AI Explain
                    if (!processes.empty()) {
                        pid_t       target_pid  = processes[selected_idx].pid;
                        std::string target_name = processes[selected_idx].name;

                        disable_raw_mode();
                        std::cout << "\x1b[2J\x1b[H" << std::flush;
                        std::cout << "🧠 \x1b[1;36mQuerying SysPilot AI diagnostic explanation for PID "
                                  << target_pid << " (" << target_name << ")...\x1b[0m\n" << std::endl;

                        Config conf = config::load();
                        json ctx;
                        ctx["current_dir"]   = utils::trim(utils::run_command_output("pwd"));
                        ctx["analysis_type"] = "causal_inference_diagnostics";
                        ctx["target_process"] = target_name;
                        ctx["target_pid"]    = target_pid;

                        CausalGraph graph;
                        graph.build_graph(2, false, target_pid);
                        std::string target_node_id = "pid:" + std::to_string(target_pid);
                        std::vector<std::string> path = graph.trace_root_cause(target_node_id);
                        ctx["causal_chain"] = json::parse(graph.serialize_chain_to_json(path));

                        std::string prompt =
                            "You are a senior system reliability engineer performing root-cause analysis.\n"
                            "Here is the structured JSON representation of the traversed causal path:\n" +
                            ctx.dump(4) + "\n\n"
                            "Please explain the diagnostic findings, step-by-step root cause chain, and action recommendations.";

                        MdStreamer streamer;
                        ai::query_ai_stream(conf, prompt, streamer);

                        std::cout << "\n\x1b[90m" << std::string(60, '-')
                                  << "\nPress any key to return to monitor...\x1b[0m" << std::flush;

                        enable_raw_mode();
                        char dummy;
                        while (read(STDIN_FILENO, &dummy, 1) <= 0)
                            std::this_thread::sleep_for(std::chrono::milliseconds(50));

                        // Force a full refresh on next iteration
                        last_refresh = std::chrono::steady_clock::now() - std::chrono::seconds(2);
                    }
                }
            }
        }
    }

    disable_raw_mode();
}

} // namespace ui
