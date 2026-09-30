//! Port of `Tests/AstTests.fs`.
//!
//! Every `[<Fact>]` of the original is reproduced with its input and expected
//! values verbatim.

mod common;

use std::collections::BTreeSet;

use marksman::ast;
use marksman::config::ParserSettings;
use marksman::cst;
use marksman::misc;
use marksman::parser;
use marksman::structure::Structure;
use marksman::syms::Sym;
use marksman::text::mk_text;

/// `let parseString content = Parser.parse Config.ParserSettings.Default (Text.mkText content)`
fn parse_string(content: &str) -> Structure {
    parser::parse(&ParserSettings::default(), &mk_text(content))
}

/// `let checkInlineSnapshot = Helpers.checkInlineSnapshot (fun (el: Element) -> el.CompactFormat())`
fn check_inline_snapshot(elements: &[ast::Element], snapshot: &[&str]) {
    let actual: Vec<String> = elements
        .iter()
        .flat_map(|el| {
            misc::lines_of(&el.compact_format())
                .into_iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    let expected: Vec<String> = snapshot.iter().map(|l| l.to_string()).collect();
    assert_eq!(
        actual,
        expected,
        "\n--- expected (F#) ---\n{}\n--- actual (Rust) ---\n{}",
        expected.join("\n"),
        actual.join("\n")
    );
}

/// `Helpers.checkInlineSnapshot Cst.Element.fmt`
fn check_inline_snapshot_cst(elements: &[cst::Element], snapshot: &[&str]) {
    let actual = common::fmt_elements(elements);
    let expected: Vec<String> = snapshot.iter().map(|l| l.to_string()).collect();
    assert_eq!(
        actual,
        expected,
        "\n--- expected (F#) ---\n{}\n--- actual (Rust) ---\n{}",
        expected.join("\n"),
        actual.join("\n")
    );
}

/// `Helpers.checkInlineSnapshot _.ToString() strukt.Symbols`
fn check_inline_snapshot_syms(symbols: &BTreeSet<Sym>, snapshot: &[&str]) {
    let actual: Vec<String> = symbols
        .iter()
        .flat_map(|sym| {
            misc::lines_of(&sym.to_string())
                .into_iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    let expected: Vec<String> = snapshot.iter().map(|l| l.to_string()).collect();
    assert_eq!(
        actual,
        expected,
        "\n--- expected (F#) ---\n{}\n--- actual (Rust) ---\n{}",
        expected.join("\n"),
        actual.join("\n")
    );
}

/// `let struct1 = parseString """..."""`
fn struct1() -> Structure {
    parse_string(
        "\n\
         # Doc 1\n\
         Some text, [collapsedRef][]. [[wiki-link]]\n\
         \n\
         ## Sub 1\n\
         \n\
         This is [inline-link](url \"title\")\n\
         \n\
         ## Sub 2\n\
         \n\
         This is a #tag\n\
         \n\
         [collapsedRef]: DefURL\n",
    )
}

#[test]
fn test_ast_shape() {
    let strukt = struct1();
    check_inline_snapshot(&strukt.ast().elements, &[
        "# Doc 1 {doc-1}",
        "[collapsedRef][]",
        "[[wiki-link]]",
        "## Sub 1 {sub-1}",
        "[inline-link](url)",
        "## Sub 2 {sub-2}",
        "#tag",
        "[collapsedRef]: DefURL",
    ]);
}

#[test]
fn test_ast_lookup() {
    let strukt = struct1();
    let sub1 = &strukt.ast().elements[3];
    assert_eq!(sub1.compact_format(), "## Sub 1 {sub-1}");

    let csub1: Vec<cst::Element> = strukt.find_concrete_for_abstract(sub1).into_iter().collect();

    check_inline_snapshot_cst(&csub1, &[
        "H2: range=(4,0)-(4,8); scope=(4,0)-(8,0)",
        "  text=`## Sub 1`",
        "  title=`Sub 1` @ (4,3)-(4,8)",
    ]);

    let should_be_sub1 = strukt.find_matching_abstract(&csub1[0]);
    assert_eq!(sub1, should_be_sub1);

    let made_up_abstract = ast::Element::MR(ast::MdRef::Collapsed("WAT".into()));
    assert_eq!(strukt.try_find_concrete_for_abstract(&made_up_abstract), None);
}

#[test]
fn test_syms_when_title_from_heading_is_off() {
    let doc = "\n# H1\nIs this a title?\n# H2\nIs this another title?\n## H2.2\n# H3\nAnd this?\n";

    let strukt = parser::parse(
        &ParserSettings { title_from_heading: false, ..ParserSettings::default() },
        &mk_text(doc),
    );

    check_inline_snapshot_syms(strukt.symbols(), &[
        "Doc",
        "H1 {h1}",
        "H1 {h2}",
        "H1 {h3}",
        "H2 {h22}",
    ]);
}

#[test]
fn test_syms_when_repeated_headings_glfm() {
    let doc = "\n# X^\n# A\n# A\n# X\n# Z\n## A\n";

    let strukt = parser::parse(
        &ParserSettings { title_from_heading: false, ..ParserSettings::default() },
        &mk_text(doc),
    );

    check_inline_snapshot_syms(strukt.symbols(), &[
        "Doc",
        "H1 {a}",
        "H1 {a-1}",
        "H1 {x}",
        "H1 {x-1}",
        "H1 {z}",
        "H2 {a-2}",
    ]);
}

#[test]
fn test_syms_when_repeated_headings_no_glfm() {
    let doc = "\n# X^\n# A\n# A\n# X\n# Z\n## A\n";

    let strukt = parser::parse(
        &ParserSettings {
            title_from_heading: false,
            glfm_heading_ids: false,
            ..ParserSettings::default()
        },
        &mk_text(doc),
    );

    check_inline_snapshot_syms(strukt.symbols(), &["Doc", "H1 {a}", "H1 {x}", "H1 {z}", "H2 {a}"]);
}

#[test]
fn test_syms_when_title_from_heading_is_on() {
    let doc = "\n# H1\nIs this a title?\n# H2\nIs this another title?\n## H2.2\n# H3\nAnd this?\n";

    let strukt = parser::parse(
        &ParserSettings { title_from_heading: true, ..ParserSettings::default() },
        &mk_text(doc),
    );

    check_inline_snapshot_syms(strukt.symbols(), &[
        "Doc",
        "T {h1}",
        "T {h2}",
        "T {h3}",
        "H2 {h22}",
    ]);
}
