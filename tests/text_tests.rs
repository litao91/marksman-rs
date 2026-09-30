//! Port of `Tests/TextTests.fs`.
//!
//! Note on offsets: the Rust port stores *byte* offsets in the line map where
//! the original stores .NET `char` (UTF-16 code unit) offsets. Every document
//! below is pure ASCII, so the two coincide and the expected values are kept
//! verbatim. `Position.character` counts UTF-16 code units in both ports.

#![allow(non_snake_case)]

use lsp_types::{Position, Range, TextDocumentContentChangeEvent};
use marksman::text::{self, mk_text};

/// Stands in for `Text.mkRange` plus the `TextDocumentContentChangeEvent`
/// record literal of the original.
fn change(start: (u32, u32), end: (u32, u32), range_length: u32, text: &str) -> TextDocumentContentChangeEvent {
    TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position { line: start.0, character: start.1 },
            end: Position { line: end.0, character: end.1 },
        }),
        range_length: Some(range_length),
        text: text.to_string(),
    }
}

#[test]
fn lineMap_empty() {
    let lm = text::mk_line_map("");
    assert_eq!(&[(0usize, 0usize)][..], lm.map());
}

#[test]
fn lineMap_finalNewLine() {
    let lm = text::mk_line_map("\n");
    assert_eq!(&[(0usize, 1usize), (1, 1)][..], lm.map());
}

#[test]
fn lineMap_singleChar_ascii() {
    let lm = text::mk_line_map("1");
    assert_eq!(&[(0usize, 1usize), (1, 1)][..], lm.map());
}

#[test]
fn lineMap_singleLine_ascii() {
    let lm = text::mk_line_map("123456789");
    assert_eq!(&[(0usize, 9usize), (9, 9)][..], lm.map());
}

#[test]
fn lineMap_multiple_lines() {
    let lm = text::mk_line_map("12\n345\r\n6789\n");
    assert_eq!(&[(0usize, 3usize), (3, 8), (8, 13), (13, 13)][..], lm.map());
}

#[test]
fn applyTextChange_insert_single() {
    let text = mk_text("!");

    let actual = text::apply_text_change(
        &[change((0, 1), (0, 1), 0, " Holla!")],
        text,
    );

    let expected = "! Holla!";
    assert_eq!(expected, actual.content);
}

#[test]
fn applyTextChange_insert_multiple() {
    let text = mk_text("!");

    let actual = text::apply_text_change(
        &[change((0, 1), (0, 1), 0, " H"), change((0, 3), (0, 3), 0, "i")],
        text,
    );

    let expected = "! Hi";
    assert_eq!(expected, actual.content);
}

#[test]
fn applyTextChange_insert_on_empty() {
    let text = mk_text("");

    let actual = text::apply_text_change(
        &[change((0, 0), (0, 0), 0, "H")],
        text,
    );

    let expected = "H";
    assert_eq!(expected, actual.content);
}

#[test]
fn applyTextChange_insert_next_line() {
    let text = mk_text("A\n");

    let actual = text::apply_text_change(
        &[change((1, 0), (1, 0), 0, "B")],
        text,
    );

    let expected = "A\nB";
    assert_eq!(expected, actual.content);
}

#[test]
fn applyTextChange_replace_single() {
    //                      012345678901
    let text = mk_text("Hello World!");

    let actual = text::apply_text_change(
        &[change((0, 0), (0, 5), 5, "Bye")],
        text,
    );

    let expected = "Bye World!";
    assert_eq!(expected, actual.content);
}
