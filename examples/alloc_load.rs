//! Counts the allocations each stage of a workspace load performs.
//!
//! Wall-clock timings on a shared machine carry several percent of noise, which
//! hides small changes. Allocation counts are deterministic for a given corpus,
//! so this measures the same work without depending on how busy the machine is.
//!
//! Run with: cargo run --release --example alloc_load -- <corpus-dir>

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use marksman::config::{Config, ParserSettings};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::markdown;
use marksman::misc;
use marksman::names::FolderId;
use marksman::parser;
use marksman::paths::{system_path_to_uri_string, LocalPath};
use marksman::structure::Structure;
use marksman::text::mk_text;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(layout.size(), Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `body` and reports the allocations it performed.
fn measure<T>(label: &str, body: impl FnOnce() -> T) -> T {
    let start_allocs = ALLOCS.load(Relaxed);
    let start_bytes = BYTES.load(Relaxed);
    let t = Instant::now();
    let out = body();
    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "  {label:36} {:>9} allocs {:>12} bytes {:>8.1} ms",
        ALLOCS.load(Relaxed) - start_allocs,
        BYTES.load(Relaxed) - start_bytes,
        elapsed
    );
    out
}

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
    let root = std::env::args().nth(1).expect("usage: alloc_load <corpus-dir>");

    let config = Config::default_config();
    let settings = ParserSettings::of_config(&config);
    let exts = config.core_markdown_file_extensions();
    let files = collect_markdown(&root, &exts);
    let folder_id = FolderId::of_uri(system_path_to_uri_string(&root));
    println!("corpus: {} files", files.len());

    let contents = measure("disk read", || {
        files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect::<Vec<_>>()
    });

    println!("\nparse pipeline, each stage on its own pass:");
    measure("markdown::scan (block+inline)", || {
        contents.iter().map(|c| markdown::scan(c).links.len()).sum::<usize>()
    });

    let texts =
        measure("mk_text (line map)", || contents.iter().map(|c| mk_text(c)).collect::<Vec<_>>());

    let scraped = measure("scrape_text (includes scan)", || {
        texts.iter().map(|t| parser::scrape_text(&settings, t)).collect::<Vec<_>>()
    });

    let csts = measure("build_cst (scopes, child map)", || {
        texts
            .iter()
            .zip(scraped)
            .map(|(text, elements)| parser::build_cst(text, elements))
            .collect::<Vec<_>>()
    });

    measure("Structure::of_cst (AST, syms)", || {
        csts.into_iter().map(|c| Structure::of_cst(&settings, c).symbols().len()).sum::<usize>()
    });
    drop(texts);
    drop(contents);

    println!("\nwhat the server actually runs:");
    let docs = measure("Doc::try_load (read+parse+index)", || {
        files
            .iter()
            .filter_map(|f| Doc::try_load(&settings, &folder_id, &LocalPath::of_system(f)))
            .collect::<Vec<_>>()
    });

    let folder = measure("Folder::multi_file (map+lookup+conn)", || {
        Folder::multi_file("alloc".to_string(), folder_id.clone(), docs, None)
    });

    let doc = folder.docs().into_iter().next().unwrap();
    let updated = doc.with_text(&settings, doc.text().clone());
    measure("one-doc folder rebuild", || folder.with_doc(updated.clone()));
}
