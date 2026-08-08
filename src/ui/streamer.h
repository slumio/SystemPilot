#ifndef STREAMER_H
#define STREAMER_H

#include <string>

class MdStreamer {
private:
    std::string buffer;
    size_t      pos          = 0;   // cursor into buffer — replaces O(N) substr(1) loop
    bool        bold         = false;
    bool        code_block   = false;
    bool        inline_code  = false;
    bool        math_block   = false;
    bool        math_inline  = false;
    bool        is_newline   = true;

    // Returns bytes consumed; appends ANSI escapes to os_buf.
    int process_token(std::string& os_buf);

public:
    MdStreamer();
    void print(const std::string& text);
    void flush();
};

#endif // STREAMER_H
