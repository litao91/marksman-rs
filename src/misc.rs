//! String, slug, link-label and set-difference helpers shared by every module.
//!
//! Port of `Marksman.Misc`.

use std::collections::BTreeSet;

use lsp_types::{Position, Range};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use unicode_normalization::UnicodeNormalization;

/// Matches .NET's `Uri.EscapeDataString`: everything but the RFC 3986 unreserved
/// characters is percent-encoded.
pub const URI_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn concat_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    lines.collect::<Vec<_>>().join("\n")
}

/// Prefixes every line of `inner` with `indent` spaces. Port of `Indented<'A>`.
pub fn indented(indent: usize, inner: &str) -> String {
    let pad = " ".repeat(indent);
    lines_of(inner).into_iter().map(|line| format!("{pad}{line}")).collect::<Vec<_>>().join("\n")
}

pub fn lines_of(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        match rest.find('\n') {
            Some(idx) => {
                let mut line = &rest[..idx];
                if line.ends_with('\r') {
                    line = &line[..line.len() - 1];
                }
                out.push(line);
                rest = &rest[idx + 1..];
            }
            None => {
                out.push(rest);
                break;
            }
        }
    }
    out
}

/// Case-insensitive subsequence test: is `needle` a subsequence of `haystack`?
pub fn is_subsequence_of(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut needle_chars = needle.chars();
    let mut expected = match needle_chars.next() {
        Some(c) => c,
        None => return true,
    };
    for candidate in haystack.chars() {
        if expected.to_lowercase().eq(candidate.to_lowercase()) {
            match needle_chars.next() {
                Some(c) => expected = c,
                None => return true,
            }
        }
    }
    false
}

pub fn is_substring_of(needle: &str, haystack: &str) -> bool {
    haystack.contains(needle)
}

/// GitHub/GitLab-style heading slug.
///
/// `.NET` classifies characters with `Char.IsPunctuation`/`Char.IsSymbol`; the
/// alphanumeric test below agrees with it on every ASCII and CJK input that
/// reaches a heading, and only diverges for unassigned and control characters.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut sep_seen = false;
    // 0: no text chunk yet, 1: chunk in progress, 2: chunk finished.
    let mut chunk_state = 0u8;

    for c in text.trim().chars() {
        let is_sep = c.is_whitespace() || c == '-';
        let is_to_out = c.is_alphanumeric();

        if is_sep {
            sep_seen = true;
        }

        if is_to_out {
            if sep_seen && chunk_state == 2 {
                out.push('-');
                sep_seen = false;
            }
            chunk_state = 1;
            out.extend(c.to_lowercase());
        } else if chunk_state == 1 {
            chunk_state = 2;
        }
    }

    out
}

/// Escapes the characters that would otherwise terminate a wiki link.
pub fn encode_for_wiki(text: &str) -> String {
    text.replace('#', "%23")
        .replace('[', "%5B")
        .replace(']', "%5D")
        .replace('|', "%7C")
}

/// Wiki-encodes every component of a slash-separated path, dropping the leading
/// slash. Other tools expect paths without it, even though that makes them
/// ambiguous. See https://github.com/artempyanykh/marksman/issues/162.
pub fn encode_path_for_wiki(path: &str) -> String {
    let parts = path.trim_start_matches('/').split(['\\', '/']);
    parts
        .map(|p| encode_for_wiki(&url_decode(p)))
        .collect::<Vec<_>>()
        .join("/")
}

pub fn url_encode(text: &str) -> String {
    utf8_percent_encode(text, URI_ESCAPE).to_string()
}

pub fn url_decode(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().into_owned()
}

/// Like [`encode_path_for_wiki`] but percent-encodes each component and keeps a
/// leading slash.
pub fn abs_path_url_encode(path: &str) -> String {
    let parts = path.trim_start_matches('/').split(['\\', '/']);
    let encoded: Vec<String> = parts.map(|p| url_encode(&url_decode(p))).collect();
    format!("/{}", encoded.join("/"))
}

pub fn as_unix_abs_path(path: &str) -> String {
    let joined = path
        .trim_start_matches('/')
        .split(['\\', '/'])
        .collect::<Vec<_>>()
        .join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

pub fn trim_prefix<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.strip_prefix(prefix).unwrap_or(text)
}

pub fn trim_suffix<'a>(text: &'a str, suffix: &str) -> &'a str {
    text.strip_suffix(suffix).unwrap_or(text)
}

pub fn trim_both<'a>(text: &'a str, prefix: &str, suffix: &str) -> &'a str {
    trim_suffix(trim_prefix(text, prefix), suffix)
}

pub fn is_emacs_backup(path: &str) -> bool {
    file_name(path).starts_with(".#")
}

/// The extension of `path` without the dot, lowercased. Empty when absent.
pub fn path_extension(path: &str) -> String {
    match path.rfind(['/', '\\']) {
        Some(idx) => &path[idx + 1..],
        None => path,
    }
    .rsplit_once('.')
    .map(|(_, ext)| ext.to_lowercase())
    .unwrap_or_default()
}

pub fn file_name(path: &str) -> &str {
    match path.rfind(['/', '\\']) {
        Some(idx) => &path[idx + 1..],
        None => path,
    }
}

pub fn file_name_stem(path: &str) -> &str {
    let name = file_name(path);
    let ext = path_extension(path);
    if ext.is_empty() {
        name
    } else {
        &name[..name.len() - ext.len() - 1]
    }
}

pub fn is_markdown_file(exts: &[String], path: &str) -> bool {
    if is_emacs_backup(path) {
        return false;
    }
    let ext = path_extension(path);
    !ext.is_empty() && exts.iter().any(|e| *e == ext)
}

pub fn chop_markdown_ext(exts: &[String], path: &str) -> String {
    if is_markdown_file(exts, path) {
        let ext = path_extension(path);
        path[..path.len() - ext.len() - 1].to_string()
    } else {
        path.to_string()
    }
}

pub fn ensure_markdown_ext(exts: &[String], path: &str) -> String {
    if is_markdown_file(exts, path) {
        path.to_string()
    } else {
        format!("{path}.{}", exts[0])
    }
}

pub fn is_potentially_markdown_file(exts: &[String], path: &str) -> bool {
    let ext = path_extension(path);
    ext.is_empty() || is_markdown_file(exts, path)
}

/// True when `name` could denote a document inside the workspace rather than an
/// external resource.
pub fn is_potentially_internal_ref(exts: &[String], name: &str) -> bool {
    if is_well_formed_absolute_uri(name) {
        false
    } else {
        is_potentially_markdown_file(exts, name)
    }
}

/// Approximates `Uri.IsWellFormedUriString(name, UriKind.Absolute)`: a syntactically
/// valid scheme followed by a non-empty remainder.
pub fn is_well_formed_absolute_uri(name: &str) -> bool {
    let bytes = name.as_bytes();
    let mut idx = 0;
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    while idx < bytes.len() {
        let b = bytes[idx];
        if b == b':' {
            return idx > 0 && idx + 1 < bytes.len() && !name[idx + 1..].is_empty();
        }
        if !(b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
            return false;
        }
        idx += 1;
    }
    false
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slug(String);

impl Slug {
    pub fn of_string(s: &str) -> Slug {
        Slug(slugify(s))
    }

    /// Builds a slug from an already-slugified string.
    pub fn of_raw(s: impl Into<String>) -> Slug {
        Slug(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_subsequence(&self, other: &Slug) -> bool {
        is_subsequence_of(&self.0, &other.0)
    }

    pub fn is_substring(&self, other: &Slug) -> bool {
        is_substring_of(&self.0, &other.0)
    }
}

impl std::fmt::Display for Slug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn equal_slug_strings(s1: &str, s2: &str) -> bool {
    Slug::of_string(s1) == Slug::of_string(s2)
}

/// A normalized Markdown link reference label: NFC, lowercased, trimmed, with
/// whitespace runs collapsed to a single space.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkLabel(String);

impl LinkLabel {
    pub fn of_string(s: &str) -> LinkLabel {
        let normalized: String = s.nfc().collect();
        let lowered = normalized.to_lowercase();
        let trimmed = lowered.trim();
        let mut out = String::with_capacity(trimmed.len());
        let mut in_ws = false;
        for c in trimmed.chars() {
            if c.is_whitespace() {
                in_ws = true;
            } else {
                if in_ws && !out.is_empty() {
                    out.push(' ');
                }
                in_ws = false;
                out.push(c);
            }
        }
        LinkLabel(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_subsequence_of(&self, other: &LinkLabel) -> bool {
        is_subsequence_of(&self.0, &other.0)
    }
}

impl std::fmt::Display for LinkLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The symmetric difference between two sets, used to drive incremental updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference<A: Ord> {
    pub added: BTreeSet<A>,
    pub removed: BTreeSet<A>,
}

impl<A: Ord + Clone> Default for Difference<A> {
    fn default() -> Self {
        Difference::empty()
    }
}

impl<A: Ord + Clone> Difference<A> {
    pub fn empty() -> Self {
        Difference {
            added: BTreeSet::new(),
            removed: BTreeSet::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    pub fn mk(before: &BTreeSet<A>, after: &BTreeSet<A>) -> Self {
        Difference {
            added: after.difference(before).cloned().collect(),
            removed: before.difference(after).cloned().collect(),
        }
    }
}

impl<A: Ord + Clone + std::fmt::Display> Difference<A> {
    pub fn compact_format(&self) -> String {
        let mut lines: Vec<String> = Vec::new();

        if !self.added.is_empty() {
            lines.push("Added:".to_string());
            for x in &self.added {
                lines.push(indented(4, &x.to_string()));
            }
        }

        if !self.removed.is_empty() {
            lines.push("Removed:".to_string());
            for x in &self.removed {
                lines.push(indented(4, &x.to_string()));
            }
        }

        lines.join("\n")
    }
}

/// How a key appears across the two inputs of a merge.
pub enum MergeEntry<O, N> {
    OnlyBefore(O),
    OnlyAfter(N),
    Both(O, N),
}

/// Walks two key-ordered sequences without materializing their union.
pub fn sorted_merge_iter<K, O, N, F>(
    before: impl Iterator<Item = (K, O)>,
    after: impl Iterator<Item = (K, N)>,
    mut visit: F,
) where
    K: Ord,
    F: FnMut(&K, MergeEntry<O, N>),
{
    let mut old_entries = before.peekable();
    let mut new_entries = after.peekable();

    loop {
        match (old_entries.peek(), new_entries.peek()) {
            (Some((ok, _)), Some((nk, _))) => match ok.cmp(nk) {
                std::cmp::Ordering::Less => {
                    let (k, v) = old_entries.next().unwrap();
                    visit(&k, MergeEntry::OnlyBefore(v));
                }
                std::cmp::Ordering::Greater => {
                    let (k, v) = new_entries.next().unwrap();
                    visit(&k, MergeEntry::OnlyAfter(v));
                }
                std::cmp::Ordering::Equal => {
                    let (k, ov) = old_entries.next().unwrap();
                    let (_, nv) = new_entries.next().unwrap();
                    visit(&k, MergeEntry::Both(ov, nv));
                }
            },
            (Some(_), None) => {
                let (k, v) = old_entries.next().unwrap();
                visit(&k, MergeEntry::OnlyBefore(v));
            }
            (None, Some(_)) => {
                let (k, v) = new_entries.next().unwrap();
                visit(&k, MergeEntry::OnlyAfter(v));
            }
            (None, None) => break,
        }
    }
}

pub fn position(line: u32, character: u32) -> Position {
    Position { line, character }
}

pub fn range(start_line: u32, start_char: u32, end_line: u32, end_char: u32) -> Range {
    Range {
        start: position(start_line, start_char),
        end: position(end_line, end_char),
    }
}

pub fn range_is_empty(r: &Range) -> bool {
    r.start >= r.end
}

pub fn range_contains_inclusive(r: &Range, pos: Position) -> bool {
    r.start <= pos && pos <= r.end
}

/// `lsp_types::Range` is not ordered, but elements must be sortable and usable
/// as map keys, so ranges are compared by their endpoints.
pub fn cmp_range(a: &Range, b: &Range) -> std::cmp::Ordering {
    a.start.cmp(&b.start).then_with(|| a.end.cmp(&b.end))
}

pub fn next_char(pos: Position, n: u32) -> Position {
    Position {
        line: pos.line,
        character: pos.character + n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("  spaced   out  "), "spaced-out");
        assert_eq!(slugify("a.b"), "ab");
        assert_eq!(slugify("a - b"), "a-b");
        assert_eq!(slugify("a--b"), "a-b");
        assert_eq!(slugify("UPPER lower"), "upper-lower");
        assert_eq!(slugify("🚀"), "");
        assert_eq!(slugify("日本語"), "日本語");
        assert_eq!(slugify("Closed ##"), "closed");
    }

    #[test]
    fn subsequence_is_case_insensitive() {
        assert!(is_subsequence_of("abc", "AxBxC"));
        assert!(is_subsequence_of("", "anything"));
        assert!(!is_subsequence_of("abd", "axbxc"));
    }

    #[test]
    fn url_roundtrip() {
        assert_eq!(url_encode("a b/c#d"), "a%20b%2Fc%23d");
        assert_eq!(url_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(encode_for_wiki("a#b[c]|d"), "a%23b%5Bc%5D%7Cd");
    }

    #[test]
    fn link_label_normalizes_whitespace_and_case() {
        assert_eq!(LinkLabel::of_string("  A   B ").as_str(), "a b");
    }

    #[test]
    fn markdown_extension_checks() {
        let exts: Vec<String> = vec!["md".into(), "markdown".into()];
        assert!(is_markdown_file(&exts, "a/b.md"));
        assert!(is_markdown_file(&exts, "a/b.MARKDOWN"));
        assert!(!is_markdown_file(&exts, "a/.#b.md"));
        assert_eq!(chop_markdown_ext(&exts, "a/b.md"), "a/b");
        assert_eq!(ensure_markdown_ext(&exts, "a/b"), "a/b.md");
        assert!(is_potentially_internal_ref(&exts, "note"));
        assert!(!is_potentially_internal_ref(&exts, "https://x.y/z"));
        assert!(!is_potentially_internal_ref(&exts, "a.txt"));
    }

    #[test]
    fn absolute_uri_detection() {
        assert!(is_well_formed_absolute_uri("https://example.com"));
        assert!(is_well_formed_absolute_uri("mailto:a@b.c"));
        assert!(!is_well_formed_absolute_uri("note.md"));
        assert!(!is_well_formed_absolute_uri("/abs/path"));
    }

    #[test]
    fn difference_of_sets() {
        let before: BTreeSet<i32> = [1, 2].into();
        let after: BTreeSet<i32> = [2, 3].into();
        let d = Difference::mk(&before, &after);
        assert_eq!(d.added.iter().copied().collect::<Vec<_>>(), vec![3]);
        assert_eq!(d.removed.iter().copied().collect::<Vec<_>>(), vec![1]);
    }
}
