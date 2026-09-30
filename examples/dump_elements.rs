//! Dumps this port's CST elements for every `*.md` file in a directory, in the
//! same format as `bench/oracle/dump_fs.fsx`.
//!
//! Usage: cargo run --release --example dump_elements -- <case-dir>

use marksman::config::ParserSettings;
use marksman::cst::Element;
use marksman::misc::lines_of;
use marksman::parser;
use marksman::text::mk_text;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: dump_elements <case-dir>");

    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {dir}: {e}"))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();

    for path in paths {
        println!("===== {}", path.file_name().unwrap().to_string_lossy());

        let content = std::fs::read_to_string(&path).unwrap();
        let text = mk_text(&content);
        let structure = parser::parse(&ParserSettings::default(), &text);
        let elements = structure.concrete_elements();

        for element in elements {
            for line in lines_of(&Element::fmt(element)) {
                println!("{line}");
            }
        }
    }
}
