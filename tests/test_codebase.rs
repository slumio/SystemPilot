/// Tests for src/codebase.rs
/// Covers: cosine similarity, vector normalization, file chunking, VectorDb
/// binary round-trip.
use syspilot::codebase::{self, DbChunk, FileRegistry, VectorDb};

// ── cosine_similarity ─────────────────────────────────────────────────────────

#[test]
fn cosine_identical_vectors_is_one() {
    let v = vec![1.0f32, 2.0, 3.0];
    let sim = codebase::cosine_similarity(&v, &v);
    assert!(
        (sim - 1.0).abs() < 1e-5,
        "identical vectors should have similarity 1.0, got {}",
        sim
    );
}

#[test]
fn cosine_opposite_vectors_is_minus_one() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![-1.0f32, 0.0, 0.0];
    let sim = codebase::cosine_similarity(&a, &b);
    assert!(
        (sim + 1.0).abs() < 1e-5,
        "opposite vectors should have similarity -1.0, got {}",
        sim
    );
}

#[test]
fn cosine_orthogonal_vectors_is_zero() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0];
    let sim = codebase::cosine_similarity(&a, &b);
    assert!(
        sim.abs() < 1e-5,
        "orthogonal vectors should have similarity 0, got {}",
        sim
    );
}

#[test]
fn cosine_zero_vector_returns_zero() {
    let a = vec![0.0f32, 0.0, 0.0];
    let b = vec![1.0f32, 2.0, 3.0];
    let sim = codebase::cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0);
}

#[test]
fn cosine_empty_vectors_return_zero() {
    let sim = codebase::cosine_similarity(&[], &[]);
    assert_eq!(sim, 0.0);
}

#[test]
fn cosine_mismatched_lengths_return_zero() {
    let a = vec![1.0f32, 2.0];
    let b = vec![1.0f32, 2.0, 3.0];
    let sim = codebase::cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0);
}

#[test]
fn cosine_similarity_symmetry() {
    let a = vec![0.5f32, 0.8, 0.1];
    let b = vec![0.3f32, 0.9, 0.4];
    let sim_ab = codebase::cosine_similarity(&a, &b);
    let sim_ba = codebase::cosine_similarity(&b, &a);
    assert!(
        (sim_ab - sim_ba).abs() < 1e-6,
        "cosine similarity must be symmetric"
    );
}

#[test]
fn cosine_result_in_range_minus_one_to_one() {
    let a = vec![3.0f32, -1.0, 2.0, 0.5];
    let b = vec![-2.0f32, 4.0, -0.5, 1.0];
    let sim = codebase::cosine_similarity(&a, &b);
    assert!(
        (-1.0..=1.0).contains(&sim),
        "cosine similarity must be in [-1, 1], got {}",
        sim
    );
}

// ── normalize_vec ─────────────────────────────────────────────────────────────

#[test]
fn normalize_unit_vector_unchanged() {
    let mut v = vec![1.0f32, 0.0, 0.0];
    codebase::normalize_vec(&mut v);
    assert!((v[0] - 1.0).abs() < 1e-6);
    assert!(v[1].abs() < 1e-6);
    assert!(v[2].abs() < 1e-6);
}

#[test]
fn normalize_produces_unit_norm() {
    let mut v = vec![3.0f32, 4.0]; // norm = 5
    codebase::normalize_vec(&mut v);
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-6,
        "norm after normalisation should be 1.0, got {}",
        norm
    );
}

#[test]
fn normalize_zero_vector_unchanged() {
    let mut v = vec![0.0f32, 0.0, 0.0];
    codebase::normalize_vec(&mut v);
    assert!(v.iter().all(|&x| x == 0.0));
}

// ── chunk_file ────────────────────────────────────────────────────────────────

fn write_temp_file(content: &str, ext: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::Builder::new().suffix(ext).tempfile().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

#[test]
fn chunk_file_rejects_binary() {
    // File containing null bytes is treated as binary and returns empty
    let mut f = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
    use std::io::Write;
    f.write_all(b"hello\x00world").unwrap();
    let path = f.path().to_str().unwrap().to_string();
    let chunks = codebase::chunk_file(&path, "syntactic");
    assert!(chunks.is_empty(), "binary file should produce no chunks");
}

#[test]
fn chunk_file_sliding_window_produces_chunks() {
    let src = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n".repeat(6); // 60 lines
    let f = write_temp_file(&src, ".txt");
    let path = f.path().to_str().unwrap().to_string();
    let chunks = codebase::chunk_file(&path, "line");
    assert!(!chunks.is_empty(), "should produce at least one chunk");
    // Each chunk's start_line should be <= end_line
    for c in &chunks {
        assert!(c.start_line <= c.end_line, "start_line must be <= end_line");
        assert!(!c.content.is_empty(), "chunk content must not be empty");
    }
}

#[test]
fn chunk_file_syntactic_rust_splits_on_fn() {
    let src = r#"
fn alpha() {
    let x = 1;
}

fn beta() {
    let y = 2;
}
"#;
    let f = write_temp_file(src, ".rs");
    let path = f.path().to_str().unwrap().to_string();
    let chunks = codebase::chunk_file(&path, "syntactic");
    // Should produce at least one chunk
    assert!(
        !chunks.is_empty(),
        "syntactic chunker must produce chunks for valid Rust"
    );
    // The chunks collectively should cover the entire file
    let total_lines: u32 = chunks.iter().map(|c| c.end_line - c.start_line + 1).sum();
    let file_lines = src.lines().count() as u32;
    assert!(
        total_lines >= file_lines.saturating_sub(2),
        "chunks should cover most of the file ({} total chunk lines vs {} file lines)",
        total_lines,
        file_lines
    );
}

#[test]
fn chunk_file_markdown_splits_on_headings() {
    let src =
        "# Section 1\nContent 1.\n\n# Section 2\nContent 2.\n\n## Subsection\nMore content.\n";
    let f = write_temp_file(src, ".md");
    let path = f.path().to_str().unwrap().to_string();
    let chunks = codebase::chunk_file(&path, "syntactic");
    // Markdown: splits on '#' lines — expect at least one chunk
    assert!(!chunks.is_empty());
}

#[test]
fn chunk_file_line_numbers_are_one_indexed() {
    let src = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n".repeat(5); // 50 lines
    let f = write_temp_file(&src, ".txt");
    let path = f.path().to_str().unwrap().to_string();
    let chunks = codebase::chunk_file(&path, "line");
    for c in &chunks {
        assert!(
            c.start_line >= 1,
            "start_line must be >= 1, got {}",
            c.start_line
        );
    }
    // First chunk must start at line 1
    assert_eq!(chunks[0].start_line, 1);
}

#[test]
fn chunk_file_skips_large_files() {
    // Create a file larger than 1 MB
    let f = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
    let path = f.path().to_str().unwrap().to_string();
    let big_content = "x".repeat(1024 * 1024 + 1);
    std::fs::write(&path, &big_content).unwrap();
    let chunks = codebase::chunk_file(&path, "syntactic");
    assert!(chunks.is_empty(), "files > 1 MB should be skipped");
}

// ── VectorDb binary round-trip ────────────────────────────────────────────────

fn sample_db() -> VectorDb {
    VectorDb {
        workspace_path: "/workspace/test".to_string(),
        embedding_source: "ollama:http://localhost:11434:qwen3-embedding:0.6b".to_string(),
        files: vec![FileRegistry {
            file_path: "src/main.rs".to_string(),
            last_modified: 1_700_000_000,
            size: 1024,
        }],
        chunks: vec![DbChunk {
            file_path: "src/main.rs".to_string(),
            content: "fn main() { println!(\"hello\"); }".to_string(),
            start_line: 1,
            end_line: 3,
            embedding: vec![0.1f32, 0.5, -0.3, 0.8],
        }],
    }
}

#[test]
fn vector_db_save_and_load_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();

    let db = sample_db();
    db.save_to_binary(&path).expect("save_to_binary failed");

    let loaded = VectorDb::load_from_binary(&path).expect("load_from_binary failed");

    assert_eq!(loaded.workspace_path, "/workspace/test");
    assert_eq!(loaded.files.len(), 1);
    assert_eq!(loaded.files[0].file_path, "src/main.rs");
    assert_eq!(loaded.files[0].last_modified, 1_700_000_000);
    assert_eq!(loaded.chunks.len(), 1);
    assert_eq!(
        loaded.chunks[0].content,
        "fn main() { println!(\"hello\"); }"
    );
    assert_eq!(loaded.chunks[0].start_line, 1);
    assert_eq!(loaded.chunks[0].end_line, 3);
    assert_eq!(loaded.chunks[0].embedding.len(), 4);
    for (a, b) in db.chunks[0]
        .embedding
        .iter()
        .zip(loaded.chunks[0].embedding.iter())
    {
        assert!(
            (a - b).abs() < 1e-7,
            "embedding value mismatch: {} vs {}",
            a,
            b
        );
    }
}

#[test]
fn vector_db_wrong_magic_returns_none() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    std::fs::write(&path, b"WRONG_MAGIC_HEADER_12345").unwrap();
    assert!(VectorDb::load_from_binary(&path).is_none());
}

#[test]
fn vector_db_missing_file_returns_none() {
    assert!(VectorDb::load_from_binary("/no/such/file/xyz_syspilot_test.bin").is_none());
}

#[test]
fn vector_db_empty_db_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();

    let empty = VectorDb::default();
    empty.save_to_binary(&path).unwrap();
    let loaded = VectorDb::load_from_binary(&path).unwrap();

    assert_eq!(loaded.workspace_path, "");
    assert!(loaded.files.is_empty());
    assert!(loaded.chunks.is_empty());
}

#[test]
fn vector_db_multiple_chunks_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();

    let mut db = VectorDb {
        workspace_path: "/repo".to_string(),
        ..Default::default()
    };
    for i in 0..10u32 {
        db.chunks.push(DbChunk {
            file_path: format!("file_{}.rs", i),
            content: format!("content {}", i),
            start_line: i * 10 + 1,
            end_line: i * 10 + 10,
            embedding: vec![i as f32 * 0.1, i as f32 * 0.2],
        });
    }

    db.save_to_binary(&path).unwrap();
    let loaded = VectorDb::load_from_binary(&path).unwrap();

    assert_eq!(loaded.chunks.len(), 10);
    for (i, chunk) in loaded.chunks.iter().enumerate() {
        assert_eq!(chunk.file_path, format!("file_{}.rs", i));
        assert_eq!(chunk.start_line, i as u32 * 10 + 1);
    }
}
