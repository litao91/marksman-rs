//! Document text with a line map, plus cursor/span views used for context detection.
//!
//! Port of `Marksman.Text`. Byte offsets are used internally; LSP positions count
//! UTF-16 code units, matching the .NET `char` indexing of the original.

use lsp_types::{Position, Range, TextDocumentContentChangeEvent};

use crate::misc;

pub type ByteRange = (usize, usize);

/// Byte range of every line, end-exclusive and including the line terminator.
/// A final empty entry allows insertion at the very end of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMap {
    map: Vec<ByteRange>,
}

fn utf16_len(text: &str) -> u32 {
    text.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Byte index within `line` of UTF-16 column `col`. `None` when `col` is past
/// the end of the line.
fn byte_of_utf16_col(line: &str, col: u32) -> Option<usize> {
    let mut units = 0u32;
    for (idx, c) in line.char_indices() {
        if units == col {
            return Some(idx);
        }
        units += c.len_utf16() as u32;
        if units > col {
            // Column points inside a surrogate pair; snap to the character start.
            return Some(idx);
        }
    }
    if units == col {
        return Some(line.len());
    }
    None
}

impl LineMap {
    pub fn map(&self) -> &[ByteRange] {
        &self.map
    }

    pub fn num_lines(&self) -> usize {
        self.map.len().saturating_sub(1)
    }

    pub fn try_find_position(&self, content: &str, offset: usize) -> Option<Position> {
        let line_idx = self.find_line(offset)?;
        let (start, _) = self.map[line_idx];
        Some(Position {
            line: line_idx as u32,
            character: utf16_len(&content[start..offset]),
        })
    }

    pub fn find_position(&self, content: &str, offset: usize) -> Position {
        self.try_find_position(content, offset)
            .unwrap_or_else(|| panic!("Couldn't find offset's position: {offset}"))
    }

    fn find_line(&self, offset: usize) -> Option<usize> {
        let mut low = 0i64;
        let mut high = self.map.len() as i64 - 1;
        while low <= high {
            let mid = low + (high - low) / 2;
            let (start, end) = self.map[mid as usize];
            if start > offset {
                high = mid - 1;
            } else if offset < end {
                return Some(mid as usize);
            } else if start == end && start == offset {
                return Some(mid as usize);
            } else if offset >= end {
                low = mid + 1;
            } else {
                return None;
            }
        }
        None
    }

    pub fn try_find_offset(&self, content: &str, pos: Position) -> Option<usize> {
        let &(start, end) = self.map.get(pos.line as usize)?;
        let rel = byte_of_utf16_col(&content[start..end], pos.character)?;
        let offset = start + rel;
        if offset > end {
            None
        } else {
            Some(offset)
        }
    }

    pub fn find_offset(&self, content: &str, pos: Position) -> usize {
        self.try_find_offset(content, pos)
            .unwrap_or_else(|| panic!("Position outside of line map: {pos:?}"))
    }

    pub fn find_range(&self, content: &str, range: Range) -> ByteRange {
        let start = self
            .try_find_offset(content, range.start)
            .unwrap_or_else(|| panic!("Range start outside of line map: {range:?}"));
        let end = self
            .try_find_offset(content, range.end)
            .unwrap_or_else(|| panic!("Range end outside of line map: {range:?}"));
        (start, end)
    }
}

#[derive(Clone, Debug)]
pub struct Text {
    pub content: String,
    pub line_map: LineMap,
}

impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.content == other.content
    }
}
impl Eq for Text {}

impl PartialOrd for Text {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Text {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.content.cmp(&other.content)
    }
}

impl std::hash::Hash for Text {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.content.hash(state);
    }
}

pub fn mk_line_map(content: &str) -> LineMap {
    let bytes = content.as_bytes();
    let mut map: Vec<ByteRange> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let terminates = bytes[i] == b'\n'
            || (bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] != b'\n');
        if terminates {
            map.push((start, i + 1));
            start = i + 1;
        }
        i += 1;
    }

    if start < bytes.len() {
        map.push((start, bytes.len()));
        start = bytes.len();
    }
    map.push((start, start));

    LineMap { map }
}

pub const DOCUMENT_BEGINNING: Range = Range {
    start: Position { line: 0, character: 0 },
    end: Position { line: 0, character: 0 },
};

pub fn mk_text(content: impl Into<String>) -> Text {
    let content = content.into();
    let line_map = mk_line_map(&content);
    Text { content, line_map }
}

impl Text {
    pub fn substring(&self, range: Range) -> String {
        if misc::range_is_empty(&range) {
            String::new()
        } else {
            let (s, e) = self.line_map.find_range(&self.content, range);
            self.content[s..e].to_string()
        }
    }

    /// Splits the document into the text before and after `range`.
    pub fn cutout(&self, range: Range) -> (String, String) {
        let (s, e) = self.line_map.find_range(&self.content, range);
        (self.content[..s].to_string(), self.content[e..].to_string())
    }

    pub fn char_at_pos(&self, pos: Position) -> char {
        let off = self.line_map.find_offset(&self.content, pos);
        self.char_at(off)
    }

    pub fn char_at(&self, off: usize) -> char {
        self.content[off..].chars().next().unwrap_or('\0')
    }

    /// Byte range of a line's content, excluding the line terminator.
    pub fn line_content_offsets(&self, line: usize) -> ByteRange {
        let (start, end) = self.line_map.map[line];
        if start == end {
            (start, end)
        } else if start == end - 1 {
            if self.content.as_bytes()[start] == b'\n' {
                (start, start)
            } else {
                (start, end)
            }
        } else if self.content.as_bytes()[end - 2] == b'\r' && self.content.as_bytes()[end - 1] == b'\n'
        {
            (start, end - 2)
        } else if self.content.as_bytes()[end - 1] == b'\n' {
            (start, end - 1)
        } else {
            (start, end)
        }
    }

    pub fn line_content_range(&self, line: usize) -> Range {
        let (start, end) = self.line_content_offsets(line);
        Range {
            start: Position { line: line as u32, character: 0 },
            end: Position {
                line: line as u32,
                character: utf16_len(&self.content[start..end]),
            },
        }
    }

    pub fn line_content(&self, line: usize) -> String {
        let (start, end) = self.line_content_offsets(line);
        self.content[start..end].to_string()
    }

    pub fn num_lines(&self) -> usize {
        self.line_map.num_lines()
    }

    pub fn full_range(&self) -> Range {
        let line_num = self.line_map.map.len() - 1;
        Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: line_num as u32, character: 0 },
        }
    }

    pub fn end_range(&self) -> Range {
        let line_num = self.line_map.map.len() - 1;
        let end = Position { line: line_num as u32, character: 0 };
        Range { start: end, end }
    }

    pub fn offset_to_position(&self, offset: usize) -> Position {
        self.line_map.find_position(&self.content, offset)
    }

    pub fn position_to_offset(&self, pos: Position) -> usize {
        self.line_map.find_offset(&self.content, pos)
    }

    pub fn try_position_to_offset(&self, pos: Position) -> Option<usize> {
        self.line_map.try_find_offset(&self.content, pos)
    }

    /// Converts byte offsets into a range the way the original's
    /// `sourceSpanToRange` does: the end is derived from the last *included*
    /// character, so an offset at end-of-document does not spill onto the
    /// phantom line the line map keeps for appending.
    pub fn range_of_offsets(&self, start: usize, end_exclusive: usize) -> Range {
        let start_pos = self.offset_to_position(start);
        if end_exclusive <= start {
            return Range { start: start_pos, end: start_pos };
        }
        let last = prev_char_boundary(&self.content, end_exclusive.min(self.content.len()));
        let last_pos = self.offset_to_position(last);
        let width = self.content[last..]
            .chars()
            .next()
            .map(|c| c.len_utf16() as u32)
            .unwrap_or(1);
        Range {
            start: start_pos,
            end: Position { line: last_pos.line, character: last_pos.character + width },
        }
    }
}

fn apply_change_one(text: &Text, change: &TextDocumentContentChangeEvent) -> Text {
    match change.range {
        Some(range) => {
            let start = text.line_map.find_offset(&text.content, range.start);
            let end = text.line_map.find_offset(&text.content, range.end);
            let mut new_content = String::with_capacity(text.content.len() + change.text.len());
            new_content.push_str(&text.content[..start]);
            new_content.push_str(&change.text);
            new_content.push_str(&text.content[end..]);
            mk_text(new_content)
        }
        None => mk_text(change.text.clone()),
    }
}

pub fn apply_text_change(changes: &[TextDocumentContentChangeEvent], text: Text) -> Text {
    changes.iter().fold(text, |acc, change| apply_change_one(&acc, change))
}

/// A half-open byte range within a document.
#[derive(Clone, Debug)]
pub struct Span {
    pub text: Text,
    /// Inclusive
    pub start: usize,
    /// Exclusive
    pub end: usize,
}

impl Span {
    pub fn range(&self) -> Range {
        let start = self.text.line_map.find_position(&self.text.content, self.start);
        let stop = self
            .text
            .line_map
            .find_position(&self.text.content, self.end.min(self.text.content.len()));
        Range {
            start: Position { line: start.line, character: start.character },
            end: Position { line: stop.line, character: stop.character },
        }
    }

    pub fn start_cursor(&self) -> Option<Cursor> {
        if self.start < self.end {
            Some(Cursor { span: self.clone(), pos: self.start })
        } else {
            None
        }
    }

    pub fn end_cursor(&self) -> Option<Cursor> {
        if self.start < self.end {
            Some(Cursor { span: self.clone(), pos: self.end - 1 })
        } else {
            None
        }
    }

    pub fn to_cursor_at(&self, pos: Position) -> Option<Cursor> {
        let offset = self.text.line_map.try_find_offset(&self.text.content, pos)?;
        if self.start <= offset && offset < self.end {
            Some(Cursor { span: self.clone(), pos: offset })
        } else {
            None
        }
    }

    pub fn forward(&self) -> Option<Span> {
        if self.start < self.end {
            let width = self.text.content[self.start..]
                .chars()
                .next()
                .map_or(1, |c| c.len_utf8());

            Some(Span { start: (self.start + width).min(self.end), ..self.clone() })
        } else {
            None
        }
    }

    pub fn start_char(&self) -> Option<char> {
        self.start_cursor().map(|c| c.char())
    }

    pub fn content(&self) -> &str {
        &self.text.content[self.start..self.end.min(self.text.content.len())]
    }
}

/// A byte offset into a span. Movement is character-aware so that it stays on
/// UTF-8 boundaries.
#[derive(Clone, Debug)]
pub struct Cursor {
    pub span: Span,
    pub pos: usize,
}

impl Cursor {
    pub fn char(&self) -> char {
        self.span.text.content[self.pos..].chars().next().unwrap_or('\0')
    }

    pub fn pos(&self) -> Position {
        self.span
            .text
            .line_map
            .find_position(&self.span.text.content, self.pos)
    }

    pub fn is_before_or_at(before: &Cursor, after: &Cursor) -> bool {
        before.pos <= after.pos
    }

    pub fn forward(&self) -> Option<Cursor> {
        let next = self.pos + self.char().len_utf8();
        if next < self.span.end {
            Some(Cursor { pos: next, ..self.clone() })
        } else {
            None
        }
    }

    pub fn backward(&self) -> Option<Cursor> {
        if self.pos > self.span.start {
            let prev = prev_char_boundary(&self.span.text.content, self.pos);
            Some(Cursor { pos: prev, ..self.clone() })
        } else {
            None
        }
    }

    pub fn backward_char(&self) -> Option<char> {
        self.backward().map(|c| c.char())
    }

    pub fn forward_char(&self) -> Option<char> {
        self.forward().map(|c| c.char())
    }

    pub fn forward_char2(&self) -> Option<(char, char)> {
        let c1 = self.char();
        self.forward().map(|c| (c1, c.char()))
    }

    pub fn backward_char2(&self) -> Option<(char, char)> {
        let c1 = self.char();
        self.backward().map(|c| (c.char(), c1))
    }

    pub fn forward_char_n(&self, n: usize) -> Option<Vec<char>> {
        let mut acc = Vec::new();
        let mut cursor = Some(self.clone());
        for _ in 0..n {
            let c = cursor?;
            acc.push(c.char());
            cursor = c.forward();
        }
        Some(acc)
    }

    pub fn backward_n(&self, n: usize) -> Option<Cursor> {
        let mut res = self.backward();
        for _ in 1..n {
            res = res.as_ref().and_then(|c| c.backward());
        }
        res
    }

    pub fn forward_n(&self, n: usize) -> Option<Cursor> {
        let mut res = self.forward();
        for _ in 1..n {
            res = res.as_ref().and_then(|c| c.forward());
        }
        res
    }

    pub fn to_span(&self) -> Span {
        Span { start: self.pos, ..self.span.clone() }
    }

    pub fn try_find_char_matching<F>(&self, move_fn: F, pred: impl Fn(char) -> bool) -> Option<Cursor>
    where
        F: Fn(&Cursor) -> Option<Cursor>,
    {
        let mut cursor = self.clone();
        loop {
            if pred(cursor.char()) {
                return Some(cursor);
            }
            cursor = move_fn(&cursor)?;
        }
    }
}

fn prev_char_boundary(content: &str, pos: usize) -> usize {
    let mut idx = pos - 1;
    while idx > 0 && !content.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// A single line of a document.
#[derive(Clone, Debug)]
pub struct Line {
    pub text: Text,
    pub line: usize,
}

impl Line {
    pub fn of_pos(text: Text, pos: Position) -> Option<Line> {
        if text.line_map.try_find_offset(&text.content, pos).is_some() {
            Some(Line { text, line: pos.line as usize })
        } else {
            None
        }
    }

    pub fn to_span(&self) -> Span {
        let (start, end) = self.text.line_content_offsets(self.line);
        Span { text: self.text.clone(), start, end }
    }

    pub fn to_cursor(&self) -> Option<Cursor> {
        self.to_span().start_cursor()
    }

    pub fn to_cursor_at(&self, pos: Position) -> Option<Cursor> {
        self.to_span().to_cursor_at(pos)
    }

    pub fn start_char(&self) -> Option<char> {
        self.to_span().start_char()
    }

    pub fn range(&self) -> Range {
        self.to_span().range()
    }

    pub fn start_cursor(&self) -> Option<Cursor> {
        self.to_span().start_cursor()
    }

    pub fn end_cursor(&self) -> Option<Cursor> {
        self.to_span().end_cursor()
    }

    pub fn ends_at(&self, pos: Position) -> bool {
        let span = self.to_span();
        match span.text.line_map.try_find_offset(&span.text.content, pos) {
            Some(off) => span.end == off,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn line_map_of_empty_text() {
        let text = mk_text("");
        assert_eq!(text.line_map.map(), &[(0, 0)]);
        assert_eq!(text.num_lines(), 0);
    }

    #[test]
    fn line_map_with_trailing_newline() {
        let text = mk_text("a\nb\n");
        assert_eq!(text.line_map.map(), &[(0, 2), (2, 4), (4, 4)]);
        assert_eq!(text.num_lines(), 2);
        assert_eq!(text.line_content(0), "a");
        assert_eq!(text.line_content(1), "b");
    }

    #[test]
    fn line_map_without_trailing_newline() {
        let text = mk_text("a\nb");
        assert_eq!(text.line_map.map(), &[(0, 2), (2, 3), (3, 3)]);
    }

    #[test]
    fn crlf_line_map() {
        let text = mk_text("a\r\nb");
        assert_eq!(text.line_map.map(), &[(0, 3), (3, 4), (4, 4)]);
        assert_eq!(text.line_content(0), "a");
    }

    #[test]
    fn offset_position_roundtrip() {
        let text = mk_text("# H1\nbody\n");
        assert_eq!(text.offset_to_position(0), pos(0, 0));
        assert_eq!(text.offset_to_position(5), pos(1, 0));
        assert_eq!(text.position_to_offset(pos(1, 2)), 7);
        assert_eq!(text.substring(misc::range(1, 0, 1, 4)), "body");
    }

    #[test]
    fn positions_count_utf16_units() {
        let text = mk_text("🚀ab\n");
        // The rocket is one char but two UTF-16 code units.
        assert_eq!(text.offset_to_position(4), pos(0, 2));
        assert_eq!(text.position_to_offset(pos(0, 3)), 5);
        assert_eq!(text.line_content_range(0).end.character, 4);
    }

    #[test]
    fn incremental_change_is_applied() {
        let text = mk_text("hello world");
        let change = TextDocumentContentChangeEvent {
            range: Some(misc::range(0, 5, 0, 11)),
            range_length: None,
            text: " marksman".to_string(),
        };
        let updated = apply_text_change(&[change], text);
        assert_eq!(updated.content, "hello marksman");
    }

    #[test]
    fn full_change_replaces_document() {
        let text = mk_text("old");
        let change = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "new".to_string(),
        };
        assert_eq!(apply_text_change(&[change], text).content, "new");
    }

    #[test]
    fn cursor_moves_within_span() {
        let text = mk_text("abc");
        let span = Span { text: text.clone(), start: 0, end: 3 };
        let cursor = span.start_cursor().unwrap();
        assert_eq!(cursor.char(), 'a');
        let next = cursor.forward().unwrap();
        assert_eq!(next.char(), 'b');
        assert_eq!(next.forward().unwrap().char(), 'c');
        assert!(next.forward().unwrap().forward().is_none());
        assert!(cursor.backward().is_none());
        assert_eq!(next.backward_char(), Some('a'));
    }

    #[test]
    fn line_view_reports_content_and_end() {
        let text = mk_text("first\nsecond");
        let line = Line::of_pos(text, pos(1, 0)).unwrap();
        assert_eq!(line.to_span().content(), "second");
        assert!(line.ends_at(pos(1, 6)));
        assert!(!line.ends_at(pos(1, 3)));
    }
}
