//! Port of `Tests/TocTests.fs`.
//!
//! Several fixtures in the original carry meaningful trailing whitespace; it is
//! preserved here verbatim.

mod common;

use common::{apply_document_action, strip_margin_trim, FakeDoc};
use lsp_types::Range;
use marksman::code_actions;
use marksman::config::Config;
use marksman::toc::{Entry, InsertionPoint, TableOfContents, END_MARKER, START_MARKER};

/// `Config.Default.caTocInclude |> Option.get`
fn include_all_levels() -> Vec<i32> {
    Config::default_config().ca_toc_include.unwrap()
}

// ---------------------------------------------------------------------------
// module DetectToc
// ---------------------------------------------------------------------------

#[test]
fn detect_toc_1() {
    let doc = FakeDoc::mk("# T1\n# T2");

    let titles = TableOfContents::detect(doc.text());

    assert_eq!(titles, None);
}

#[test]
fn detect_toc_no_marker() {
    let doc = FakeDoc::mk_lines(&["- [T1][#t1]", " - [T2][#t2]", "", "", "# T1", "## T2"]);

    let titles = TableOfContents::detect(doc.text());

    assert_eq!(titles, None);
}

#[test]
fn detect_toc_with_marker() {
    let doc = FakeDoc::mk_lines(&[
        START_MARKER,
        "- [T1][#t1]",
        " - [T2][#t2]",
        END_MARKER,
        "# T1",
        "## T2",
    ]);

    let toc = TableOfContents::detect(doc.text()).unwrap();
    let toc_text = doc.text().substring(toc);

    let expected_toc_text = {
        let line = |num: usize| doc.text().line_content(num);
        // The original joins with `Environment.NewLine`, which is `\n` on Linux.
        (0..=3).map(line).collect::<Vec<_>>().join("\n")
    };

    assert_eq!(toc_text.trim_end(), expected_toc_text.trim_end());
    assert_eq!(3, toc.end.line);
    assert_eq!(END_MARKER.len() as u32, toc.end.character);
}

// ---------------------------------------------------------------------------
// module CreateToc
// ---------------------------------------------------------------------------

#[test]
fn create_toc() {
    let doc = FakeDoc::mk_lines(&["# T1", "## T2"]);

    let titles = TableOfContents::mk(&include_all_levels(), doc.index()).unwrap();

    let expected = TableOfContents {
        entries: vec![Entry::mk(1, "T1".to_string()), Entry::mk(2, "T2".to_string())],
    };

    assert_eq!(expected, titles);
}

#[test]
fn create_toc_yaml_front_matter() {
    let doc = FakeDoc::mk_lines(&[
        "---",
        r#"title: "First" "#,
        r#"tags: ["1", "2"] "#,
        "---",
        "",
        "# T1",
        "## T2",
    ]);

    let titles = TableOfContents::mk(&include_all_levels(), doc.index()).unwrap();

    let expected = TableOfContents {
        entries: vec![Entry::mk(1, "T1".to_string()), Entry::mk(2, "T2".to_string())],
    };

    // Test that YAML front matter is not picked up as one of the headings
    // See https://spec.commonmark.org/0.30/#example-80 for why
    // it can be interpreted as a heading
    assert_eq!(expected, titles);
}

// ---------------------------------------------------------------------------
// module InsertToc
// ---------------------------------------------------------------------------

#[test]
fn insert_document_beginning() {
    let doc = FakeDoc::mk_lines(&["## T1", "## T2"]);

    let insertion = TableOfContents::insertion_point(&doc);

    assert_eq!(insertion, InsertionPoint::DocumentBeginning);
}

#[test]
fn insert_first_title() {
    let doc = FakeDoc::mk_lines(&["# T1", "## T2"]);

    let insertion = TableOfContents::insertion_point(&doc);
    let first_title_range: Range = doc.index().titles[0].range;

    assert_eq!(insertion, InsertionPoint::After(first_title_range));
}

#[test]
fn insert_after_yaml() {
    let doc = FakeDoc::mk_lines(&[
        "---",
        r#"title: "First" "#,
        r#"tags: ["1", "2"] "#,
        "---",
        "",
        "## T1",
        "## T2",
    ]);

    let insertion = TableOfContents::insertion_point(&doc);
    let yaml_range = doc.index().yaml_front_matter.as_ref().unwrap().range;

    assert_eq!(insertion, InsertionPoint::After(yaml_range));
}

#[test]
fn insert_after_first_title_with_yaml() {
    let doc = FakeDoc::mk_lines(&[
        "---",
        r#"title: "First" "#,
        r#"tags: ["1", "2"] "#,
        "---",
        "",
        "# T1",
        "## T2",
    ]);

    let insertion = TableOfContents::insertion_point(&doc);
    let first_title_range: Range = doc.index().titles[0].range;

    assert_eq!(insertion, InsertionPoint::After(first_title_range));
}

// ---------------------------------------------------------------------------
// module RenderToc
// ---------------------------------------------------------------------------

#[test]
fn render_create_toc() {
    let doc = FakeDoc::mk_lines(&["# T1", "## T2", "### T3", "## T4", "### T5"]);

    let titles = TableOfContents::mk(&include_all_levels(), doc.index()).unwrap().render();

    let expected_lines = [
        START_MARKER,
        "- [T1](#t1)",
        "  - [T2](#t2)",
        "    - [T3](#t3)",
        "  - [T4](#t4)",
        "    - [T5](#t5)",
        END_MARKER,
    ];

    let expected = expected_lines.join("\n");

    assert_eq!(expected, titles);
}

#[test]
fn render_create_toc_filtered_levels() {
    let doc = FakeDoc::mk_lines(&["# T1", "## T2", "### T3", "#### T4"]);

    let titles = TableOfContents::mk(&[2, 3], doc.index()).unwrap().render();

    let expected_lines = [START_MARKER, "- [T2](#t2)", "  - [T3](#t3)", END_MARKER];

    let expected = expected_lines.join("\n");

    assert_eq!(expected, titles);
}

// ---------------------------------------------------------------------------
// module DocumentEdit
// ---------------------------------------------------------------------------

#[test]
fn document_edit_insert_after_yaml() {
    let text = strip_margin_trim(
        r#"
        |---
        |title: "First"
        |tags: [1, 2]
        |---
        |
        |## T1 
        |### T2
        |## T3
        |#### T4
        "#,
    );

    let doc = FakeDoc::mk(&text);

    let action = code_actions::table_of_contents_inner(&include_all_levels(), &doc).unwrap();

    let modified_text = apply_document_action(&doc, &action);

    let expected = strip_margin_trim(&format!(
        r#"
        |---
        |title: "First"
        |tags: [1, 2]
        |---
        |
        |{START_MARKER}
        |- [T1](#t1)
        |  - [T2](#t2)
        |- [T3](#t3)
        |    - [T4](#t4)
        |{END_MARKER}
        |
        |## T1 
        |### T2
        |## T3
        |#### T4
        "#
    ));

    assert_eq!(expected, modified_text);
}

#[test]
fn document_edit_insert_after_top_heading() {
    let text = strip_margin_trim(
        r#"
        |# T1 
        |### T2 
        |
        |## T3
        |
        |#### T4"#,
    );

    let doc = FakeDoc::mk(&text);

    let action = code_actions::table_of_contents_inner(&include_all_levels(), &doc).unwrap();

    let modified_text = apply_document_action(&doc, &action);

    let expected = strip_margin_trim(&format!(
        r#"
        |# T1 
        |
        |{START_MARKER}
        |- [T1](#t1)
        |    - [T2](#t2)
        |  - [T3](#t3)
        |      - [T4](#t4)
        |{END_MARKER}
        |
        |### T2 
        |
        |## T3
        |
        |#### T4"#
    ));

    assert_eq!(expected, modified_text);
}

#[test]
fn document_edit_insert_document_beginning() {
    let text = strip_margin_trim(
        r#"
        |## T1 
        |
        |hello 
        |### T2 
        |
        |## T3
        |
        |#### T4"#,
    );

    let doc = FakeDoc::mk(&text);

    let action = code_actions::table_of_contents_inner(&include_all_levels(), &doc).unwrap();

    let modified_text = apply_document_action(&doc, &action);

    let expected = strip_margin_trim(&format!(
        r#"
        |{START_MARKER}
        |- [T1](#t1)
        |  - [T2](#t2)
        |- [T3](#t3)
        |    - [T4](#t4)
        |{END_MARKER}
        |
        |## T1 
        |
        |hello 
        |### T2 
        |
        |## T3
        |
        |#### T4"#
    ));

    assert_eq!(expected, modified_text);
}

#[test]
fn document_edit_update_at_beginning_of_file() {
    let doc = FakeDoc::mk(&strip_margin_trim(&format!(
        r#"
        |{START_MARKER}
        |- [T1](#t1)
        |{END_MARKER}
        |
        |# T2"#
    )));

    let action = code_actions::table_of_contents_inner(&include_all_levels(), &doc).unwrap();

    let modified_text = apply_document_action(&doc, &action);

    let expected = strip_margin_trim(&format!(
        r#"
        |{START_MARKER}
        |- [T2](#t2)
        |{END_MARKER}
        |
        |# T2"#
    ));

    assert_eq!(expected, modified_text);
}

#[test]
fn document_edit_up_to_date_no_update() {
    let text = strip_margin_trim(&format!(
        r#"
        |{START_MARKER}
        |- [T1](#t1)
        |{END_MARKER}
        |
        |# T1"#
    ));

    let doc = FakeDoc::mk(&text);

    let action = code_actions::table_of_contents_inner(&include_all_levels(), &doc);

    assert!(action.is_none(), "expected no action, got {action:?}");
}

#[test]
fn document_edit_up_to_date_whitespace_no_update() {
    let text = strip_margin_trim(&format!(
        r#"
        |{START_MARKER}
        |
        |- [T1](#t1)
        |{END_MARKER}
        |
        |# T1"#
    ));

    let doc = FakeDoc::mk(&text);

    let action = code_actions::table_of_contents_inner(&include_all_levels(), &doc);

    assert!(action.is_none(), "expected no action, got {action:?}");
}
