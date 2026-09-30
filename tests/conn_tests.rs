//! Port of `Tests/ConnTest.fs`.
//!
//! The original uses Snapper snapshots of `Conn.CompactFormat()`. The Rust port
//! has no whole-graph formatter (only `ConnDifference::compact_format` and
//! `Unresolved::compact_format`), so `conn_compact_format` below rebuilds the
//! same text from public state and the expected lines are kept verbatim from
//! `Tests/_snapshots/ConnGraphTests.json`.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use common::{FakeDoc, FakeFolder};
use marksman::config::{ComplWikiStyle, Config};
use marksman::conn::{
    CandidateDocumentResolution, Conn, ConnDifference, DefinitionSelector, Oracle, OracleApi,
};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::misc;
use marksman::mmap::MMap;
use marksman::names::{DocId, InternName};
use marksman::syms::{Def, Scope, ScopedSym, Sym};

/// Reproduces `new System.Random(seed)` from .NET. Seeded `Random` instances
/// still use the legacy Knuth subtractive generator in .NET 6 and later, which
/// is what the original `editSequences` theory relies on.
struct DotNetRandom {
    seed_array: [i32; 56],
    inext: usize,
    inext_p: usize,
}

impl DotNetRandom {
    const MBIG: i32 = i32::MAX;
    const MSEED: i32 = 161803398;

    fn new(seed: i32) -> DotNetRandom {
        let mut seed_array = [0i32; 56];
        let subtraction = if seed == i32::MIN { i32::MAX } else { seed.abs() };
        let mut mj = DotNetRandom::MSEED - subtraction;
        seed_array[55] = mj;
        let mut mk = 1;
        for i in 1..55usize {
            let ii = (21 * i) % 55;
            seed_array[ii] = mk;
            mk = mj - mk;
            if mk < 0 {
                mk += DotNetRandom::MBIG;
            }
            mj = seed_array[ii];
        }
        for _k in 1..5 {
            for i in 1..56usize {
                seed_array[i] -= seed_array[1 + ((i + 30) % 55)];
                if seed_array[i] < 0 {
                    seed_array[i] += DotNetRandom::MBIG;
                }
            }
        }
        DotNetRandom { seed_array, inext: 0, inext_p: 21 }
    }

    fn sample(&mut self) -> f64 {
        let mut loc_inext = self.inext + 1;
        if loc_inext >= 56 {
            loc_inext = 1;
        }
        let mut loc_inext_p = self.inext_p + 1;
        if loc_inext_p >= 56 {
            loc_inext_p = 1;
        }

        let mut ret_val = self.seed_array[loc_inext] - self.seed_array[loc_inext_p];
        if ret_val == DotNetRandom::MBIG {
            ret_val -= 1;
        }
        if ret_val < 0 {
            ret_val += DotNetRandom::MBIG;
        }

        self.seed_array[loc_inext] = ret_val;
        self.inext = loc_inext;
        self.inext_p = loc_inext_p;

        ret_val as f64 * (1.0 / DotNetRandom::MBIG as f64)
    }

    /// `Random.Next(maxValue)`, i.e. a value in `[0, maxValue)`.
    fn next(&mut self, max_value: i32) -> i32 {
        (self.sample() * max_value as f64) as i32
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// `Indented(n, x).ToString()` from `Marksman.Misc`, as lines. An empty block
/// still yields one line of `n` spaces, because `"".Lines()` is `[""]`.
fn indent_block(n: usize, inner: &[String]) -> Vec<String> {
    let pad = " ".repeat(n);
    if inner.is_empty() {
        vec![pad]
    } else {
        inner.iter().map(|line| format!("{pad}{line}")).collect()
    }
}

fn is_ref(sym: &Sym) -> bool {
    sym.as_ref().is_some()
}

fn is_def(sym: &Sym) -> bool {
    sym.as_def().is_some()
}

fn is_tag(sym: &Sym) -> bool {
    sym.as_tag().is_some()
}

fn symbols_grouped(
    conn: &Conn,
    keep: fn(&Sym) -> bool,
) -> Vec<(Scope, Vec<String>)> {
    let mut by_scope: BTreeMap<Scope, BTreeSet<Sym>> = BTreeMap::new();
    for (scope, sym) in conn.symbols().to_seq() {
        if keep(&sym) {
            by_scope.entry(scope).or_default().insert(sym);
        }
    }
    by_scope
        .into_iter()
        .map(|(scope, syms)| (scope, syms.iter().map(|s| s.to_string()).collect()))
        .collect()
}

/// Rebuilds `Conn.CompactFormat()`. Refs/Defs/Tags come from the public symbol
/// map; the Resolved and Unresolved sections are read back through
/// `Conn::difference(conn, Conn::empty())`, which exposes the same two graphs
/// the original formats from its private `computedValues`.
fn conn_compact_format(conn: &Conn) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    for (header, keep) in [
        ("Refs:", is_ref as fn(&Sym) -> bool),
        ("Defs:", is_def as fn(&Sym) -> bool),
        ("Tags:", is_tag as fn(&Sym) -> bool),
    ] {
        let grouped = symbols_grouped(conn, keep);
        if !grouped.is_empty() {
            lines.push(header.to_string());
            for (scope, syms) in grouped {
                lines.push(format!("  {scope}:"));
                for sym in syms {
                    lines.push(format!("    {sym}"));
                }
            }
        }
    }

    // Comparing against the empty graph yields exactly the edges the original
    // reads out of its private computed-value map.
    let diff = Conn::difference(conn, &Conn::empty());

    let mut edges: BTreeMap<ScopedSym, BTreeSet<ScopedSym>> = BTreeMap::new();
    for (src, dst) in &diff.resolved_difference.edge_difference.removed {
        edges.entry(src.clone()).or_default().insert(dst.clone());
    }

    let mut resolved_inner: Vec<String> = Vec::new();
    let mut current_scope: Option<Scope> = None;
    for ((scope, sym), targets) in &edges {
        if current_scope.as_ref() != Some(scope) {
            resolved_inner.push(format!("{scope}:"));
            current_scope = Some(scope.clone());
        }
        for (target_scope, target_sym) in targets {
            resolved_inner.push(format!("  {sym} -> {target_sym} @ {target_scope}"));
        }
    }

    let unresolved_inner: Vec<String> = diff
        .unresolved_difference
        .edge_difference
        .removed
        .iter()
        .map(|(src, dst)| format!("{} -> {}", src.compact_format(), dst.compact_format()))
        .collect();

    lines.push("Resolved:".to_string());
    lines.extend(indent_block(2, &resolved_inner));
    lines.push("Unresolved:".to_string());
    lines.extend(indent_block(2, &unresolved_inner));

    lines
}

/// `checkSnapshot`: `conn.CompactFormat().Lines().ShouldMatchSnapshot()`.
fn check_snapshot(conn: &Conn, expected: &[&str]) {
    let actual = conn_compact_format(conn);
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(expected, actual);
}

/// `checkInlineSnapshot id [ connDiff.CompactFormat() ] [ "" ]`.
fn check_empty_difference(diff: &ConnDifference) {
    assert_eq!(vec![""], misc::lines_of(&diff.compact_format()));
    assert!(diff.is_empty(), "{}", diff.compact_format());
}

/// The incremental graph must have the same observable results and internal
/// connection state as a clean construction (`ConnDependencyTests`).
fn assert_matches_clean_construction(folder: &Folder) {
    let expected = Conn::mk(&folder.oracle(), &folder.syms());
    let difference = Conn::difference(&expected, folder.conn());
    assert!(difference.is_empty(), "{}", difference.compact_format());
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct EmptyOracle;

impl OracleApi for EmptyOracle {
    fn resolve_candidate_documents(&self, _name: &InternName) -> CandidateDocumentResolution {
        CandidateDocumentResolution::default()
    }

    fn select_definitions(&self, _selector: &DefinitionSelector) -> Vec<Def> {
        Vec::new()
    }
}

fn empty_oracle() -> Oracle {
    Arc::new(EmptyOracle)
}

fn incr_config() -> Config {
    Config { core_incremental_references: Some(true), ..Config::default_config() }
}

fn mk_folder(docs: Vec<Doc>) -> Folder {
    FakeFolder::mk_with_config(docs, Some(incr_config()))
}

fn fake_doc(path: &str, lines: &[&str]) -> Doc {
    FakeDoc::mk_with(&lines.join("\n"), path, None, None)
}

fn fake_doc_config(path: &str, lines: &[&str], config: &Config) -> Doc {
    FakeDoc::mk_with(&lines.join("\n"), path, None, Some(config.clone()))
}

fn d1() -> Doc {
    fake_doc("d1.md", &["# Doc 1", "", "## D1.S1", "", "[[doc-2]]", "[[doc-NA]]", ""])
}

fn d1_dup() -> Doc {
    fake_doc("d1_dup.md", &["# Doc 1", "", "[[doc-2]]", ""])
}

fn d2() -> Doc {
    fake_doc("d2.md", &["# Doc 2", "[[doc-1]]", "[[doc-1#d1s1]]", "[[doc-NA2]]", ""])
}

fn d3() -> Doc {
    fake_doc(
        "d3.md",
        &["# Doc 3", "", "[link1]", "[link1][]", "[linkX]", "", "[link1]: /url1", ""],
    )
}

// ---------------------------------------------------------------------------
// ConnGraphTests
// ---------------------------------------------------------------------------

/// `ConnGraphTests.emptyGraph`
#[test]
fn empty_graph() {
    let oracle = empty_oracle();
    let conn = Conn::mk(&oracle, &MMap::<DocId, Sym>::empty());
    check_snapshot(&conn, &["Resolved:", "  ", "Unresolved:", "  "]);
}

/// `ConnGraphTests.initGraph`
#[test]
fn init_graph() {
    let f = mk_folder(vec![d1(), d1_dup(), d2(), d3()]);
    let conn = Conn::mk(&f.oracle(), &f.syms());
    check_snapshot(
        &conn,
        &[
            "Refs:",
            "  d1.md:",
            "    [[doc-2]]",
            "    [[doc-NA]]",
            "  d1_dup.md:",
            "    [[doc-2]]",
            "  d2.md:",
            "    [[doc-1]]",
            "    [[doc-NA2]]",
            "    [[doc-1#d1s1]]",
            "  d3.md:",
            "    [link1]",
            "    [linkx]",
            "Defs:",
            "  d1.md:",
            "    Doc",
            "    T {doc-1}",
            "    H2 {d1s1}",
            "  d1_dup.md:",
            "    Doc",
            "    T {doc-1}",
            "  d2.md:",
            "    Doc",
            "    T {doc-2}",
            "  d3.md:",
            "    Doc",
            "    T {doc-3}",
            "    [link1]:",
            "Resolved:",
            "  d1.md:",
            "    T {doc-1} -> [[doc-1]] @ d2.md",
            "    H2 {d1s1} -> [[doc-1#d1s1]] @ d2.md",
            "    [[doc-2]] -> T {doc-2} @ d2.md",
            "  d1_dup.md:",
            "    T {doc-1} -> [[doc-1]] @ d2.md",
            "    [[doc-2]] -> T {doc-2} @ d2.md",
            "  d2.md:",
            "    T {doc-2} -> [[doc-2]] @ d1.md",
            "    T {doc-2} -> [[doc-2]] @ d1_dup.md",
            "    [[doc-1]] -> T {doc-1} @ d1.md",
            "    [[doc-1]] -> T {doc-1} @ d1_dup.md",
            "    [[doc-1#d1s1]] -> H2 {d1s1} @ d1.md",
            "  d3.md:",
            "    [link1]: -> [link1] @ d3.md",
            "    [link1] -> [link1]: @ d3.md",
            "Unresolved:",
            "  [[doc-NA]] @ d1.md -> FullyUnknown",
            "  [[doc-NA2]] @ d2.md -> FullyUnknown",
            "  [[doc-1#d1s1]] @ d2.md -> d1_dup.md",
            "  [linkx] @ d3.md -> d3.md",
            "  FullyUnknown -> [[doc-NA]] @ d1.md",
            "  FullyUnknown -> [[doc-NA2]] @ d2.md",
            "  d1_dup.md -> [[doc-1#d1s1]] @ d2.md",
            "  d3.md -> [linkx] @ d3.md",
        ],
    );
}

/// `ConnGraphTests.removeDoc`
#[test]
fn remove_doc() {
    let f1 = mk_folder(vec![d1(), d1_dup(), d2(), d3()]);
    let f2 = f1.without_doc(d1_dup().id()).unwrap();

    check_snapshot(
        f2.conn(),
        &[
            "Refs:",
            "  d1.md:",
            "    [[doc-2]]",
            "    [[doc-NA]]",
            "  d2.md:",
            "    [[doc-1]]",
            "    [[doc-NA2]]",
            "    [[doc-1#d1s1]]",
            "  d3.md:",
            "    [link1]",
            "    [linkx]",
            "Defs:",
            "  d1.md:",
            "    Doc",
            "    T {doc-1}",
            "    H2 {d1s1}",
            "  d2.md:",
            "    Doc",
            "    T {doc-2}",
            "  d3.md:",
            "    Doc",
            "    T {doc-3}",
            "    [link1]:",
            "Resolved:",
            "  d1.md:",
            "    T {doc-1} -> [[doc-1]] @ d2.md",
            "    H2 {d1s1} -> [[doc-1#d1s1]] @ d2.md",
            "    [[doc-2]] -> T {doc-2} @ d2.md",
            "  d2.md:",
            "    T {doc-2} -> [[doc-2]] @ d1.md",
            "    [[doc-1]] -> T {doc-1} @ d1.md",
            "    [[doc-1#d1s1]] -> H2 {d1s1} @ d1.md",
            "  d3.md:",
            "    [link1]: -> [link1] @ d3.md",
            "    [link1] -> [link1]: @ d3.md",
            "Unresolved:",
            "  [[doc-NA]] @ d1.md -> FullyUnknown",
            "  [[doc-NA2]] @ d2.md -> FullyUnknown",
            "  [linkx] @ d3.md -> d3.md",
            "  FullyUnknown -> [[doc-NA]] @ d1.md",
            "  FullyUnknown -> [[doc-NA2]] @ d2.md",
            "  d3.md -> [linkx] @ d3.md",
        ],
    );
}

/// `ConnGraphTests.addDoc`
#[test]
fn add_doc() {
    let f1 = mk_folder(vec![d1(), d2(), d3()]);

    let d_na = fake_doc("docNA.md", &["# Doc NA", ""]);

    let f2 = f1.with_doc(d_na);
    check_snapshot(
        f2.conn(),
        &[
            "Refs:",
            "  d1.md:",
            "    [[doc-2]]",
            "    [[doc-NA]]",
            "  d2.md:",
            "    [[doc-1]]",
            "    [[doc-NA2]]",
            "    [[doc-1#d1s1]]",
            "  d3.md:",
            "    [link1]",
            "    [linkx]",
            "Defs:",
            "  d1.md:",
            "    Doc",
            "    T {doc-1}",
            "    H2 {d1s1}",
            "  d2.md:",
            "    Doc",
            "    T {doc-2}",
            "  d3.md:",
            "    Doc",
            "    T {doc-3}",
            "    [link1]:",
            "  docNA.md:",
            "    Doc",
            "    T {doc-na}",
            "Resolved:",
            "  d1.md:",
            "    T {doc-1} -> [[doc-1]] @ d2.md",
            "    H2 {d1s1} -> [[doc-1#d1s1]] @ d2.md",
            "    [[doc-2]] -> T {doc-2} @ d2.md",
            "    [[doc-NA]] -> T {doc-na} @ docNA.md",
            "  d2.md:",
            "    T {doc-2} -> [[doc-2]] @ d1.md",
            "    [[doc-1]] -> T {doc-1} @ d1.md",
            "    [[doc-1#d1s1]] -> H2 {d1s1} @ d1.md",
            "  d3.md:",
            "    [link1]: -> [link1] @ d3.md",
            "    [link1] -> [link1]: @ d3.md",
            "  docNA.md:",
            "    T {doc-na} -> [[doc-NA]] @ d1.md",
            "Unresolved:",
            "  [[doc-NA2]] @ d2.md -> FullyUnknown",
            "  [linkx] @ d3.md -> d3.md",
            "  FullyUnknown -> [[doc-NA2]] @ d2.md",
            "  d3.md -> [linkx] @ d3.md",
        ],
    );
}

/// `ConnGraphTests.addLinkDef`
#[test]
fn add_link_def() {
    let f1 = mk_folder(vec![d1(), d2(), d3()]);

    let d3_update = fake_doc(
        "d3.md",
        &[
            "# Doc 3",
            "",
            "[link1]",
            "[link1][]",
            "[linkX]",
            "",
            "[link1]: /url1",
            "[linkX]: /url2",
            "",
        ],
    );

    let f2 = f1.with_doc(d3_update);
    check_snapshot(
        f2.conn(),
        &[
            "Refs:",
            "  d1.md:",
            "    [[doc-2]]",
            "    [[doc-NA]]",
            "  d2.md:",
            "    [[doc-1]]",
            "    [[doc-NA2]]",
            "    [[doc-1#d1s1]]",
            "  d3.md:",
            "    [link1]",
            "    [linkx]",
            "Defs:",
            "  d1.md:",
            "    Doc",
            "    T {doc-1}",
            "    H2 {d1s1}",
            "  d2.md:",
            "    Doc",
            "    T {doc-2}",
            "  d3.md:",
            "    Doc",
            "    T {doc-3}",
            "    [link1]:",
            "    [linkx]:",
            "Resolved:",
            "  d1.md:",
            "    T {doc-1} -> [[doc-1]] @ d2.md",
            "    H2 {d1s1} -> [[doc-1#d1s1]] @ d2.md",
            "    [[doc-2]] -> T {doc-2} @ d2.md",
            "  d2.md:",
            "    T {doc-2} -> [[doc-2]] @ d1.md",
            "    [[doc-1]] -> T {doc-1} @ d1.md",
            "    [[doc-1#d1s1]] -> H2 {d1s1} @ d1.md",
            "  d3.md:",
            "    [link1]: -> [link1] @ d3.md",
            "    [linkx]: -> [linkx] @ d3.md",
            "    [link1] -> [link1]: @ d3.md",
            "    [linkx] -> [linkx]: @ d3.md",
            "Unresolved:",
            "  [[doc-NA]] @ d1.md -> FullyUnknown",
            "  [[doc-NA2]] @ d2.md -> FullyUnknown",
            "  FullyUnknown -> [[doc-NA]] @ d1.md",
            "  FullyUnknown -> [[doc-NA2]] @ d2.md",
        ],
    );
}

/// `ConnGraphTests.removeHeading`
#[test]
fn remove_heading() {
    let f1 = mk_folder(vec![d1(), d2(), d3()]);

    let d1_update = fake_doc("d1.md", &["# Doc 1", "", "[[doc-2]]", "[[doc-NA]]", ""]);

    let f2 = f1.with_doc(d1_update);
    check_snapshot(
        f2.conn(),
        &[
            "Refs:",
            "  d1.md:",
            "    [[doc-2]]",
            "    [[doc-NA]]",
            "  d2.md:",
            "    [[doc-1]]",
            "    [[doc-NA2]]",
            "    [[doc-1#d1s1]]",
            "  d3.md:",
            "    [link1]",
            "    [linkx]",
            "Defs:",
            "  d1.md:",
            "    Doc",
            "    T {doc-1}",
            "  d2.md:",
            "    Doc",
            "    T {doc-2}",
            "  d3.md:",
            "    Doc",
            "    T {doc-3}",
            "    [link1]:",
            "Resolved:",
            "  d1.md:",
            "    T {doc-1} -> [[doc-1]] @ d2.md",
            "    [[doc-2]] -> T {doc-2} @ d2.md",
            "  d2.md:",
            "    T {doc-2} -> [[doc-2]] @ d1.md",
            "    [[doc-1]] -> T {doc-1} @ d1.md",
            "  d3.md:",
            "    [link1]: -> [link1] @ d3.md",
            "    [link1] -> [link1]: @ d3.md",
            "Unresolved:",
            "  [[doc-NA]] @ d1.md -> FullyUnknown",
            "  [[doc-NA2]] @ d2.md -> FullyUnknown",
            "  [[doc-1#d1s1]] @ d2.md -> d1.md",
            "  [linkx] @ d3.md -> d3.md",
            "  FullyUnknown -> [[doc-NA]] @ d1.md",
            "  FullyUnknown -> [[doc-NA2]] @ d2.md",
            "  d1.md -> [[doc-1#d1s1]] @ d2.md",
            "  d3.md -> [linkx] @ d3.md",
        ],
    );
}

/// `ConnGraphTests.removeTitle_PARANOID`
#[test]
fn remove_title_paranoid() {
    let d1 = fake_doc("ocaml.md", &["# OCaml", "", "## Multicore", "[[#Multicore]]"]);

    let d1_upd = fake_doc("ocaml.md", &["", "## Multicore"]);

    let d2 = fake_doc("fsharp.md", &["# FSharp", "[[OCaml#Multicore]]"]);

    let config = Config {
        compl_wiki_style: Some(ComplWikiStyle::FilePathStem),
        core_paranoid: Some(true),
        ..incr_config()
    };

    let incr = FakeFolder::mk_with_config(vec![d1.clone(), d2.clone()], Some(config.clone()))
        .with_doc(d1_upd.clone());
    let incr = incr.conn().clone();

    let from_scratch =
        FakeFolder::mk_with_config(vec![d1_upd, d2], Some(config)).conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);

    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.addTitle_SameAsFileName`
#[test]
fn add_title_same_as_file_name() {
    let d1 = fake_doc("d1.md", &["[[d2]]"]);
    let d2 = fake_doc("d2.md", &["Some content"]);
    let d3 = fake_doc("d3.md", &["# D3"]);

    let d3_upd = fake_doc("d3.md", &["# D2"]);

    let incr = mk_folder(vec![d1.clone(), d2.clone(), d3]).with_doc(d3_upd.clone());
    let incr = incr.conn().clone();

    let from_scratch = mk_folder(vec![d1, d2, d3_upd]);
    let from_scratch = from_scratch.conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);
    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.addTitle_CrossSection`
#[test]
fn add_title_cross_section() {
    let d1 = fake_doc("d1.md", &["# D1", "[[d2#sub]]"]);

    let d2 = fake_doc("d2.md", &["# D2", "## Sub"]);

    let d1_upd = fake_doc("d1.md", &["# D2", "[[d2#sub]]"]);

    let incr = mk_folder(vec![d1, d2.clone()]).with_doc(d1_upd.clone());
    let incr = incr.conn().clone();

    let from_scratch = mk_folder(vec![d1_upd, d2]);
    let from_scratch = from_scratch.conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);
    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.fixRef`
#[test]
fn fix_ref() {
    let d1 = fake_doc("d1.md", &["[[#Lnk]]", "## Link"]);

    let d1_upd = fake_doc("d1.md", &["[[#Link]]", "## Link"]);

    let incr = mk_folder(vec![d1]).with_doc(d1_upd.clone());
    let incr = incr.conn().clone();

    let from_scratch = mk_folder(vec![d1_upd]);
    let from_scratch = from_scratch.conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);
    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.breakCrossRef`
#[test]
fn break_cross_ref() {
    let d1 = fake_doc("d1.md", &["# Doc1 idx", "## Sub"]);

    // Update (remove + add a def)
    let d1_upd = fake_doc("d1.md", &["# Doc1 index", "## Sub"]);

    let d2 = fake_doc("d2.md", &["[[Doc1 idx#Sub]]"]);

    let f = mk_folder(vec![d1, d2.clone()]);
    let f = f.with_doc(d1_upd.clone());
    let incr = f.conn().clone();

    let from_scratch = mk_folder(vec![d1_upd, d2]);
    let from_scratch = from_scratch.conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);
    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.addingEmptyHeader`
#[test]
fn adding_empty_header() {
    let d1 = fake_doc("d1.md", &["# Doc1", "## Sub"]);

    let d2 = fake_doc("d2.md", &["[[Doc1#Sub]]"]);

    let d1_upd = fake_doc("d1.md", &["# Doc1", "## Sub", "# "]);

    let f = mk_folder(vec![d1, d2.clone()]);
    let f = f.with_doc(d1_upd.clone());
    let incr = f.conn().clone();

    let from_scratch = mk_folder(vec![d1_upd, d2]);
    let from_scratch = from_scratch.conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);
    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.RenameTests` fixtures.
mod rename_tests {
    use super::*;

    fn d1() -> Doc {
        fake_doc("d1.md", &["# Doc1 idx", "## Sub"])
    }

    // Update (remove + add a def)
    fn d1_upd() -> Doc {
        fake_doc("d1.md", &["# Doc1 index", "## Sub"])
    }

    fn d2() -> Doc {
        fake_doc("d2.md", &["[[Doc1 idx#Sub]]"])
    }

    // Fix the reference
    fn d2_upd() -> Doc {
        fake_doc("d2.md", &["[[Doc1 index#Sub]]"])
    }

    /// `ConnGraphTests.RenameTests.renameCrossRef_D1_then_D2`
    #[test]
    fn rename_cross_ref_d1_then_d2() {
        let f = mk_folder(vec![d1(), d2()]);
        let f = f.with_doc(d1_upd()).with_doc(d2_upd());
        let incr = f.conn().clone();

        let from_scratch = mk_folder(vec![d1_upd(), d2_upd()]);
        let from_scratch = from_scratch.conn().clone();

        let conn_diff = Conn::difference(&from_scratch, &incr);
        check_empty_difference(&conn_diff);
    }

    /// `ConnGraphTests.RenameTests.renameCrossRef_D2_then_D1`
    #[test]
    fn rename_cross_ref_d2_then_d1() {
        let f = mk_folder(vec![d1(), d2()]);
        let f = f.with_doc(d2_upd()).with_doc(d1_upd());
        let incr = f.conn().clone();

        let from_scratch = mk_folder(vec![d1_upd(), d2_upd()]);
        let from_scratch = from_scratch.conn().clone();

        let conn_diff = Conn::difference(&from_scratch, &incr);
        check_empty_difference(&conn_diff);
    }
}

/// `ConnGraphTests.addDocThenTitle`
#[test]
fn add_doc_then_title() {
    let d1 = fake_doc("doc-1.md", &["[[non-existent]]"]);

    let d_na1 = fake_doc("non-existent.md", &[""]);

    let d_na2 = fake_doc("non-existent.md", &["# D"]);

    let incr = mk_folder(vec![d1.clone()]).with_doc(d_na1).with_doc(d_na2.clone());
    let incr = incr.conn().clone();

    let from_scratch = mk_folder(vec![d1, d_na2]);
    let from_scratch = from_scratch.conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);
    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.addSecondTitle`
#[test]
fn add_second_title() {
    let d1 = fake_doc("doc-1.md", &["[[doc-2]]"]);
    let d2 = fake_doc("doc-2.md", &[""]);
    let d21 = fake_doc("doc-2.md", &["# T1"]);

    let d22 = fake_doc("doc-2.md", &["# T1", "# T2"]);

    let incr = mk_folder(vec![d1.clone(), d2]).with_doc(d21).with_doc(d22.clone());
    let incr = incr.conn().clone();

    let from_scratch = mk_folder(vec![d1, d22]);
    let from_scratch = from_scratch.conn().clone();

    let conn_diff = Conn::difference(&from_scratch, &incr);
    check_empty_difference(&conn_diff);
}

/// `ConnGraphTests.initGraphWithTags`
#[test]
fn init_graph_with_tags() {
    let d1 = fake_doc("d1.md", &["#tag1 #tag2"]);
    let d2 = fake_doc("d2.md", &["#tag2", "#tag3"]);
    let f = mk_folder(vec![d1, d2]);
    let conn = Conn::mk(&f.oracle(), &f.syms());
    check_snapshot(
        &conn,
        &[
            "Defs:",
            "  d1.md:",
            "    Doc",
            "  d2.md:",
            "    Doc",
            "Tags:",
            "  d1.md:",
            "    #tag1",
            "    #tag2",
            "  d2.md:",
            "    #tag2",
            "    #tag3",
            "Resolved:",
            "  d1.md:",
            "    #tag1 -> #tag1 @ Global",
            "    #tag2 -> #tag2 @ Global",
            "  d2.md:",
            "    #tag2 -> #tag2 @ Global",
            "    #tag3 -> #tag3 @ Global",
            "  Global:",
            "    #tag1 -> #tag1 @ d1.md",
            "    #tag2 -> #tag2 @ d1.md",
            "    #tag2 -> #tag2 @ d2.md",
            "    #tag3 -> #tag3 @ d2.md",
            "Unresolved:",
            "  ",
        ],
    );
}

/// `ConnGraphTests.removingTag`
#[test]
fn removing_tag() {
    let d1 = fake_doc("d1.md", &["#tag1 #tag2"]);
    let d1p = fake_doc("d1.md", &["#tag1"]);
    let d2 = fake_doc("d2.md", &["#tag2", "#tag3"]);
    let fp = mk_folder(vec![d1, d2]).with_doc(d1p);
    check_snapshot(
        fp.conn(),
        &[
            "Defs:",
            "  d1.md:",
            "    Doc",
            "  d2.md:",
            "    Doc",
            "Tags:",
            "  d1.md:",
            "    #tag1",
            "  d2.md:",
            "    #tag2",
            "    #tag3",
            "Resolved:",
            "  d1.md:",
            "    #tag1 -> #tag1 @ Global",
            "  d2.md:",
            "    #tag2 -> #tag2 @ Global",
            "    #tag3 -> #tag3 @ Global",
            "  Global:",
            "    #tag1 -> #tag1 @ d1.md",
            "    #tag2 -> #tag2 @ d2.md",
            "    #tag3 -> #tag3 @ d2.md",
            "Unresolved:",
            "  ",
        ],
    );
}

// ---------------------------------------------------------------------------
// ConnGraphTests_TitleLess
// ---------------------------------------------------------------------------

mod conn_graph_tests_title_less {
    use super::*;

    fn title_less_config() -> Config {
        Config {
            core_incremental_references: Some(true),
            core_title_from_heading: Some(false),
            compl_wiki_style: Some(ComplWikiStyle::TitleSlug),
            ..Config::default_config()
        }
    }

    fn mk_folder_title_less(docs: Vec<Doc>) -> Folder {
        FakeFolder::mk_with_config(docs, Some(title_less_config()))
    }

    fn mk_doc(path: &str, content: &[&str]) -> Doc {
        fake_doc_config(path, content, &title_less_config())
    }

    /// `ConnGraphTests_TitleLess.updateH1`
    #[test]
    fn update_h1() {
        let d1 = mk_doc("ocaml.md", &["# OCaml", "[[#Multicore]]", "## Multicore"]);

        let d1p = mk_doc("ocaml.md", &["# OCaml L", "[[#Multicore]]", "## Multicore"]);

        let d2 = mk_doc("test.md", &["# Test", "[[OCaml#Multicore]]"]);

        let incr = mk_folder_title_less(vec![d1])
            .with_doc(d2.clone())
            .with_doc(d1p.clone());
        let incr = incr.conn().clone();

        let from_scratch = mk_folder_title_less(vec![d1p, d2]);
        let from_scratch = from_scratch.conn().clone();

        let conn_diff = Conn::difference(&from_scratch, &incr);
        check_empty_difference(&conn_diff);
    }
}

// ---------------------------------------------------------------------------
// IncrementalRegressionTests
// ---------------------------------------------------------------------------

mod incremental_regression_tests {
    use super::*;

    fn reg_doc(path: &str, lines: &[&str]) -> Doc {
        fake_doc(path, lines)
    }

    enum Update {
        Add(Doc),
        Remove(DocId),
    }

    fn apply(folder: Folder, update: Update) -> Folder {
        match update {
            Update::Add(doc) => folder.with_doc(doc),
            Update::Remove(id) => folder.without_doc(&id).unwrap(),
        }
    }

    /// `IncrementalRegressionTests.lifecycle`
    #[test]
    fn lifecycle() {
        for scenario in [
            "remove section",
            "remove explicit doc",
            "add matching heading",
            "rename missing target",
            "delete missing target",
            "add ambiguous target",
        ] {
            let target = reg_doc("target.md", &["# Target", "## Sub"]);
            let source = |lines: &[&str]| reg_doc("source.md", lines);

            let (folder, update) = match scenario {
                "remove section" => (
                    mk_folder(vec![target.clone(), source(&["[[target#Sub]]"])]),
                    Update::Add(source(&[])),
                ),
                "remove explicit doc" => (
                    mk_folder(vec![target.clone(), source(&["[[target]]", "[[target#Sub]]"])]),
                    Update::Add(source(&["[[target#Sub]]"])),
                ),
                "add matching heading" => (
                    mk_folder(vec![target.clone(), source(&["[[target#Sub]]"])]),
                    Update::Add(reg_doc("target.md", &["# Target", "## Sub", "### Sub"])),
                ),
                "rename missing target" => (
                    mk_folder(vec![target.clone(), source(&["[[Target#Missing]]"])]),
                    Update::Add(reg_doc("target.md", &["# Other", "## Sub"])),
                ),
                "delete missing target" => (
                    mk_folder(vec![target.clone(), source(&["[[target#Missing]]"])]),
                    Update::Remove(target.id().clone()),
                ),
                _ => (
                    mk_folder(vec![target.clone(), source(&["[[target#Sub]]"])]),
                    Update::Add(reg_doc("other.md", &["# Target", "## Sub"])),
                ),
            };

            assert!(
                folder.config_or_default().core_incremental_references(),
                "incremental enabled"
            );

            let updated = apply(folder, update);
            assert_matches_clean_construction(&updated);

            let renamed = updated.with_doc(reg_doc("target.md", &["# Renamed"]));
            assert_matches_clean_construction(&renamed);
        }
    }

    /// `IncrementalRegressionTests.addingAmbiguousScopeInvalidatesMissingSections`
    #[test]
    fn adding_ambiguous_scope_invalidates_missing_sections() {
        let folder = mk_folder(vec![
            reg_doc("one.md", &["# Alpha"]),
            reg_doc("source.md", &["[[Alpha#Sub]]"]),
        ])
        .with_doc(reg_doc("two.md", &["# Alpha", "## Sub"]));
        assert_matches_clean_construction(&folder);
    }

    /// `IncrementalRegressionTests.addingPathMatchWhenExistingMatchHasDifferentTitle`
    #[test]
    fn adding_path_match_when_existing_match_has_different_title() {
        let folder = mk_folder(vec![
            reg_doc("nested/a.md", &["# Beta"]),
            reg_doc("source.md", &["[[a]]"]),
        ])
        .with_doc(reg_doc("a.md", &[]));
        assert_matches_clean_construction(&folder);
    }

    /// `IncrementalRegressionTests.renamingScopeClearsOldUnresolvedEdges`
    #[test]
    fn renaming_scope_clears_old_unresolved_edges() {
        let folder = mk_folder(vec![
            reg_doc("one.md", &["# Alpha"]),
            reg_doc("source.md", &["[[Alpha#Sub]]"]),
        ])
        .with_doc(reg_doc("one.md", &["# Beta"]));
        assert_matches_clean_construction(&folder);
    }

    /// `IncrementalRegressionTests.reorderingTitlesChangesCandidateSelection`
    #[test]
    fn reordering_titles_changes_candidate_selection() {
        let folder = mk_folder(vec![
            reg_doc("one.md", &["# Alpha", "# Beta"]),
            reg_doc("source.md", &["[[Alpha]]", "[[Beta]]"]),
        ])
        .with_doc(reg_doc("one.md", &["# Beta", "# Alpha"]));
        assert_matches_clean_construction(&folder);
    }

    /// `IncrementalRegressionTests.editSequences`
    #[test]
    fn edit_sequences() {
        for seed in [17i32, 42, 123] {
            let mut random = DotNetRandom::new(seed);
            let paths = ["a.md", "b.md", "nested/a.md", "c.md"];

            let contents: [&[&str]; 13] = [
                &[],
                &["# Alpha", "## Sub"],
                &["# Beta", "## Other"],
                &["# Alpha", "## Other", "# Beta"],
                &["[[Alpha#Sub]]", "[[a#Other]]", "[[b]]"],
                &["[[Alpha]]", "[[Alpha#Other]]", "[[#Sub]]", "## Sub"],
                &["# Beta", "[[Alpha#Missing]]", "[[nested/a#Sub]]"],
                &["# Alpha", "#tag", "[link]", "[link]: /url"],
                &["# Beta", "# Alpha", "## Other"],
                &["[[a]]", "[[a#Sub]]", "[[a#Other]]"],
                &["[[a#Other]]", "[[./a#Sub]]", "[[/a#Sub]]"],
                &["## Sub", "### Other", "[link]"],
                &["[link]: /url", "[link]", "#tag"],
            ];

            let mut folder = mk_folder(Vec::new());
            let mut history: Vec<String> = Vec::new();

            for step in 1..=1000 {
                let path = paths[random.next(paths.len() as i32) as usize];
                let content = random.next(contents.len() as i32) as usize;
                let next = reg_doc(path, contents[content]);
                let delete = random.next(5) == 0;
                let action = if delete { "deleted".to_string() } else { content.to_string() };
                history.push(format!("{step}: {path} = {action}"));

                folder = if delete {
                    folder.without_doc(next.id()).unwrap()
                } else {
                    folder.with_doc(next)
                };

                let expected = Conn::mk(&folder.oracle(), &folder.syms());
                let difference = Conn::difference(&expected, folder.conn());
                assert!(
                    difference.is_empty(),
                    "seed {seed}\n{}\n{}",
                    history.join("\n"),
                    difference.compact_format()
                );
            }
        }
    }

    /// `IncrementalRegressionTests.removingTitleClearsUnresolvedScope`
    #[test]
    fn removing_title_clears_unresolved_scope() {
        let folder = mk_folder(vec![
            reg_doc("one.md", &["# Alpha"]),
            reg_doc("two.md", &["# Alpha", "## Sub"]),
            reg_doc("source.md", &["[[Alpha#Sub]]"]),
        ])
        .with_doc(reg_doc("one.md", &[]));
        assert_matches_clean_construction(&folder);
    }

    /// `IncrementalRegressionTests.paranoidScopeChanges`
    #[test]
    fn paranoid_scope_changes() {
        let config = Config { core_paranoid: Some(true), ..incr_config() };

        let folder = FakeFolder::mk_with_config(
            vec![reg_doc("one.md", &["# Alpha"]), reg_doc("source.md", &["[[Alpha#Sub]]"])],
            Some(config),
        )
        .with_doc(reg_doc("two.md", &["# Alpha", "## Sub", "# Beta"]))
        .with_doc(reg_doc("two.md", &["# Beta", "## Sub", "# Alpha"]));
        let folder = folder.without_doc(&reg_doc("one.md", &[]).id()).unwrap();
        assert_matches_clean_construction(&folder);
    }

    /// `IncrementalRegressionTests.changingTitlePreservesUnresolvedLocalReferences`
    #[test]
    fn changing_title_preserves_unresolved_local_references() {
        let folder = mk_folder(vec![reg_doc("one.md", &["## Sub", "[missing]"])])
            .with_doc(reg_doc("one.md", &["# Alpha", "[missing]"]));
        assert_matches_clean_construction(&folder);
    }
}
