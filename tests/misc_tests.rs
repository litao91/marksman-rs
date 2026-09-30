//! Port of `Tests/MiscTests.fs`.
//!
//! The F# suite is organized in nested modules (`StringExtensionsTests`,
//! `LinkLabelTest`, `WatchGlobTest`); those are kept as Rust modules so the two
//! files can be read side by side.

#![allow(non_snake_case)]

use marksman::misc::{self, LinkLabel};
use marksman::server::mk_watch_glob;

/// `String.AbsPathUrlEncodedToRelPath` has no counterpart in the Rust port (the
/// original only uses it from tests), so it is spelled out here exactly as
/// `Marksman/Misc.fs` defines it: `this.TrimStart('/').UrlDecode()`.
fn abs_path_url_encoded_to_rel_path(encoded: &str) -> String {
    misc::url_decode(encoded.trim_start_matches('/'))
}

mod StringExtensionsTests {
    use super::*;

    #[test]
    fn isSubSequenceOf_1() {
        assert!(misc::is_subsequence_of("f", "fsharp"));
    }

    #[test]
    fn isSubSequenceOf_2() {
        assert!(misc::is_subsequence_of("fsharp", "fsharp"));
    }

    #[test]
    fn isSubSequenceOf_3() {
        assert!(misc::is_subsequence_of("fr", "fsharp"));
    }

    #[test]
    fn isSubSequenceOf_4() {
        assert!(misc::is_subsequence_of("", "fsharp"));
    }

    #[test]
    fn isSubSequenceOf_5() {
        assert!(!misc::is_subsequence_of("Md", "fsharp"));
    }

    #[test]
    fn slug_1() {
        assert_eq!(
            misc::slugify("# Header with words, and some; punctuation$%@?"),
            "header-with-words-and-some-punctuation"
        );
    }

    #[test]
    fn slug_2() {
        assert_eq!(
            misc::slugify("## --Текст на другом языке. Как тебе такое, Илон Маск?!"),
            "текст-на-другом-языке-как-тебе-такое-илон-маск"
        );
    }

    #[test]
    fn slug_3() {
        assert_eq!(
            misc::slugify("#-wow-some-hyphens- in-the- -middle"),
            "wow-some-hyphens-in-the-middle"
        );
    }

    #[test]
    fn slug_4() {
        assert_eq!(misc::slugify("multi-------hyphens"), "multi-hyphens");
    }

    #[test]
    fn slug_5() {
        assert_eq!(misc::slugify(""), "");
    }

    #[test]
    fn lines_1() {
        assert_eq!(misc::lines_of("Line"), ["Line"]);
    }

    #[test]
    fn lines_2() {
        assert_eq!(misc::lines_of("Line 1\nLine 2"), ["Line 1", "Line 2"]);
    }

    #[test]
    fn lines_3() {
        assert_eq!(
            misc::lines_of("Line 1\nLine 2\r\nLine 3"),
            ["Line 1", "Line 2", "Line 3"]
        );
    }

    #[test]
    fn abspath_urlencode_1() {
        assert_eq!(misc::abs_path_url_encode("file.md"), "/file.md");
        assert_eq!("file.md", abs_path_url_encoded_to_rel_path("/file.md"));
    }

    #[test]
    fn abspath_urlencode_2() {
        assert_eq!(misc::abs_path_url_encode("/file.md"), "/file.md");
    }

    #[test]
    fn abspath_urlencode_3() {
        assert_eq!(
            misc::abs_path_url_encode("file with spaces.md"),
            "/file%20with%20spaces.md"
        );
        assert_eq!(
            "file with spaces.md",
            abs_path_url_encoded_to_rel_path("/file%20with%20spaces.md")
        );
    }

    #[test]
    fn abspath_urlencode_4() {
        assert_eq!(misc::abs_path_url_encode("file#name.md"), "/file%23name.md");
        assert_eq!(
            "file#name.md",
            abs_path_url_encoded_to_rel_path("/file%23name.md")
        );
    }

    #[test]
    fn abspath_urlencode_5() {
        assert_eq!(
            misc::abs_path_url_encode("folder name/file name.md"),
            "/folder%20name/file%20name.md"
        );

        assert_eq!(
            "folder name/file name.md",
            abs_path_url_encoded_to_rel_path("/folder%20name/file%20name.md")
        );
    }

    #[test]
    fn trimSuffix_1() {
        assert_eq!(misc::trim_suffix("foobar", "bar"), "foo");
    }

    #[test]
    fn trimSuffix_2() {
        assert_eq!(misc::trim_suffix("foobar", "baz"), "foobar");
    }

    #[test]
    fn encodeForWiki_1() {
        assert_eq!(misc::encode_for_wiki("blah blah"), "blah blah");
    }

    #[test]
    fn encodeForWiki_2() {
        let original = "blah #blah [] () |";
        let expected = "blah %23blah %5B%5D () %7C";
        let actual = misc::encode_for_wiki(original);
        assert_eq!(expected, actual);
        assert_eq!(original, misc::url_decode(&actual));
    }
}

mod LinkLabelTest {
    use super::*;

    #[test]
    fn caseSensitivity() {
        assert_eq!(LinkLabel::of_string("hello"), LinkLabel::of_string("HELLO"));
    }

    #[test]
    fn consecutiveWhitespace() {
        assert_eq!(
            LinkLabel::of_string("H e ll o"),
            LinkLabel::of_string("H  e    ll  o")
        );
    }

    #[test]
    fn surroundingWhitespace() {
        assert_eq!(LinkLabel::of_string("abc"), LinkLabel::of_string("  abc "));
    }
}

mod WatchGlobTest {
    use super::*;

    #[test]
    fn test1() {
        let exts: Vec<String> =
            vec!["md".to_string(), "markdown".to_string(), "mdx".to_string()];
        assert_eq!(mk_watch_glob(&exts), "**/*.{md,markdown,mdx}");
    }
}
