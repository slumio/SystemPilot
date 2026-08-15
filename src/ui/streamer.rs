use std::io::{self, Write};

/// Real-time Markdown → ANSI terminal renderer.
/// Processes a streaming token-by-token input and emits ANSI escape codes
/// for bold, code blocks, headers, and bullet points.
pub struct MdStreamer {
    buffer: String,
    pos: usize,
    bold: bool,
    code_block: bool,
    inline_code: bool,
    math_block: bool,
    math_inline: bool,
    is_newline: bool,
}

impl MdStreamer {
    pub fn new() -> Self {
        MdStreamer {
            buffer: String::new(),
            pos: 0,
            bold: false,
            code_block: false,
            inline_code: false,
            math_block: false,
            math_inline: false,
            is_newline: true,
        }
    }

    /// Try to match and emit a markdown token at `self.pos`.
    /// Returns bytes consumed (0 if no token matched).
    fn process_token(&mut self, out: &mut String) -> usize {
        let buf = &self.buffer[self.pos..];

        if buf.starts_with("```") {
            self.code_block = !self.code_block;
            out.push_str(if self.code_block {
                "\x1b[36m"
            } else {
                "\x1b[0m"
            });
            self.is_newline = false;
            return 3;
        }
        if buf.starts_with("$$") || buf.starts_with("\\[") || buf.starts_with("\\]") {
            self.math_block = !self.math_block;
            out.push_str(if self.math_block {
                "\x1b[35m"
            } else {
                "\x1b[0m"
            });
            self.is_newline = false;
            return 2;
        }
        if buf.starts_with("\\(") || buf.starts_with("\\)") {
            self.math_inline = !self.math_inline;
            out.push_str(if self.math_inline {
                "\x1b[35m"
            } else {
                "\x1b[0m"
            });
            self.is_newline = false;
            return 2;
        }
        if buf.starts_with("**") {
            self.bold = !self.bold;
            out.push_str(if self.bold { "\x1b[1m" } else { "\x1b[22m" });
            self.is_newline = false;
            return 2;
        }
        if self.is_newline {
            if buf.starts_with("### ") {
                out.push_str("\x1b[1;96m### ");
                self.is_newline = false;
                return 4;
            }
            if buf.starts_with("## ") {
                out.push_str("\x1b[1;96m## ");
                self.is_newline = false;
                return 3;
            }
            if buf.starts_with("# ") {
                out.push_str("\x1b[1;96m# ");
                self.is_newline = false;
                return 2;
            }
            if buf.starts_with("- ") || buf.starts_with("* ") {
                out.push_str("\x1b[33m\u{2022} \x1b[0m");
                self.is_newline = false;
                return 2;
            }
        }
        if buf.starts_with('`') {
            self.inline_code = !self.inline_code;
            out.push_str(if self.inline_code {
                "\x1b[36m"
            } else {
                "\x1b[0m"
            });
            self.is_newline = false;
            return 1;
        }
        if buf.starts_with('$') {
            self.math_inline = !self.math_inline;
            out.push_str(if self.math_inline {
                "\x1b[35m"
            } else {
                "\x1b[0m"
            });
            self.is_newline = false;
            return 1;
        }
        0
    }

    /// Feed a chunk of text to the streamer.
    pub fn print(&mut self, text: &str) {
        self.buffer.push_str(text);
        let mut out = String::with_capacity(text.len() * 2);

        // Keep a 3-byte lookahead so we never split a multi-byte token prefix
        while self.pos + 3 <= self.buffer.len() {
            let consumed = self.process_token(&mut out);
            if consumed > 0 {
                self.pos += consumed;
            } else {
                let c = self.buffer[self.pos..]
                    .chars()
                    .next()
                    .expect("streamer position is within the buffer");
                self.pos += c.len_utf8();
                if c == '\n' {
                    self.is_newline = true;
                    out.push_str("\x1b[0m");
                    if self.code_block {
                        out.push_str("\x1b[36m");
                    }
                    if self.math_block {
                        out.push_str("\x1b[35m");
                    }
                    if self.bold {
                        out.push_str("\x1b[1m");
                    }
                } else {
                    self.is_newline = false;
                }
                out.push(c);
            }
        }

        if !out.is_empty() {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            let _ = handle.write_all(out.as_bytes());
            let _ = handle.flush();
        }

        // Compact: discard consumed bytes
        if self.pos > 0 {
            self.buffer.drain(..self.pos);
            self.pos = 0;
        }
    }

    /// Drain the remaining tail (called at end of stream).
    pub fn flush(&mut self) {
        let mut out = String::with_capacity(self.buffer.len() * 2);

        while self.pos < self.buffer.len() {
            let consumed = self.process_token(&mut out);
            if consumed > 0 {
                self.pos += consumed;
            } else {
                let c = self.buffer[self.pos..]
                    .chars()
                    .next()
                    .expect("streamer position is within the buffer");
                self.pos += c.len_utf8();
                if c == '\n' {
                    self.is_newline = true;
                    out.push_str("\x1b[0m");
                    if self.code_block {
                        out.push_str("\x1b[36m");
                    }
                    if self.math_block {
                        out.push_str("\x1b[35m");
                    }
                    if self.bold {
                        out.push_str("\x1b[1m");
                    }
                } else {
                    self.is_newline = false;
                }
                out.push(c);
            }
        }

        out.push_str("\x1b[0m"); // full reset at stream end

        let stdout = io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(out.as_bytes());
        let _ = handle.flush();

        self.buffer.clear();
        self.pos = 0;
    }
}

impl Default for MdStreamer {
    fn default() -> Self {
        Self::new()
    }
}
