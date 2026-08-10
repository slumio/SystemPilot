/// Tests for src/ui/streamer.rs — Markdown → ANSI renderer.
///
/// Strategy: capture what the streamer writes to stdout by redirecting
/// stdout to a pipe in the test process. Because MdStreamer writes directly
/// via io::stdout().lock(), we instead inspect the *internal* buffer-drain
/// logic by calling print() + flush() and capturing via a helper that
/// replaces stdout with a pipe for the duration of the test.
///
/// For simplicity we test the *output string* by capturing it through
/// a child process that echoes the streamer output back over stdout.
/// This avoids unsafe global-state tricks in multi-threaded test runners.
use syspilot::ui::streamer::MdStreamer;

// ── Helper: run streamer, capture its writes via a subprocess ────────────────
// Because MdStreamer writes to real stdout, the cleanest unit-testable surface
// is the *state mutations* we can observe indirectly. We test the public API
// contract (no panic, flush drains everything) and use a pipe-redirect helper
// for output-content assertions.

fn capture_streamer_output(inputs: &[&str]) -> String {
    // Build a tiny inline Rust program that creates a MdStreamer, feeds the
    // given inputs, flushes, and exits. We run it as a child and capture
    // its stdout.
    // For test speed we instead test via a Python-style approach: we fork,
    // redirect stdout of the child to a pipe, run the streamer there, and
    // read back the result.

    // Simpler approach: use libc::pipe + fork
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe failed");
    let pipe_read = fds[0];
    let pipe_write = fds[1];

    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");

    if pid == 0 {
        // Child: redirect stdout to write end of pipe
        unsafe {
            libc::dup2(pipe_write, 1); // stdout = pipe_write
            libc::close(pipe_read);
            libc::close(pipe_write);
        }
        let mut s = MdStreamer::new();
        for &input in inputs {
            s.print(input);
        }
        s.flush();
        unsafe { libc::_exit(0) };
    }

    // Parent: close write end, read from read end
    unsafe { libc::close(pipe_write) };

    let mut output = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libc::read(pipe_read, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
        output.extend_from_slice(&buf[..n as usize]);
    }
    unsafe {
        libc::close(pipe_read);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }

    String::from_utf8_lossy(&output).into_owned()
}

// ── Basic API: no panic ───────────────────────────────────────────────────────

#[test]
fn streamer_new_does_not_panic() {
    let _s = MdStreamer::new();
}

#[test]
fn streamer_flush_on_empty_does_not_panic() {
    let mut s = MdStreamer::new();
    s.flush(); // must not panic or hang
}

#[test]
fn streamer_print_empty_does_not_panic() {
    let mut s = MdStreamer::new();
    s.print("");
    s.flush();
}

// ── Output content ────────────────────────────────────────────────────────────

#[test]
fn plain_text_passes_through() {
    let out = capture_streamer_output(&["hello world"]);
    assert!(
        out.contains("hello world"),
        "plain text should appear in output, got: {:?}",
        out
    );
}

#[test]
fn newline_in_input_appears_in_output() {
    let out = capture_streamer_output(&["line1\nline2"]);
    assert!(out.contains("line1"), "line1 should appear");
    assert!(out.contains("line2"), "line2 should appear");
}

#[test]
fn bold_tokens_emit_ansi_bold() {
    let out = capture_streamer_output(&["**bold text**"]);
    // Bold ON = ESC[1m, Bold OFF = ESC[22m
    assert!(out.contains("\x1b[1m"), "bold ON escape should be present");
    assert!(out.contains("bold text"), "bold text content should appear");
}

#[test]
fn inline_code_emits_cyan() {
    let out = capture_streamer_output(&["`some_code`"]);
    // Inline code = ESC[36m (cyan)
    assert!(
        out.contains("\x1b[36m"),
        "cyan escape for inline code should be present"
    );
    assert!(out.contains("some_code"));
}

#[test]
fn code_block_emits_cyan_and_reset() {
    let out = capture_streamer_output(&["```\ncode here\n```"]);
    assert!(out.contains("\x1b[36m"), "code block should emit cyan");
    assert!(
        out.contains("\x1b[0m"),
        "code block close should reset colour"
    );
    assert!(out.contains("code here"));
}

#[test]
fn h1_heading_emits_bright_colour() {
    let out = capture_streamer_output(&["# Title\n"]);
    // Headings use ESC[1;96m
    assert!(
        out.contains("\x1b[1;96m"),
        "H1 heading should emit bright cyan bold"
    );
    assert!(out.contains("Title"));
}

#[test]
fn h2_heading_emits_bright_colour() {
    let out = capture_streamer_output(&["## Section\n"]);
    assert!(
        out.contains("\x1b[1;96m"),
        "H2 heading should emit bright cyan bold"
    );
}

#[test]
fn bullet_list_emits_bullet_character() {
    let out = capture_streamer_output(&["- item one\n"]);
    assert!(
        out.contains('\u{2022}'),
        "bullet point (•) should appear for '- ' list items"
    );
}

#[test]
fn asterisk_bullet_emits_bullet_character() {
    let out = capture_streamer_output(&["* item two\n"]);
    assert!(
        out.contains('\u{2022}'),
        "asterisk list items should also render as •"
    );
}

#[test]
fn reset_emitted_at_stream_end() {
    let out = capture_streamer_output(&["some text"]);
    // flush() always appends ESC[0m at end
    assert!(
        out.ends_with("\x1b[0m"),
        "stream end must emit a full ANSI reset"
    );
}

// ── Chunked streaming (multi-call print) ──────────────────────────────────────

#[test]
fn chunked_input_produces_same_output_as_single() {
    let full = capture_streamer_output(&["hello **world** `code`"]);
    let chunked = capture_streamer_output(&["hello ", "**world**", " `code`"]);
    assert_eq!(
        full, chunked,
        "chunked input must produce identical output to single-call input"
    );
}

#[test]
fn token_split_across_chunks_handled() {
    // Split a bold marker ** across two chunks
    let out = capture_streamer_output(&["text *", "*bold** end"]);
    assert!(
        out.contains("bold"),
        "bold text split across chunks should still render"
    );
}

// ── State resets on newline ───────────────────────────────────────────────────

#[test]
fn heading_only_on_line_start() {
    // "# " in middle of line must NOT trigger heading colour
    let out = capture_streamer_output(&["mid # line\n"]);
    // The heading escape should NOT appear because "# " is not at line start
    // (streamer checks is_newline flag)
    let heading_escape = "\x1b[1;96m";
    // "mid " starts a line, then "# " is mid-line — no heading
    let lines_with_heading_esc: Vec<&str> = out
        .split('\n')
        .filter(|l| l.contains(heading_escape) && l.contains("mid"))
        .collect();
    assert!(
        lines_with_heading_esc.is_empty(),
        "heading escape must not appear mid-line, got: {:?}",
        lines_with_heading_esc
    );
}
