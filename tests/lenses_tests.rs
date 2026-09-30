//! Port of `Tests/LensesTests.fs`.

mod common;

use lsp_types::CodeLens;

use marksman::cst;
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::lenses;
use marksman::misc;
use marksman::state::ClientDescription;

use common::{FakeDoc, FakeFolder};

/// Renders a lens the way the original's inline snapshot does, i.e. as the F#
/// `Command` record's `ToString()` followed by the range's debugger display:
///
/// ```text
/// { Title = "1 reference"
///   Command = "marksman.findReferences"
///   Arguments = None }, (0,0)-(0,7)
/// ```
fn fmt_lens(lens: &CodeLens) -> String {
    let command = lens.command.as_ref().expect("a lens always carries a command");
    let arguments = match &command.arguments {
        None => "None".to_string(),
        Some(_) => "Some [|...|]".to_string(),
    };
    format!(
        "{{ Title = \"{}\"\n  Command = \"{}\"\n  Arguments = {} }}, {}",
        command.title,
        command.command,
        arguments,
        cst::fmt_range(lens.range)
    )
}

/// `Lenses.forDoc ... |> Array.map (fun lens -> $"{lens.Command.Value}, {lens.Range}")`
/// followed by `checkInlineSnapshot id`, which splits every item into lines.
fn lens_lines(lenses: &[CodeLens]) -> Vec<String> {
    lenses
        .iter()
        .flat_map(|lens| {
            misc::lines_of(&fmt_lens(lens))
                .into_iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn check_inline_snapshot(actual: &[String], expected: &[&str]) {
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(actual, &expected);
}

/// `FakeDoc.Mk(path = ..., contentLines = [| ... |])`
fn doc_lines(path: &str, lines: &[&str]) -> Doc {
    FakeDoc::mk_at(&lines.join("\n"), path)
}

fn fixtures() -> (Doc, Doc, Folder) {
    let d1 = doc_lines("d1.md", &["# Doc 1", "## Sub", "[[#Sub]]", "## No ref"]);
    let d2 = doc_lines("d2.md", &["# Doc 2", "[[Doc 1#Sub]]"]);
    let f = FakeFolder::mk(vec![d1.clone(), d2.clone()]);
    (d1, d2, f)
}

#[test]
fn basic_header_lenses() {
    let (d1, _d2, f) = fixtures();

    let lenses = lenses::for_doc(&ClientDescription::empty(), &f, &d1);
    let lenses = lens_lines(&lenses);

    check_inline_snapshot(
        &lenses,
        &[
            "{ Title = \"1 reference\"",
            "  Command = \"marksman.findReferences\"",
            "  Arguments = None }, (0,0)-(0,7)",
            "{ Title = \"2 references\"",
            "  Command = \"marksman.findReferences\"",
            "  Arguments = None }, (1,0)-(1,6)",
        ],
    );
}

#[test]
fn basic_header_lenses_with_command_arguments() {
    let (d1, _d2, f) = fixtures();

    let mut client = ClientDescription::empty();
    client.caps.experimental =
        Some(serde_json::from_str(r#"{"codeLensFindReferences": true}"#).unwrap());

    // The original deserializes `Arguments.[0]` back into `Lenses.FindReferencesData`;
    // that type is `Serialize`-only in the Rust port, so the payload is inspected
    // as JSON instead. The asserted values are unchanged.
    let lenses_data: Vec<(String, serde_json::Value)> = lenses::for_doc(&client, &f, &d1)
        .iter()
        .map(|lens| {
            let command = lens.command.as_ref().expect("a lens carries a command");
            let arguments = command.arguments.as_ref().expect("arguments are present");
            (command.title.clone(), arguments[0].clone())
        })
        .collect();

    assert_eq!(2, lenses_data.len());
    assert_eq!("1 reference", lenses_data[0].0);
    assert_eq!(1, lenses_data[0].1["locations"].as_array().unwrap().len());
    assert_eq!("2 references", lenses_data[1].0);
    assert_eq!(2, lenses_data[1].1["locations"].as_array().unwrap().len());
}

#[test]
fn basic_link_def_lenses() {
    let d1 = doc_lines("d1.md", &["[foo]", "[foo][]", "[bar]", "", "[foo]: /url"]);

    let f = FakeFolder::mk(vec![d1.clone()]);

    let lenses = lenses::for_doc(&ClientDescription::empty(), &f, &d1);
    let lenses = lens_lines(&lenses);

    check_inline_snapshot(
        &lenses,
        &[
            "{ Title = \"2 references\"",
            "  Command = \"marksman.findReferences\"",
            "  Arguments = None }, (4,0)-(4,11)",
        ],
    );
}
