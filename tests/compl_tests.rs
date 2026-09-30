//! Port of `Tests/ComplTests.fs`.
//!
//! Expectations are taken verbatim from the original's recorded snapshots under
//! `Tests/_snapshots/*.json` (Snapper `ShouldMatchSnapshot`), and from the inline
//! `Assert.Equal` calls where the original used those instead.
//!
//! `PartialElement::Display` in `src/compl.rs` mirrors the F# `ToString()`
//! override, and `cst::fmt_range` mirrors `Range.DebuggerDisplay`, so the
//! snapshot lines can be compared character for character.

mod common;

use lsp_types::{CompletionItem, CompletionTextEdit, Position};

use marksman::compl::{find_candidates_in_doc, PartialElement};
use marksman::config::{ComplWikiStyle, Config};
use marksman::cst;
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::misc;
use marksman::text::mk_text;

use common::{FakeDoc, FakeFolder};

/// `let tryParsePartialElement text line col = PartialElement.inText text (Position.Mk(line, col))`
fn try_parse_partial_element(text: &str, line: u32, col: u32) -> Option<PartialElement> {
    PartialElement::in_text(&mk_text(text), Position { line, character: col })
}

/// `let parsePartialElement text line col = tryParsePartialElement text line col |> Option.get`
fn parse_partial_element(text: &str, line: u32, col: u32) -> PartialElement {
    try_parse_partial_element(text, line, col)
        .unwrap_or_else(|| panic!("No partial element at ({line}, {col}) in {text:?}"))
}

/// `checkSnapshot` of the `PartialElement*` modules:
/// `Seq.map (fun x -> x.ToString().Lines()) els |> Array.concat`.
fn fmt_partial_elements(els: &[PartialElement]) -> Vec<String> {
    els.iter()
        .flat_map(|el| {
            misc::lines_of(&el.to_string())
                .into_iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn check_partial_snapshot(els: &[PartialElement], expected: &[&str]) {
    let actual = fmt_partial_elements(els);
    let expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(actual, expected);
}

/// `checkSnapshot` of the `Candidates` module.
fn fmt_item(ci: &CompletionItem) -> String {
    let filter_text =
        ci.filter_text.clone().unwrap_or_else(|| "<no-filter>".to_string());

    match &ci.text_edit {
        Some(CompletionTextEdit::Edit(te)) => {
            format!("{}: {} / {}", cst::fmt_range(te.range), te.new_text, filter_text)
        }
        Some(CompletionTextEdit::InsertAndReplace(_)) => panic!("Expected a TextEdit"),
        None => "<no-edit>".to_string(),
    }
}

/// `let findCandidatesInDoc folder doc pos = findCandidatesInDoc folder doc pos |> Array.ofSeq`
fn find_candidates_in_doc_at(folder: &Folder, doc: &Doc, line: u32, col: u32) -> Vec<CompletionItem> {
    find_candidates_in_doc(folder, doc, Position { line, character: col })
}

fn check_candidates(folder: &Folder, doc: &Doc, line: u32, col: u32, expected: &[&str]) {
    let actual: Vec<String> =
        find_candidates_in_doc_at(folder, doc, line, col).iter().map(fmt_item).collect();
    let expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(actual, expected);
}

/// `FakeDoc.Mk(path = ..., contentLines = [| ... |])`
fn fake_doc(path: &str, content_lines: &[&str]) -> Doc {
    FakeDoc::mk_with(&content_lines.join("\n"), path, None, None)
}

// ---------------------------------------------------------------------------
// module PartialElementWiki
// ---------------------------------------------------------------------------

#[test]
fn partial_element_wiki_empty() {
    assert!(try_parse_partial_element("", 0, 0).is_none());
}

#[test]
fn partial_element_wiki_empty_eof() {
    check_partial_snapshot(
        &[parse_partial_element("[[", 0, 2)],
        &["WL (0,0)-(1,0): dest=∅; heading=∅"],
    );
}

#[test]
fn partial_element_wiki_empty_eol() {
    check_partial_snapshot(
        &[parse_partial_element("[[\n", 0, 2)],
        &["WL (0,0)-(0,2): dest=∅; heading=∅"],
    );
}

#[test]
fn partial_element_wiki_empty_non_eol() {
    check_partial_snapshot(
        &[parse_partial_element("[[ ", 0, 2)],
        &["WL (0,0)-(0,2): dest=∅; heading=∅"],
    );
}

#[test]
fn partial_element_wiki_some_eof() {
    check_partial_snapshot(
        &[parse_partial_element("[[t", 0, 2)],
        &["WL (0,0)-(1,0): dest=t @ (0,2)-(1,0); heading=∅"],
    );
}

#[test]
fn partial_element_wiki_some_eol() {
    check_partial_snapshot(
        &[parse_partial_element("[[to\n", 0, 2)],
        &["WL (0,0)-(0,4): dest=to @ (0,2)-(0,4); heading=∅"],
    );
}

#[test]
fn partial_element_wiki_some_ws() {
    check_partial_snapshot(
        &[parse_partial_element("[[t ", 0, 2)],
        &["WL (0,0)-(0,3): dest=t @ (0,2)-(0,3); heading=∅"],
    );
}

#[test]
fn partial_element_wiki_some_and_text_after() {
    check_partial_snapshot(
        &[parse_partial_element("[[t other", 0, 2)],
        &["WL (0,0)-(0,3): dest=t @ (0,2)-(0,3); heading=∅"],
    );
}

#[test]
fn partial_element_wiki_empty_heading() {
    check_partial_snapshot(
        &[parse_partial_element("[[#", 0, 3)],
        &["WL (0,0)-(1,0): dest=∅; heading= @ (0,3)-(1,0)"],
    );
}

#[test]
fn partial_element_wiki_non_empty_heading() {
    check_partial_snapshot(
        &[parse_partial_element("[[#hea] ", 0, 3)],
        &["WL (0,0)-(0,7): dest=∅; heading=hea @ (0,3)-(0,6)"],
    );
}

// ---------------------------------------------------------------------------
// module PartialElementReference
// ---------------------------------------------------------------------------

#[test]
fn partial_element_reference_empty() {
    assert!(try_parse_partial_element("", 0, 0).is_none());
}

#[test]
fn partial_element_reference_empty_eof() {
    check_partial_snapshot(&[parse_partial_element("[", 0, 1)], &["RL (0,0)-(1,0): label=∅"]);
}

#[test]
fn partial_element_reference_empty_eol() {
    check_partial_snapshot(&[parse_partial_element("[\n", 0, 1)], &["RL (0,0)-(0,1): label=∅"]);
}

#[test]
fn partial_element_reference_empty_non_eol() {
    check_partial_snapshot(&[parse_partial_element("[ ", 0, 1)], &["RL (0,0)-(0,1): label=∅"]);
}

#[test]
fn partial_element_reference_some_eof() {
    check_partial_snapshot(
        &[parse_partial_element("[t", 0, 1)],
        &["RL (0,0)-(1,0): label=t @ (0,1)-(1,0)"],
    );
}

#[test]
fn partial_element_reference_some_eol() {
    check_partial_snapshot(
        &[parse_partial_element("[t\n", 0, 1)],
        &["RL (0,0)-(0,2): label=t @ (0,1)-(0,2)"],
    );
}

#[test]
fn partial_element_reference_some_ws() {
    check_partial_snapshot(
        &[parse_partial_element("[t ", 0, 1)],
        &["RL (0,0)-(0,2): label=t @ (0,1)-(0,2)"],
    );
}

#[test]
fn partial_element_reference_some_and_text_after() {
    check_partial_snapshot(
        &[parse_partial_element("[t other", 0, 1)],
        &["RL (0,0)-(0,2): label=t @ (0,1)-(0,2)"],
    );
}

#[test]
fn partial_element_reference_empty_brackets() {
    check_partial_snapshot(&[parse_partial_element("[]", 0, 1)], &["RL (0,0)-(1,0): label=∅"]);
}

#[test]
fn partial_element_reference_partial_reference() {
    check_partial_snapshot(&[parse_partial_element("[l][", 0, 4)], &["RL (0,3)-(1,0): label=∅"]);
}

// ---------------------------------------------------------------------------
// module PartialElementInline
// ---------------------------------------------------------------------------

#[test]
fn partial_element_inline_empty() {
    assert!(try_parse_partial_element("", 0, 0).is_none());
}

#[test]
fn partial_element_inline_empty_eof() {
    check_partial_snapshot(
        &[parse_partial_element("(", 0, 1)],
        &["IL (0,0)-(1,0): text=∅; path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_empty_eol() {
    check_partial_snapshot(
        &[parse_partial_element("(\n", 0, 1)],
        &["IL (0,0)-(0,1): text=∅; path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_empty_link_eof() {
    check_partial_snapshot(
        &[parse_partial_element("](", 0, 2)],
        &["IL (0,1)-(1,0): text=∅; path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_empty_link_eol() {
    check_partial_snapshot(
        &[parse_partial_element("](\n", 0, 2)],
        &["IL (0,1)-(0,2): text=∅; path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_empty_non_eol() {
    check_partial_snapshot(
        &[parse_partial_element("]( ", 0, 2)],
        &["IL (0,1)-(0,2): text=∅; path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_empty_non_eol_further() {
    assert!(try_parse_partial_element("]( ", 0, 3).is_none());
}

#[test]
fn partial_element_inline_some_eol() {
    check_partial_snapshot(
        &[parse_partial_element("](t", 0, 2)],
        &["IL (0,1)-(1,0): text=∅; path=t @ (0,2)-(1,0); anchor=∅"],
    );
}

#[test]
fn partial_element_inline_some_ws() {
    check_partial_snapshot(
        &[parse_partial_element("](t ", 0, 2)],
        &["IL (0,1)-(0,3): text=∅; path=t @ (0,2)-(0,3); anchor=∅"],
    );
}

#[test]
fn partial_element_inline_some_and_text_after() {
    check_partial_snapshot(
        &[parse_partial_element("](t other", 0, 2)],
        &["IL (0,1)-(0,3): text=∅; path=t @ (0,2)-(0,3); anchor=∅"],
    );
}

#[test]
fn partial_element_inline_some_and_text_before_after() {
    //                      01234567890
    check_partial_snapshot(
        &[parse_partial_element("before](t other", 0, 8)],
        &["IL (0,7)-(0,9): text=∅; path=t @ (0,8)-(0,9); anchor=∅"],
    );
}

#[test]
fn partial_element_inline_empty_brackets() {
    check_partial_snapshot(
        &[parse_partial_element("]()\n", 0, 2)],
        &["IL (0,1)-(0,3): text=∅; path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_brackets_and_open_paren() {
    check_partial_snapshot(
        &[parse_partial_element("[](", 0, 3)],
        &["IL (0,0)-(1,0): text= @ (0,1)-(0,1); path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_brackets_with_text_and_open_paren() {
    check_partial_snapshot(
        &[parse_partial_element("[b](", 0, 4)],
        &["IL (0,0)-(1,0): text=b @ (0,1)-(0,2); path=∅; anchor=∅"],
    );
}

#[test]
fn partial_element_inline_brackets_with_spaced_text_and_open_paren() {
    check_partial_snapshot(
        &[parse_partial_element("[a b](c", 0, 7)],
        &["IL (0,0)-(1,0): text=a b @ (0,1)-(0,4); path=c @ (0,6)-(1,0); anchor=∅"],
    );
}

#[test]
fn partial_element_inline_anchor1() {
    check_partial_snapshot(
        &[
            parse_partial_element("](t# other", 0, 2),
            parse_partial_element("](t# other", 0, 3),
            parse_partial_element("](t# other", 0, 4),
        ],
        &[
            "IL (0,1)-(0,4): text=∅; path=t @ (0,2)-(0,3); anchor= @ (0,4)-(0,4)",
            "IL (0,1)-(0,4): text=∅; path=t @ (0,2)-(0,3); anchor= @ (0,4)-(0,4)",
            "IL (0,1)-(0,4): text=∅; path=t @ (0,2)-(0,3); anchor= @ (0,4)-(0,4)",
        ],
    );
}

#[test]
fn partial_element_inline_anchor2() {
    check_partial_snapshot(
        &[parse_partial_element("](#a other", 0, 3)],
        &["IL (0,1)-(0,4): text=∅; path=∅; anchor=a @ (0,3)-(0,4)"],
    );

    assert!(try_parse_partial_element("](#a other", 0, 5).is_none());
}

#[test]
fn partial_element_inline_anchor3() {
    assert!(try_parse_partial_element("(#a )", 0, 4).is_none());
}

#[test]
fn partial_element_inline_anchor4() {
    check_partial_snapshot(
        &[parse_partial_element("](#a)", 0, 4)],
        &["IL (0,1)-(1,0): text=∅; path=∅; anchor=a @ (0,3)-(0,4)"],
    );
}

#[test]
fn partial_element_inline_anchor5() {
    //                      012345678
    check_partial_snapshot(
        &[parse_partial_element("](doc.md#", 0, 9)],
        &["IL (0,1)-(1,0): text=∅; path=doc.md @ (0,2)-(0,8); anchor= @ (0,9)-(1,0)"],
    );
}

#[test]
fn partial_element_inline_anchor6() {
    check_partial_snapshot(
        &[parse_partial_element("(# ", 0, 2)],
        &["IL (0,0)-(0,2): text=∅; path=∅; anchor= @ (0,2)-(0,2)"],
    );
}

// ---------------------------------------------------------------------------
// module PartialElementTag
// ---------------------------------------------------------------------------

#[test]
fn partial_element_tag_opening1() {
    check_partial_snapshot(&[parse_partial_element("#", 0, 1)], &["TO: cursorPos=(0,1)"]);
}

#[test]
fn partial_element_tag_opening2() {
    check_partial_snapshot(&[parse_partial_element("# ", 0, 1)], &["TO: cursorPos=(0,1)"]);
}

#[test]
fn partial_element_tag_opening3() {
    check_partial_snapshot(&[parse_partial_element("## ", 0, 2)], &["TO: cursorPos=(0,2)"]);
}

#[test]
fn partial_element_tag_opening4() {
    // This one is arguably where we may want to NOT suggest any completion.
    // IOW we may require that a hash sign is preceded with a punctuation or
    // a whitespace char.
    //                      01234567
    check_partial_snapshot(&[parse_partial_element("hello# ", 0, 6)], &["TO: cursorPos=(0,6)"]);
}

// ---------------------------------------------------------------------------
// module Candidates
// ---------------------------------------------------------------------------

//  012345678901234567890
fn global_doc1() -> Doc {
    fake_doc(
        "doc1.md",
        &[
            "# H1", // 0
            "[[#",
            "# A",
            "## H2", // 3
            "#B",
            "## H2",
            "[](/doc%202.md#)",
            "[[#]]",
        ], // 7
    )
}

fn global_doc2() -> Doc {
    fake_doc("doc 2.md", &["# H2", "[[h1#", "## D2 H2"])
}

fn global_folder() -> Folder {
    FakeFolder::mk(vec![global_doc1(), global_doc2()])
}

#[test]
fn candidates_no_dups_on_anchor_intra_file() {
    let folder = global_folder();
    let doc = global_doc1();
    check_candidates(&folder, &doc, 1, 3, &["(1,0)-(1,3): [[#H2]] / [[#H2]]"]);
}

#[test]
fn candidates_no_extra_hash_wiki_heading_intra_file_issue174() {
    let folder = global_folder();
    let doc = global_doc1();
    check_candidates(&folder, &doc, 7, 3, &["(7,2)-(7,3): #H2 / #H2"]);
}

#[test]
fn candidates_no_dups_on_anchor_cross_file() {
    let folder = global_folder();
    let doc = global_doc2();
    check_candidates(&folder, &doc, 1, 5, &["(1,0)-(1,5): [[h1#H2]] / [[h1#H2]]"]);
}

#[test]
fn candidates_file_with_spaces_anchor() {
    let folder = global_folder();
    let doc = global_doc1();
    check_candidates(
        &folder,
        &doc,
        6,
        15,
        &["(6,3)-(6,15): /doc%202.md#d2-h2 / /doc%202.md#D2 H2"],
    );
}

#[test]
fn candidates_doc_and_heading_fuzzy() {
    //                                                                   012345
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[[do#]]"]);
    let doc2 = fake_doc("doc2.md", &["# Doc 2", "## H2.1", "## H2.2"]);
    let doc3 = fake_doc("doc3.md", &["# Doc 3", "## H3"]);

    let folder = FakeFolder::mk(vec![doc1.clone(), doc2, doc3]);

    check_candidates(
        &folder,
        &doc1,
        1,
        5,
        &[
            "(1,2)-(1,5): doc-2#H2.1 / do#H2.1",
            "(1,2)-(1,5): doc-2#H2.2 / do#H2.2",
            "(1,2)-(1,5): doc-3#H3 / do#H3",
        ],
    );
}

#[test]
fn candidates_reference_empty_brackets() {
    let doc1 = fake_doc(
        "doc1.md",
        &["# Doc 1", "[]", "", "[link-1]: url1", "[link-2]: url2"],
    );

    let folder = FakeFolder::mk(vec![doc1.clone()]);

    check_candidates(
        &folder,
        &doc1,
        1,
        1,
        &["(1,0)-(1,2): [link-1] / [link-1]", "(1,0)-(1,2): [link-2] / [link-2]"],
    );
}

#[test]
fn candidates_reference_non_empty_brackets() {
    let doc1 = fake_doc(
        "doc1.md",
        &["# Doc 1", "[l]", "", "[link-1]: url1", "[link-2]: url2"],
    );

    let folder = FakeFolder::mk(vec![doc1.clone()]);

    check_candidates(
        &folder,
        &doc1,
        1,
        2,
        &["(1,1)-(1,2): link-1 / link-1", "(1,1)-(1,2): link-2 / link-2"],
    );
}

#[test]
fn candidates_inline_empty() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[]()"]);
    let doc2 = fake_doc("doc2.md", &["# Doc 2"]);
    let doc3 = fake_doc("doc3.md", &["# Doc 3"]);
    let folder = FakeFolder::mk(vec![doc1.clone(), doc2, doc3]);

    check_candidates(
        &folder,
        &doc1,
        1,
        3,
        &["(1,3)-(1,3): /doc2.md / <no-filter>", "(1,3)-(1,3): /doc3.md / <no-filter>"],
    );
}

#[test]
fn candidates_partial_wiki_doc() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[["]);
    let doc2 = fake_doc("doc2.md", &["# Doc 2"]);
    let doc3 = fake_doc("doc3.md", &["# Doc 3"]);
    let folder = FakeFolder::mk(vec![doc1.clone(), doc2, doc3]);

    check_candidates(
        &folder,
        &doc1,
        1,
        2,
        &["(1,0)-(2,0): [[doc-2]] / [[Doc 2]]", "(1,0)-(2,0): [[doc-3]] / [[Doc 3]]"],
    );
}

#[test]
fn candidates_partial_wiki_heading() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[[#", "## H2.1", "## H2.2"]);

    let folder = FakeFolder::mk(vec![doc1.clone()]);

    check_candidates(
        &folder,
        &doc1,
        1,
        3,
        &["(1,0)-(1,3): [[#H2.1]] / [[#H2.1]]", "(1,0)-(1,3): [[#H2.2]] / [[#H2.2]]"],
    );
}

#[test]
fn candidates_partial_wiki_doc_heading() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[[d#"]);
    let doc2 = fake_doc("doc2.md", &["# Doc 2", "## H2"]);
    let doc3 = fake_doc("doc3.md", &["# Doc 3", "## H3"]);

    let folder = FakeFolder::mk(vec![doc1.clone(), doc2, doc3]);

    check_candidates(
        &folder,
        &doc1,
        1,
        4,
        &["(1,0)-(2,0): [[doc-2#H2]] / [[d#H2]]", "(1,0)-(2,0): [[doc-3#H3]] / [[d#H3]]"],
    );
}

#[test]
fn candidates_partial_wiki_doc_heading_file_path_stem() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[[d#"]);
    let doc2 = fake_doc("sub2/doc2.md", &["# Doc 2", "## H2"]);
    let doc3 = fake_doc("sub3/this is doc 3.md", &["# Doc 3", "## H3"]);

    let config = Config {
        compl_wiki_style: Some(ComplWikiStyle::FilePathStem),
        ..Config::empty()
    };

    let folder = FakeFolder::mk_with_config(vec![doc1.clone(), doc2, doc3], Some(config));

    check_candidates(
        &folder,
        &doc1,
        1,
        4,
        &[
            "(1,0)-(2,0): [[sub2/doc2#H2]] / [[d#H2]]",
            "(1,0)-(2,0): [[sub3/this is doc 3#H3]] / [[d#H3]]",
        ],
    );
}

#[test]
fn candidates_partial_wiki_doc_heading_file_stem() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[[d#"]);
    let doc2 = fake_doc("sub2/doc2.md", &["# Doc 2", "## H2"]);
    let doc3 = fake_doc("sub3/this is doc 3.md", &["# Doc 3", "## H3"]);

    let config =
        Config { compl_wiki_style: Some(ComplWikiStyle::FileStem), ..Config::empty() };

    let folder = FakeFolder::mk_with_config(vec![doc1.clone(), doc2, doc3], Some(config));

    check_candidates(
        &folder,
        &doc1,
        1,
        4,
        &[
            "(1,0)-(2,0): [[doc2#H2]] / [[d#H2]]",
            "(1,0)-(2,0): [[this is doc 3#H3]] / [[d#H3]]",
        ],
    );
}

#[test]
fn candidates_partial_wiki_doc_file_stem_arbitrary_path() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[[sun"]);
    let doc2 = fake_doc("20221218.md", &["# Sun 18 Dec 2022"]);

    let folder = FakeFolder::mk(vec![doc1.clone(), doc2]);

    check_candidates(
        &folder,
        &doc1,
        1,
        5,
        &["(1,0)-(2,0): [[sun-18-dec-2022]] / [[Sun 18 Dec 2022]]"],
    );
}

#[test]
fn candidates_partial_reference_empty() {
    let doc1 =
        fake_doc("doc1.md", &["# Doc 1", "[", "", "[link-1]: url1", "[link-2]: url2"]);

    let folder = FakeFolder::mk(vec![doc1.clone()]);

    check_candidates(
        &folder,
        &doc1,
        1,
        1,
        &["(1,0)-(1,1): [link-1] / [link-1]", "(1,0)-(1,1): [link-2] / [link-2]"],
    );
}

#[test]
fn candidates_partial_inline_heading() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[link](#", "## H2.1", "## H2.2"]);

    let folder = FakeFolder::mk(vec![doc1.clone()]);

    check_candidates(
        &folder,
        &doc1,
        1,
        8,
        &["(1,0)-(1,8): [link](#h21) / [link](#h21)", "(1,0)-(1,8): [link](#h22) / [link](#h22)"],
    );
}

#[test]
fn candidates_partial_inline_doc() {
    let doc1 = fake_doc("doc1.md", &["# Doc 1", "[]("]);
    let doc2 = fake_doc("doc2.md", &["# Doc 2"]);
    let doc3 = fake_doc("doc3.md", &["# Doc 3"]);
    let folder = FakeFolder::mk(vec![doc1.clone(), doc2, doc3]);

    check_candidates(
        &folder,
        &doc1,
        1,
        3,
        &["(1,0)-(2,0): [](/doc2.md) / [](/doc2.md)", "(1,0)-(2,0): [](/doc3.md) / [](/doc3.md)"],
    );
}

// module Candidates.WikiWithSpaces_TitleSlug

fn wiki_with_spaces_title_slug() -> (Folder, Doc) {
    let doc1 = fake_doc("doc1.md", &["# A A B B"]);
    let doc2 = fake_doc("doc2.md", &["# A A C C"]);
    let doc3 = fake_doc("doc3.md", &["# A B B D"]);
    let doc4 = fake_doc("doc4.md", &["[[a]]", "[[a-a]]", "[[a-b]]"]);

    let folder =
        FakeFolder::mk(vec![doc1, doc2, doc3, doc4.clone()]);

    (folder, doc4)
}

#[test]
fn candidates_wiki_with_spaces_title_slug_test1() {
    let (folder, doc4) = wiki_with_spaces_title_slug();
    check_candidates(
        &folder,
        &doc4,
        0,
        3,
        &[
            "(0,2)-(0,3): a-a-b-b / A A B B",
            "(0,2)-(0,3): a-a-c-c / A A C C",
            "(0,2)-(0,3): a-b-b-d / A B B D",
        ],
    );
}

#[test]
fn candidates_wiki_with_spaces_title_slug_test2() {
    let (folder, doc4) = wiki_with_spaces_title_slug();
    check_candidates(
        &folder,
        &doc4,
        1,
        5,
        &["(1,2)-(1,5): a-a-b-b / A A B B", "(1,2)-(1,5): a-a-c-c / A A C C"],
    );
}

#[test]
fn candidates_wiki_with_spaces_title_slug_test3() {
    let (folder, doc4) = wiki_with_spaces_title_slug();
    check_candidates(
        &folder,
        &doc4,
        2,
        5,
        &["(2,2)-(2,5): a-a-b-b / A A B B", "(2,2)-(2,5): a-b-b-d / A B B D"],
    );
}

// module Candidates.WikiWithSpaces_FileStem

fn wiki_with_spaces_file_stem() -> (Folder, Doc) {
    let doc1 = fake_doc("doc one.md", &["# Doc 1"]);
    let doc2 = fake_doc("doc two.md", &["# Doc 2"]);
    let doc3 = fake_doc("another doc.md", &["# Doc 3"]);
    let doc4 = fake_doc("doc4.md", &["[[do]]", "[[doc o]]", "[[ano]]"]);

    let config =
        Config { compl_wiki_style: Some(ComplWikiStyle::FileStem), ..Config::empty() };

    let folder =
        FakeFolder::mk_with_config(vec![doc1, doc2, doc3, doc4.clone()], Some(config));

    (folder, doc4)
}

#[test]
fn candidates_wiki_with_spaces_file_stem_test1() {
    let (folder, doc4) = wiki_with_spaces_file_stem();
    check_candidates(
        &folder,
        &doc4,
        0,
        4,
        &[
            "(0,2)-(0,4): another doc / Doc 3",
            "(0,2)-(0,4): doc one / Doc 1",
            "(0,2)-(0,4): doc two / Doc 2",
        ],
    );
}

#[test]
fn candidates_wiki_with_spaces_file_stem_test2() {
    let (folder, doc4) = wiki_with_spaces_file_stem();
    check_candidates(&folder, &doc4, 1, 7, &["(1,2)-(1,7): doc one / Doc 1"]);
}

#[test]
fn candidates_wiki_with_spaces_file_stem_test3() {
    let (folder, doc4) = wiki_with_spaces_file_stem();
    check_candidates(&folder, &doc4, 2, 5, &["(2,2)-(2,5): another doc / Doc 3"]);
}

#[test]
fn candidates_wiki_heading_with_special_chars_not_encoded() {
    let doc1 = fake_doc(
        "doc1.md",
        &[
            "# Doc 1",
            "## Foo / Bar",
            "## Baz",
            //1234
            "[[#f",
            "",
        ],
    );

    let folder = FakeFolder::mk(vec![doc1.clone()]);
    check_candidates(
        &folder,
        &doc1,
        3,
        4,
        &["(3,0)-(3,4): [[#Foo / Bar]] / [[#Foo / Bar]]"],
    );
}

#[test]
// # has a special meaning in URIs. Because of this Uri library doesn't handle
// paths with # well and we can't provide subtitle completion.
// Best workaround -- don't use paths with # sign.
//
// DIVERGENCE (defensible, but a divergence): the F# expectation above is an
// artifact of `System.Uri`. `Paths.uriToSystemPath` (Marksman/Paths.fs:45) does
// `Uri(unescaped).LocalPath`, and .NET splits `file:///blah#blah.md` into
// AbsolutePath="/blah" + Fragment="#blah.md", so `LocalPath` is just "/blah".
// The document therefore loses its ".md" name and `FileLink.filterFuzzyMatchingDocs`
// cannot resolve the typed dest "blah%23blah" to it -> zero candidates.
//
// The Rust port's `paths::uri_to_system_path` (src/paths.rs:611) is plain string
// surgery and keeps the fragment, so doc1 stays "blah#blah.md", the dest resolves,
// and the two subtitles of doc1 are offered:
//
//   F# expected: []
//   Rust actual: ["(0,2)-(0,14): blah%23blah#Subtitle 1 / blah%23blah#Subtitle 1",
//                 "(0,2)-(0,14): blah%23blah#Subtitle 2 / blah%23blah#Subtitle 2"]
//
// The Rust behaviour is arguably the better one (the whole point of the F# test
// name is that this is a *limitation*), but it is not what the original does.
fn candidates_wiki_file_with_special_chars_subtitle_no_completion_provided() {
    let doc1 = fake_doc(
        "blah#blah.md",
        &["# Blah Blah", "## Subtitle 1", "## Subtitle 2"],
    );

    let doc2 =
        //                                              0123456789012345
        fake_doc("doc2.md", &["[[blah%23blah#]]"]);

    let config =
        Config { compl_wiki_style: Some(ComplWikiStyle::FileStem), ..Config::empty() };

    let folder = FakeFolder::mk_with_config(vec![doc1, doc2.clone()], Some(config));

    check_candidates(&folder, &doc2, 0, 14, &[]);
}

#[test]
fn candidates_wiki_cross_heading_title_name_vs_slug() {
    let d1 = fake_doc("d1.md", &["# Doc Title", "## Subtitle"]);
    //                                                     01234567890123
    let d2 = fake_doc("d2.md", &["[[Doc Title#]]"]);

    let folder = FakeFolder::mk(vec![d1, d2.clone()]);

    check_candidates(
        &folder,
        &d2,
        0,
        12,
        &["(0,2)-(0,12): doc-title#Subtitle / Doc Title#Subtitle"],
    );
}

// module Candidates.Tags

fn tags_folder() -> (Folder, Doc) {
    let doc1 = fake_doc(
        "doc1.md",
        &[
            "We have #tag and #anotherTag",
            //12345678901234567
            "And an opening # ",
            //12345678901234567
            "And partial #ta ",
        ],
    );

    let doc2 = fake_doc("doc2.md", &["And #somethingElse #otherDocTag"]);

    let folder = FakeFolder::mk(vec![doc1.clone(), doc2]);

    (folder, doc1)
}

// DIVERGENCE (port bug): `Candidates.findTagCandidates` in `Marksman/Compl.fs`
// ends with `|> Seq.countBy id`, which yields the tags in *first-appearance*
// order (folder's doc order, then in-document order). `candidates::
// find_tag_candidates` in `src/compl.rs` builds the same first-appearance list
// but then calls `counts.sort()` (src/compl.rs:921), re-ordering it
// alphabetically. The candidate *set* is identical; only the order differs.
//
//   F# expected: tag, anotherTag, ta, somethingElse, otherDocTag
//   Rust actual: anotherTag, otherDocTag, somethingElse, ta, tag
#[test]
fn candidates_tags_tag_opening() {
    let (folder, doc1) = tags_folder();
    check_candidates(
        &folder,
        &doc1,
        1,
        16,
        &[
            "(1,16)-(1,16): tag / <no-filter>",
            "(1,16)-(1,16): anotherTag / <no-filter>",
            "(1,16)-(1,16): ta / <no-filter>",
            "(1,16)-(1,16): somethingElse / <no-filter>",
            "(1,16)-(1,16): otherDocTag / <no-filter>",
        ],
    );
}

// DIVERGENCE (port bug): same root cause as `candidates_tags_tag_opening` above
// -- the extra `counts.sort()` in `candidates::find_tag_candidates`.
//
//   F# expected: tag, anotherTag, otherDocTag
//   Rust actual: anotherTag, otherDocTag, tag
#[test]
fn candidates_tags_tag_with_name() {
    let (folder, doc1) = tags_folder();
    check_candidates(
        &folder,
        &doc1,
        2,
        15,
        &[
            "(2,13)-(2,15): tag / <no-filter>",
            "(2,13)-(2,15): anotherTag / <no-filter>",
            "(2,13)-(2,15): otherDocTag / <no-filter>",
        ],
    );
}
