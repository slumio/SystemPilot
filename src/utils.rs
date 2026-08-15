use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;

// ── String helpers ────────────────────────────────────────────────────────────

pub fn trim(s: &str) -> &str {
    s.trim()
}

pub fn trim_owned(s: String) -> String {
    s.trim().to_string()
}

pub fn to_lower(s: &str) -> String {
    s.to_lowercase()
}

pub fn starts_with(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix)
}

pub fn ends_with(s: &str, suffix: &str) -> bool {
    s.ends_with(suffix)
}

/// Split a string by a char delimiter — equivalent to C++ split(str, char)
pub fn split_char(s: &str, delimiter: char) -> Vec<String> {
    s.split(delimiter).map(|t| t.to_string()).collect()
}

/// Split a string by a string delimiter — equivalent to C++ split(str, string)
pub fn split_str<'a>(s: &'a str, delimiter: &str) -> Vec<&'a str> {
    s.split(delimiter).collect()
}

pub fn replace_all(s: &str, from: &str, to: &str) -> String {
    s.replace(from, to)
}

// ── File / Directory helpers ──────────────────────────────────────────────────

pub fn file_exists(path: &str) -> bool {
    Path::new(path).exists()
}

pub fn is_directory(path: &str) -> bool {
    Path::new(path).is_dir()
}

pub fn get_file_size(path: &str) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

pub fn get_last_modified_time(path: &str) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

pub fn read_file_content(path: &str) -> io::Result<String> {
    fs::read_to_string(path)
}

pub fn write_file_content(path: &str, content: &str) -> io::Result<()> {
    fs::write(path, content)
}

pub fn delete_file(path: &str) -> bool {
    fs::remove_file(path).is_ok()
}

pub fn create_directory_recursive(path: &str) -> bool {
    fs::create_dir_all(path).is_ok()
}

/// List directory, optionally recursive.
/// Mirrors C++ list_directory — only returns files with known code/text extensions.
pub fn list_directory(path: &str, recursive: bool) -> Vec<String> {
    let valid_exts: &[&str] = &[
        "rs", "py", "js", "ts", "c", "cpp", "h", "hpp", "java", "go", "md", "txt", "html", "css",
        "json", "yaml", "yml", "sh", "toml",
    ];
    let mut files = Vec::new();

    // Try ripgrep first for speed
    if recursive {
        let output = Command::new("rg").arg("--files").arg(path).output();
        if let Ok(out) = output {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                for line in stdout.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Some(ext) = Path::new(line).extension().and_then(|e| e.to_str()) {
                        if valid_exts.contains(&ext) {
                            files.push(line.to_string());
                        }
                    }
                }
                return files;
            }
        }
    }

    // Fallback: std walkdir-equivalent via std::fs
    fn walk(
        dir: &Path,
        recursive: bool,
        valid_exts: &[&str],
        skip: &[&str],
        out: &mut Vec<String>,
    ) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if skip.contains(&name) {
                continue;
            }
            if p.is_dir() && recursive {
                walk(&p, recursive, valid_exts, skip, out);
            } else if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if valid_exts.contains(&ext) {
                        if let Some(s) = p.to_str() {
                            out.push(s.to_string());
                        }
                    }
                }
            }
        }
    }

    const SKIP: &[&str] = &[
        ".git",
        "target",
        "node_modules",
        "dist",
        "build",
        ".syspilot",
    ];
    walk(Path::new(path), recursive, valid_exts, SKIP, &mut files);
    files
}

// ── Process / Shell helpers ───────────────────────────────────────────────────

/// Run a shell command (via sh -c) and capture stdout+stderr.
/// Equivalent to C++ run_command_output.
pub fn run_command_output(cmd: &str) -> (String, i32) {
    let result = Command::new("sh").arg("-c").arg(cmd).output();
    match result {
        Ok(out) => {
            let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            if !stderr.is_empty() {
                combined.push_str(&stderr);
            }
            let code = out.status.code().unwrap_or(-1);
            (combined, code)
        }
        Err(e) => (format!("Failed to start process: {}", e), -1),
    }
}

/// Secure command runner — uses execvp without a shell, feeds `input` to stdin,
/// returns (stdout+stderr, exit_code). Equivalent to C++ run_command_secure.
pub fn run_command_secure(args: &[String], input: &str) -> (String, i32) {
    if args.is_empty() {
        return (String::new(), -1);
    }
    let mut child = match Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (format!("spawn failed: {}", e), -1),
    };

    // Write stdin concurrently to avoid pipe deadlock on large payloads
    let mut input_error = None;
    if !input.is_empty() {
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(input.as_bytes()) {
                input_error = Some(error);
            }
            // stdin closes when dropped — sends EOF
        }
    } else {
        drop(child.stdin.take());
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return (format!("wait failed: {}", e), -1),
    };

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stderr.is_empty() {
        combined.push_str(&stderr);
    }
    if let Some(error) = input_error {
        combined.push_str(&format!("stdin write failed: {error}"));
        return (combined, -1);
    }
    let code = output.status.code().unwrap_or(-1);
    (combined, code)
}

/// Streaming variant — calls `callback` with each chunk of stdout.
/// Stdin is written in a background thread to avoid deadlock.
/// Equivalent to C++ run_command_secure_stream.
pub fn run_command_secure_stream<F>(args: &[String], input: String, mut callback: F) -> (bool, i32)
where
    F: FnMut(&str),
{
    if args.is_empty() {
        return (false, -1);
    }

    let mut child = match Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("spawn failed: {}", e);
            return (false, -1);
        }
    };

    // Write stdin in a background thread
    let stdin_handle = if let Some(mut stdin) = child.stdin.take() {
        let input_clone = input.clone();
        Some(std::thread::spawn(move || {
            let result = stdin.write_all(input_clone.as_bytes());
            // stdin drops here, sending EOF
            result
        }))
    } else {
        None
    };

    // Drain stdout on this thread
    let mut buf = [0u8; 16384];
    if let Some(mut stdout) = child.stdout.take() {
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    callback(&String::from_utf8_lossy(chunk));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    }

    let stdin_ok = stdin_handle.is_none_or(|handle| {
        handle.join().is_ok_and(|result| {
            result.map(|_| true).unwrap_or_else(|error| {
                eprintln!("stdin write failed: {error}");
                false
            })
        })
    });

    // Surface curl stderr
    if let Some(mut stderr) = child.stderr.take() {
        let mut err_out = String::new();
        if let Err(error) = stderr.read_to_string(&mut err_out) {
            eprintln!("stderr read failed: {error}");
        }
        let trimmed = err_out.trim();
        if !trimmed.is_empty() {
            eprintln!("\x1b[33m[curl] {}\x1b[0m", trimmed);
        }
    }

    let status = match child.wait() {
        Ok(s) => s,
        Err(_) => return (false, -1),
    };
    let code = status.code().unwrap_or(-1);
    (code == 0 && stdin_ok, code)
}
