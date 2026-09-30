//! Conversion between the internal URI strings and `lsp_types::Uri`.
//!
//! The internal format follows the original implementation, which leaves
//! non-Latin-1 characters unescaped. `lsp_types::Uri` rejects those, so they are
//! percent-encoded here, at the protocol boundary.

use std::str::FromStr;

use lsp_types::Uri;

use crate::misc;

const FALLBACK: &str = "file:///";

pub fn parse(s: &str) -> Uri {
    if let Ok(uri) = Uri::from_str(s) {
        return uri;
    }

    let encoded: String = s
        .chars()
        .map(|c| {
            if (c as u32) > 0x7F {
                misc::url_encode(&c.to_string())
            } else {
                c.to_string()
            }
        })
        .collect();

    Uri::from_str(&encoded).unwrap_or_else(|_| Uri::from_str(FALLBACK).expect("valid fallback uri"))
}

pub fn as_str(uri: &Uri) -> &str {
    uri.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_uris_pass_through_unchanged() {
        assert_eq!(as_str(&parse("file:///a/b.md")), "file:///a/b.md");
        assert_eq!(as_str(&parse("file:///a%20b/c.md")), "file:///a%20b/c.md");
    }

    #[test]
    fn non_ascii_characters_are_encoded() {
        assert_eq!(
            as_str(&parse("file:///notes/日本.md")),
            "file:///notes/%E6%97%A5%E6%9C%AC.md"
        );
    }

    #[test]
    fn unparseable_input_falls_back() {
        assert_eq!(as_str(&parse("not a uri at all")), FALLBACK);
    }
}
