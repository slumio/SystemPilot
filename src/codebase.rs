use crate::config::Config;
use crate::utils;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct FileRegistry {
    pub file_path: String,
    pub last_modified: u64,
    pub size: u64,
}

#[derive(Debug, Default, Clone)]
pub struct DbChunk {
    pub file_path: String,
    pub content: String,
    pub start_line: u32,
    pub end_line: u32,
    pub embedding: Vec<f32>,
}

pub struct RawChunk {
    pub content: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Default)]
pub struct VectorDb {
    pub workspace_path: String,
    pub files: Vec<FileRegistry>,
    pub chunks: Vec<DbChunk>,
}

// ── Binary serialization ──────────────────────────────────────────────────────

fn write_str(out: &mut dyn Write, s: &str) -> std::io::Result<()> {
    let len = s.len() as u32;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(s.as_bytes())
}

fn read_str(inp: &mut dyn Read) -> std::io::Result<String> {
    let mut len_buf = [0u8; 4];
    inp.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 10 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "string too long",
        ));
    }
    let mut buf = vec![0u8; len];
    inp.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

impl VectorDb {
    const MAGIC: &'static [u8] = b"SYSPILOT_VDB_2";

    pub fn load_from_binary(path: &str) -> Option<Self> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut magic = [0u8; 14];
        f.read_exact(&mut magic).ok()?;
        if magic != Self::MAGIC {
            return None;
        }

        let workspace_path = read_str(&mut f).ok()?;

        let mut cnt_buf = [0u8; 4];
        f.read_exact(&mut cnt_buf).ok()?;
        let files_count = u32::from_le_bytes(cnt_buf) as usize;
        let mut files = Vec::with_capacity(files_count);
        for _ in 0..files_count {
            let file_path = read_str(&mut f).ok()?;
            let mut tmp = [0u8; 8];
            f.read_exact(&mut tmp).ok()?;
            let last_modified = u64::from_le_bytes(tmp);
            f.read_exact(&mut tmp).ok()?;
            let size = u64::from_le_bytes(tmp);
            files.push(FileRegistry {
                file_path,
                last_modified,
                size,
            });
        }

        f.read_exact(&mut cnt_buf).ok()?;
        let chunks_count = u32::from_le_bytes(cnt_buf) as usize;
        let mut chunks = Vec::with_capacity(chunks_count);
        for _ in 0..chunks_count {
            let file_path = read_str(&mut f).ok()?;
            let content = read_str(&mut f).ok()?;
            let mut u32_buf = [0u8; 4];
            f.read_exact(&mut u32_buf).ok()?;
            let start_line = u32::from_le_bytes(u32_buf);
            f.read_exact(&mut u32_buf).ok()?;
            let end_line = u32::from_le_bytes(u32_buf);
            f.read_exact(&mut u32_buf).ok()?;
            let embed_len = u32::from_le_bytes(u32_buf) as usize;
            if embed_len > 8192 {
                return None;
            }
            let mut embed = vec![0f32; embed_len];
            let byte_len = embed_len * 4;
            let mut bytes = vec![0u8; byte_len];
            f.read_exact(&mut bytes).ok()?;
            for (i, chunk) in bytes.chunks_exact(4).enumerate() {
                embed[i] = f32::from_le_bytes(chunk.try_into().unwrap());
            }
            chunks.push(DbChunk {
                file_path,
                content,
                start_line,
                end_line,
                embedding: embed,
            });
        }

        Some(VectorDb {
            workspace_path,
            files,
            chunks,
        })
    }

    pub fn save_to_binary(&self, path: &str) -> std::io::Result<()> {
        let mut f = std::fs::File::create(path)?;
        f.write_all(Self::MAGIC)?;
        write_str(&mut f, &self.workspace_path)?;

        f.write_all(&(self.files.len() as u32).to_le_bytes())?;
        for reg in &self.files {
            write_str(&mut f, &reg.file_path)?;
            f.write_all(&reg.last_modified.to_le_bytes())?;
            f.write_all(&reg.size.to_le_bytes())?;
        }

        f.write_all(&(self.chunks.len() as u32).to_le_bytes())?;
        for chunk in &self.chunks {
            write_str(&mut f, &chunk.file_path)?;
            write_str(&mut f, &chunk.content)?;
            f.write_all(&chunk.start_line.to_le_bytes())?;
            f.write_all(&chunk.end_line.to_le_bytes())?;
            f.write_all(&(chunk.embedding.len() as u32).to_le_bytes())?;
            for v in &chunk.embedding {
                f.write_all(&v.to_le_bytes())?;
            }
        }
        Ok(())
    }
}

// ── SIMD cosine similarity ────────────────────────────────────────────────────
// Rust's auto-vectoriser produces AVX2/SSE instructions with opt-level=3,
// so this scalar loop compiles to SIMD without unsafe intrinsics.

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

pub fn normalize_vec(v: &mut Vec<f32>) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

// ── File chunker ──────────────────────────────────────────────────────────────

fn post_process(raw: Vec<RawChunk>) -> Vec<RawChunk> {
    let mut out: Vec<RawChunk> = Vec::new();
    let mut acc: Option<RawChunk> = None;
    for rc in raw {
        let lines = rc.end_line.saturating_sub(rc.start_line) + 1;
        if let Some(mut a) = acc.take() {
            a.content.push('\n');
            a.content.push_str(&rc.content);
            a.end_line = rc.end_line;
            let merged = a.end_line.saturating_sub(a.start_line) + 1;
            if merged >= 8 {
                out.push(a);
            } else {
                acc = Some(a);
            }
        } else if lines < 8 {
            acc = Some(rc);
        } else {
            out.push(rc);
        }
    }
    if let Some(a) = acc {
        out.push(a);
    }
    out
}

pub fn chunk_file(path: &str, strategy: &str) -> Vec<RawChunk> {
    if utils::get_file_size(path) > 1024 * 1024 {
        return Vec::new();
    }
    let content = match utils::read_file_content(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    if content.contains('\0') {
        return Vec::new();
    } // binary file

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    if strategy == "syntactic" {
        match ext {
            "rs" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "java" => {
                let mut raw: Vec<RawChunk> = Vec::new();
                let mut cur: Vec<&str> = Vec::new();
                let mut start = 1u32;
                let mut depth = 0i32;
                for (i, &line) in lines.iter().enumerate() {
                    let tr = line.trim();
                    let opens = tr.chars().filter(|&c| c == '{').count() as i32;
                    let closes = tr.chars().filter(|&c| c == '}').count() as i32;
                    let is_def = depth <= 1
                        && (tr.starts_with("fn ")
                            || tr.starts_with("pub fn ")
                            || tr.starts_with("struct ")
                            || tr.starts_with("pub struct ")
                            || tr.starts_with("enum ")
                            || tr.starts_with("pub enum ")
                            || tr.starts_with("impl ")
                            || tr.starts_with("class ")
                            || tr.starts_with("function ")
                            || tr.starts_with("async function "));
                    if is_def && !cur.is_empty() {
                        raw.push(RawChunk {
                            content: cur.join("\n"),
                            start_line: start,
                            end_line: i as u32,
                        });
                        cur.clear();
                        start = i as u32 + 1;
                    }
                    cur.push(line);
                    depth = (depth + opens - closes).max(0);
                }
                if !cur.is_empty() {
                    raw.push(RawChunk {
                        content: cur.join("\n"),
                        start_line: start,
                        end_line: lines.len() as u32,
                    });
                }
                return post_process(raw);
            }
            "py" => {
                let mut raw: Vec<RawChunk> = Vec::new();
                let mut cur: Vec<&str> = Vec::new();
                let mut start = 1u32;
                for (i, &line) in lines.iter().enumerate() {
                    let tr = line.trim();
                    let is_root = (tr.starts_with("def ") || tr.starts_with("class "))
                        && !line.starts_with(' ')
                        && !line.starts_with('\t');
                    if is_root && !cur.is_empty() {
                        raw.push(RawChunk {
                            content: cur.join("\n"),
                            start_line: start,
                            end_line: i as u32,
                        });
                        cur.clear();
                        start = i as u32 + 1;
                    }
                    cur.push(line);
                }
                if !cur.is_empty() {
                    raw.push(RawChunk {
                        content: cur.join("\n"),
                        start_line: start,
                        end_line: lines.len() as u32,
                    });
                }
                return post_process(raw);
            }
            "md" => {
                let mut raw: Vec<RawChunk> = Vec::new();
                let mut cur: Vec<&str> = Vec::new();
                let mut start = 1u32;
                for (i, &line) in lines.iter().enumerate() {
                    if line.starts_with('#') && !cur.is_empty() {
                        raw.push(RawChunk {
                            content: cur.join("\n"),
                            start_line: start,
                            end_line: i as u32,
                        });
                        cur.clear();
                        start = i as u32 + 1;
                    }
                    cur.push(line);
                }
                if !cur.is_empty() {
                    raw.push(RawChunk {
                        content: cur.join("\n"),
                        start_line: start,
                        end_line: lines.len() as u32,
                    });
                }
                return post_process(raw);
            }
            _ => {}
        }
    }

    // Sliding window fallback
    let chunk_size = 40usize;
    let overlap = 10usize;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < lines.len() {
        let end = (start + chunk_size).min(lines.len());
        chunks.push(RawChunk {
            content: lines[start..end].join("\n"),
            start_line: start as u32 + 1,
            end_line: end as u32,
        });
        if end == lines.len() {
            break;
        }
        start += chunk_size - overlap;
    }
    chunks
}

// ── Embeddings via API ────────────────────────────────────────────────────────

fn fetch_embeddings(texts: &[String], config: &Config) -> Vec<Vec<f32>> {
    let mut results = Vec::new();
    if texts.is_empty() {
        return results;
    }

    match config.active_provider.as_str() {
        "gemini" => {
            if config.gemini_api_key.is_empty() {
                eprintln!("⚠️  Gemini API key not set for embedding generation.");
                return results;
            }
            let model = if config.embedding_model.contains("gemini")
                || config.embedding_model.contains("embedding")
            {
                config.embedding_model.clone()
            } else {
                "text-embedding-004".to_string()
            };
            let model_path = if model.starts_with("models/") {
                model.clone()
            } else {
                format!("models/{}", model)
            };

            let requests: Vec<serde_json::Value> = texts
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "model": model_path,
                        "content": { "parts": [{ "text": t }] }
                    })
                })
                .collect();
            let payload = serde_json::json!({ "requests": requests }).to_string();
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/{}:batchEmbedContents?key={}",
                model_path, config.gemini_api_key
            );
            let args: Vec<String> = vec![
                "curl".into(),
                "-s".into(),
                "-X".into(),
                "POST".into(),
                "-H".into(),
                "Content-Type: application/json".into(),
                "-d".into(),
                "@-".into(),
                url,
            ];
            let (resp, code) = utils::run_command_secure(&args, &payload);
            if code == 0 {
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(&resp) {
                    if let Some(arr) = j["embeddings"].as_array() {
                        for item in arr {
                            if let Some(vals) = item["values"].as_array() {
                                results.push(
                                    vals.iter()
                                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                                        .collect(),
                                );
                            }
                        }
                    }
                }
            }
        }
        "ollama" => {
            for text in texts {
                let truncated = if text.len() > 1000 {
                    &text[..1000]
                } else {
                    text.as_str()
                };
                let payload =
                    serde_json::json!({ "model": config.embedding_model, "input": truncated })
                        .to_string();
                let url = format!("{}/api/embed", config.ollama_url);
                let args: Vec<String> = vec![
                    "curl".into(),
                    "-s".into(),
                    "-X".into(),
                    "POST".into(),
                    "-H".into(),
                    "Content-Type: application/json".into(),
                    "-d".into(),
                    "@-".into(),
                    url,
                ];
                let (resp, code) = utils::run_command_secure(&args, &payload);
                if code == 0 {
                    if let Ok(j) = serde_json::from_str::<serde_json::Value>(&resp) {
                        if let Some(arr) = j["embeddings"].as_array() {
                            for item in arr {
                                if let Some(vals) = item.as_array() {
                                    results.push(
                                        vals.iter()
                                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                                            .collect(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    results
}

// ── DB path ───────────────────────────────────────────────────────────────────

fn get_db_path(workspace: &str) -> String {
    let dir = crate::config::get_syspilot_dir().join("vector_dbs");
    let _ = std::fs::create_dir_all(&dir);
    let safe: String = workspace
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    dir.join(format!("{}.bin", safe))
        .to_string_lossy()
        .into_owned()
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn update_index(workspace: &str, config: &Config, force: bool) -> bool {
    let db_path = get_db_path(workspace);
    let mut db = if !force {
        VectorDb::load_from_binary(&db_path).unwrap_or_default()
    } else {
        VectorDb::default()
    };
    db.workspace_path = workspace.to_string();

    let all_files = utils::list_directory(workspace, true);
    let registry: HashMap<String, &FileRegistry> =
        db.files.iter().map(|r| (r.file_path.clone(), r)).collect();
    let active_rel: HashSet<String> = all_files
        .iter()
        .map(|f| {
            f.strip_prefix(workspace)
                .unwrap_or(f)
                .trim_start_matches('/')
                .to_string()
        })
        .collect();

    let mut to_index: Vec<(String, String)> = Vec::new(); // (abs, rel)
    for abs_path in &all_files {
        let rel = abs_path
            .strip_prefix(workspace)
            .unwrap_or(abs_path)
            .trim_start_matches('/')
            .to_string();
        let mtime = utils::get_last_modified_time(abs_path);
        let size = utils::get_file_size(abs_path);
        let needs_index = match registry.get(&rel) {
            Some(r) => r.last_modified != mtime || r.size != size,
            None => true,
        };
        if needs_index {
            to_index.push((abs_path.clone(), rel));
        }
    }

    // Purge deleted files
    db.files.retain(|r| active_rel.contains(&r.file_path));
    db.chunks.retain(|c| active_rel.contains(&c.file_path));

    if to_index.is_empty() {
        return true;
    }

    println!("🔍 Indexing {} new/modified files...", to_index.len());
    let paths_set: HashSet<String> = to_index.iter().map(|(_, r)| r.clone()).collect();
    db.chunks.retain(|c| !paths_set.contains(&c.file_path));
    db.files.retain(|r| !paths_set.contains(&r.file_path));

    let mut new_chunks: Vec<DbChunk> = Vec::new();
    for (abs, rel) in &to_index {
        for rc in chunk_file(abs, &config.chunk_strategy) {
            new_chunks.push(DbChunk {
                file_path: rel.clone(),
                content: rc.content,
                start_line: rc.start_line,
                end_line: rc.end_line,
                embedding: Vec::new(),
            });
        }
        db.files.push(FileRegistry {
            file_path: rel.clone(),
            last_modified: utils::get_last_modified_time(abs),
            size: utils::get_file_size(abs),
        });
    }

    if !new_chunks.is_empty() {
        println!("⚡ Batch embedding {} chunks...", new_chunks.len());
        let texts: Vec<String> = new_chunks.iter().map(|c| c.content.clone()).collect();
        let batch = 50;
        let mut all_embeds: Vec<Vec<f32>> = Vec::new();
        for chunk in texts.chunks(batch) {
            let sub = fetch_embeddings(&chunk.to_vec(), config);
            all_embeds.extend(sub);
        }
        if all_embeds.len() == new_chunks.len() {
            for (i, mut e) in all_embeds.into_iter().enumerate() {
                normalize_vec(&mut e);
                new_chunks[i].embedding = e;
                db.chunks.push(new_chunks[i].clone());
            }
            println!("✅ Embedding complete.");
        } else {
            eprintln!(
                "⚠️  Embedding size mismatch: expected {}, got {}",
                new_chunks.len(),
                all_embeds.len()
            );
        }
    }

    let _ = db.save_to_binary(&db_path);
    println!("✅ Vector index stored.");
    true
}

pub fn query_context(workspace: &str, query: &str, config: &Config) -> String {
    update_index(workspace, config, false);
    let db_path = get_db_path(workspace);
    let db = match VectorDb::load_from_binary(&db_path) {
        Some(d) if !d.chunks.is_empty() => d,
        _ => return "No indexed files found.".to_string(),
    };

    let embeds = fetch_embeddings(&[query.to_string()], config);
    if embeds.is_empty() {
        return "Failed to embed search query.".to_string();
    }
    let mut qv = embeds[0].clone();
    normalize_vec(&mut qv);

    let mut scores: Vec<(usize, f32)> = db
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| c.embedding.len() == qv.len())
        .map(|(i, c)| (i, cosine_similarity(&c.embedding, &qv)))
        .collect();
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut context = String::new();
    let budget = 8000usize;
    for (i, (idx, sim)) in scores.iter().take(8).enumerate() {
        if *sim < 0.2 {
            break;
        }
        let chunk = &db.chunks[*idx];
        let entry = format!(
            "--- Chunk {} from '{}' (Lines {}-{}, Sim: {:.2}) ---\n{}\n\n",
            i + 1,
            chunk.file_path,
            chunk.start_line,
            chunk.end_line,
            sim,
            chunk.content
        );
        if context.len() + entry.len() > budget {
            break;
        }
        context.push_str(&entry);
    }
    if context.is_empty() {
        "No matching context found.".to_string()
    } else {
        context
    }
}
