//! Port of `Tests/SematoTests.fs`.

mod common;

use common::{dummy_root_path, mk_doc_id, mk_folder_id};
use marksman::config::ParserSettings;
use marksman::doc::Doc;
use marksman::semato;
use marksman::text::mk_text;

/// `let nthToken (data: array<uint32>) n = data[n * 5 .. n * 5 + 4]`
/// (an F# range slice is inclusive, so this is five elements).
fn nth_token(data: &[u32], n: usize) -> &[u32] {
    &data[n * 5..n * 5 + 5]
}

#[test]
fn test_encoding() {
    let folder_path = mk_folder_id(&dummy_root_path(&["folder"]));
    let doc_path = mk_doc_id(&folder_path, &dummy_root_path(&["folder", "doc1.md"]));

    let content = "# Title
Start with [[a-wiki-link]]. Then a [ref-link].
[[wiki-at-sol]]
<blank>
End with [[wiki-link-no-eol]] and #tag.";

    let doc = Doc::mk(&ParserSettings::default(), doc_path, None, mk_text(content)).unwrap();

    let data = semato::of_index_encoded(doc.index());
    assert_eq!(5 * 5, data.len());

    assert_eq!([1u32, 13u32, 11u32, 0u32, 0u32], nth_token(&data, 0));
    assert_eq!([0u32, 23u32, 8u32, 1u32, 0u32], nth_token(&data, 1));
    assert_eq!([1u32, 2u32, 11u32, 0u32, 0u32], nth_token(&data, 2));
    assert_eq!([2u32, 11u32, 16u32, 0u32, 0u32], nth_token(&data, 3));
    assert_eq!([0u32, 23u32, 4u32, 2u32, 0u32], nth_token(&data, 4));
}
