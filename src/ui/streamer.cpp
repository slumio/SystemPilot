#include "streamer.h"
#include "../utils.h"
#include <iostream>
#include <cstring>

MdStreamer::MdStreamer() {}

// ─────────────────────────────────────────────────────────────────────────────
//  process_token — scan buffer starting at pos for a markdown token.
//  Appends ANSI escape to os_buf, advances pos, returns bytes consumed.
//  Returns 0 if no token matched (caller should emit one raw char).
//
//  FIX (AI not responding — bug A):
//  Old code did `buffer = buffer.substr(1)` on every character, which is an
//  O(N) heap copy per byte.  For a 2 KB AI response that's ~4 MB of copying.
//  This new version uses a size_t `pos` cursor — zero copies during scan.
// ─────────────────────────────────────────────────────────────────────────────
int MdStreamer::process_token(std::string& os_buf) {
    size_t remaining = buffer.size() - pos;
    const char* p    = buffer.data() + pos;

    auto starts = [&](const char* s, size_t n) -> bool {
        return remaining >= n && std::memcmp(p, s, n) == 0;
    };

    if (starts("```", 3)) {
        code_block = !code_block;
        os_buf += code_block ? "\x1b[36m" : "\x1b[0m";
        pos += 3; is_newline = false; return 3;
    }
    if (starts("$$", 2) || starts("\\[", 2) || starts("\\]", 2)) {
        math_block = !math_block;
        os_buf += math_block ? "\x1b[35m" : "\x1b[0m";
        pos += 2; is_newline = false; return 2;
    }
    if (starts("\\(", 2) || starts("\\)", 2)) {
        math_inline = !math_inline;
        os_buf += math_inline ? "\x1b[35m" : "\x1b[0m";
        pos += 2; is_newline = false; return 2;
    }
    if (starts("**", 2)) {
        bold = !bold;
        os_buf += bold ? "\x1b[1m" : "\x1b[22m";
        pos += 2; is_newline = false; return 2;
    }
    if (starts("### ", 4) && is_newline) {
        os_buf += "\x1b[1;96m### ";
        pos += 4; is_newline = false; return 4;
    }
    if (starts("## ", 3) && is_newline) {
        os_buf += "\x1b[1;96m## ";
        pos += 3; is_newline = false; return 3;
    }
    if (starts("# ", 2) && is_newline) {
        os_buf += "\x1b[1;96m# ";
        pos += 2; is_newline = false; return 2;
    }
    if ((starts("- ", 2) || starts("* ", 2)) && is_newline) {
        os_buf += "\x1b[33m\u2022 \x1b[0m";
        pos += 2; is_newline = false; return 2;
    }
    if (starts("`", 1)) {
        inline_code = !inline_code;
        os_buf += inline_code ? "\x1b[36m" : "\x1b[0m";
        pos += 1; is_newline = false; return 1;
    }
    if (starts("$", 1)) {
        math_inline = !math_inline;
        os_buf += math_inline ? "\x1b[35m" : "\x1b[0m";
        pos += 1; is_newline = false; return 1;
    }
    return 0;
}

// ─────────────────────────────────────────────────────────────────────────────
void MdStreamer::print(const std::string& text) {
    buffer += text;

    // Batch all terminal output into a local string, then write once.
    // Old code called std::cout.flush() per character — hundreds of
    // unnecessary syscalls per AI chunk made the stream feel laggy.
    std::string os_buf;
    os_buf.reserve(text.size() * 2);

    // FIX (AI not responding — bug B):
    // Old guard was `buffer.length() >= 4`, which swallowed the LAST 3 bytes
    // of every chunk without printing them (they accumulated and were never
    // drained unless flush() was called, which came too late).
    // New rule: scan while at least 3 bytes remain (enough for the longest
    // 3-byte token prefix: ```). flush() drains the final tail.
    while (pos + 3 <= buffer.size()) {
        if (process_token(os_buf) == 0) {
            char c = buffer[pos++];
            if (c == '\n') {
                is_newline = true;
                os_buf += "\x1b[0m";
                if (code_block) os_buf += "\x1b[36m";
                if (math_block) os_buf += "\x1b[35m";
                if (bold)       os_buf += "\x1b[1m";
            } else {
                is_newline = false;
            }
            os_buf += c;
        }
    }

    if (!os_buf.empty()) {
        std::cout.write(os_buf.data(), (std::streamsize)os_buf.size());
        std::cout.flush();
    }

    // Compact: drop consumed bytes so buffer doesn't grow unboundedly
    if (pos > 0) {
        buffer.erase(0, pos);
        pos = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
void MdStreamer::flush() {
    std::string os_buf;
    os_buf.reserve(buffer.size() * 2);

    // Drain the entire tail including the lookahead bytes print() left behind
    while (pos < buffer.size()) {
        if (process_token(os_buf) == 0) {
            char c = buffer[pos++];
            if (c == '\n') {
                is_newline = true;
                os_buf += "\x1b[0m";
                if (code_block) os_buf += "\x1b[36m";
                if (math_block) os_buf += "\x1b[35m";
                if (bold)       os_buf += "\x1b[1m";
            } else {
                is_newline = false;
            }
            os_buf += c;
        }
    }
    os_buf += "\x1b[0m"; // full reset at stream end
    if (!os_buf.empty()) {
        std::cout.write(os_buf.data(), (std::streamsize)os_buf.size());
        std::cout.flush();
    }
    buffer.clear();
    pos = 0;
}
