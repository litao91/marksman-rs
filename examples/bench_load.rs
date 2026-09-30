//! Times the phases of loading a workspace folder.
//!
//! Run with: cargo run --release --example bench_load -- <corpus-dir> [repeats]

use std::time::Instant;

use marksman::config::{Config, ParserSettings};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::misc;
use marksman::names::FolderId;
use marksman::paths::{system_path_to_uri_string, LocalPath};

fn collect_markdown(root: &str, exts: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_string()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let s = path.to_string_lossy().to_string();
            if path.is_dir() {
                stack.push(s);
            } else if misc::is_markdown_file(exts, &s) {
                out.push(s);
            }
        }
    }

    out.sort();
    out
}

fn main() {
    let root = std::env::args().nth(1).expect("usage: bench_load <corpus-dir> [repeats]");
    let repeats: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let config = Config::default_config();
    let settings = ParserSettings::of_config(&config);
    let exts = config.core_markdown_file_extensions();

    let files = collect_markdown(&root, &exts);
    let total_bytes: u64 = files.iter().map(|f| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)).sum();
    println!(
        "corpus: {} files, {:.2} MB",
        files.len(),
        total_bytes as f64 / 1e6
    );

    let folder_id = FolderId::of_uri(system_path_to_uri_string(&root));

    let mut read_parse_best = f64::MAX;
    let mut index_best = f64::MAX;
    let mut total_best = f64::MAX;

    for _ in 0..repeats {
        // Phase 1: read from disk and parse into documents.
        let t = Instant::now();
        let docs: Vec<Doc> = files
            .iter()
            .filter_map(|f| Doc::try_load(&settings, &folder_id, &LocalPath::of_system(f)))
            .collect();
        let read_parse = t.elapsed().as_secs_f64() * 1000.0;

        // Phase 2: build the lookup indexes and the connection graph.
        let t = Instant::now();
        let folder = Folder::multi_file("bench".to_string(), folder_id.clone(), docs, None);
        let index = t.elapsed().as_secs_f64() * 1000.0;

        // Phase 3: a full graph rebuild, which is what a document edit triggers
        // under the default configuration.
        let t = Instant::now();
        let doc = folder.docs().into_iter().next().unwrap();
        let updated = doc.with_text(&settings, doc.text().clone());
        let _ = folder.with_doc(updated);
        let rebuild = t.elapsed().as_secs_f64() * 1000.0;

        let total = read_parse + index;
        read_parse_best = read_parse_best.min(read_parse);
        index_best = index_best.min(index);
        total_best = total_best.min(total);

        println!(
            "  read+parse {:8.1} ms | lookup+conn {:8.1} ms | load total {:8.1} ms | one-doc rebuild {:8.1} ms",
            read_parse, index, total, rebuild
        );
    }

    println!(
        "\nbest of {repeats}: read+parse {:.1} ms, lookup+conn {:.1} ms, load total {:.1} ms",
        read_parse_best, index_best, total_best
    );
    println!(
        "throughput: {:.1} MB/s parse, {:.0} docs/s load",
        (total_bytes as f64 / 1e6) / (read_parse_best / 1000.0),
        files.len() as f64 / (total_best / 1000.0)
    );
}
