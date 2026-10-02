//! A CommonMark-subset block and inline parser.
//!
//! This replaces the third-party Markdown parser with one written for what
//! marksman actually needs. It does not build a document tree; it produces the
//! facts the CST is made of — heading blocks, link occurrences, link reference
//! definitions, front matter, and the source regions that inline scanning must
//! skip — all as byte offsets into the original content.
//!
//! Span conventions follow Markdig, not CommonMark, because that is what the
//! original marksman reports:
//!
//! * an ATX heading's block span excludes its closing `##` sequence
//! * a setext heading's block span includes its underline
//! * a link title's span includes the surrounding quotes while its text does not
//! * a `<...>` destination's span includes the angle brackets while its text
//!   does not
//! * a label's span is trimmed of surrounding whitespace
//! * an empty label reports Markdig's zeroed span, which renders as the document
//!   origin
//!
//! Reference links are recognised without consulting the definitions: the
//! original patches Markdig's link parser to emit a link even when no definition
//! matches, so `[foo]` is always a shortcut link.

/// The kind of link, mirroring the distinctions the CST makes. Autolinks and
/// inline HTML are not included: the original produces no element for them, only
/// a region that scanning must skip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// `[label](url "title")`
    Inline,
    /// `[label][ref]`
    FullReference,
    /// `[label][]`
    Collapsed,
    /// `[label]`
    Shortcut,
}

/// A piece of link syntax whose text and whose reported span differ: the text
/// excludes delimiters that the span includes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpannedText {
    pub text: (usize, usize),
    pub range: (usize, usize),
}

#[derive(Clone, Debug)]
pub struct HeadingFact {
    /// Markdig's `HeadingBlock.Span`.
    pub block: (usize, usize),
    pub level: i32,
    /// The region whose inline content is parsed. For a setext heading this is
    /// the title line alone, without the underline.
    pub content: (usize, usize),
}

#[derive(Clone, Debug)]
pub struct LinkFact {
    pub full: (usize, usize),
    pub kind: LinkKind,
    pub is_image: bool,
    pub label: SpannedText,
    /// The destination. For a full reference link this is the `[ref]` label,
    /// which is what the original reports as the link's url.
    pub url: Option<SpannedText>,
    pub title: Option<SpannedText>,
}

#[derive(Clone, Debug)]
pub struct LinkDefFact {
    pub full: (usize, usize),
    pub label: (usize, usize),
    pub url: SpannedText,
    pub title: Option<SpannedText>,
}

#[derive(Clone, Debug, Default)]
pub struct Scanned {
    pub front_matter: Option<(usize, usize)>,
    pub headings: Vec<HeadingFact>,
    pub links: Vec<LinkFact>,
    pub link_defs: Vec<LinkDefFact>,
    pub exclusions: Vec<(usize, usize)>,
}

// ---------------------------------------------------------------------------
// Lines
// ---------------------------------------------------------------------------

/// One line's inline-content region: the part left after any container marker.
#[derive(Clone, Copy, Debug)]
struct LineSpan {
    /// First byte of the content region.
    start: usize,
    /// One past the last byte of the content region, excluding the terminator.
    end: usize,
    /// One past the line terminator.
    next: usize,
}

fn split_lines(content: &str) -> Vec<LineSpan> {
    let bytes = content.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;

    while start <= bytes.len() {
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }

        let has_newline = end < bytes.len();
        // A CRLF terminator does not belong to the content region.
        let text_end = if has_newline && end > start && bytes[end - 1] == b'\r' {
            end - 1
        } else {
            end
        };
        let next = if has_newline { end + 1 } else { end };

        lines.push(LineSpan { start, end: text_end, next });

        if !has_newline {
            break;
        }
        start = next;
    }

    lines
}

/// Width of the leading whitespace of `content[span.start..span.end]`, counting
/// a tab as four columns.
fn indent_width(content: &str, span: &LineSpan) -> usize {
    let mut width = 0;
    for b in content[span.start..span.end].bytes() {
        match b {
            b' ' => width += 1,
            b'\t' => width += 4 - (width % 4),
            _ => break,
        }
    }
    width
}

fn is_blank(content: &str, span: &LineSpan) -> bool {
    content[span.start..span.end].trim().is_empty()
}

// ---------------------------------------------------------------------------
// Segments: inline content that may span several non-contiguous source ranges
// ---------------------------------------------------------------------------

/// The concatenation of a block's per-line content regions, with a mapping back
/// to source offsets. A paragraph inside a blockquote is not contiguous in the
/// source, but its inline content is parsed as one string.
struct Segments {
    text: String,
    /// Cumulative virtual length before each span.
    cum: Vec<usize>,
    spans: Vec<(usize, usize)>,
}

impl Segments {
    fn new(content: &str, lines: &[LineSpan]) -> Segments {
        let mut text = String::new();
        let mut cum = Vec::with_capacity(lines.len());
        let mut spans = Vec::with_capacity(lines.len());

        for (idx, line) in lines.iter().enumerate() {
            cum.push(text.len());
            text.push_str(&content[line.start..line.end]);
            spans.push((line.start, line.end));
            if idx + 1 < lines.len() {
                // A line break inside a paragraph is part of its inline content.
                // It maps back to the end of the line it terminates.
                text.push('\n');
            }
        }

        Segments { text, cum, spans }
    }

    /// The source offset of a virtual offset.
    fn source(&self, virtual_offset: usize) -> usize {
        let idx = match self.cum.binary_search(&virtual_offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let idx = idx.min(self.spans.len().saturating_sub(1));
        let (start, end) = self.spans[idx];
        let local = virtual_offset.saturating_sub(self.cum[idx]);
        (start + local).min(end.max(start))
    }

}

// ---------------------------------------------------------------------------
// Leaf-block recognition
// ---------------------------------------------------------------------------

fn is_front_matter_delimiter(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed == "---" || trimmed == "..."
}

/// `#{1,6}` followed by whitespace or end of line.
fn atx_level(text: &str) -> Option<i32> {
    let bytes = text.as_bytes();
    let mut hashes = 0;
    while hashes < bytes.len() && bytes[hashes] == b'#' {
        hashes += 1;
    }
    if hashes == 0 || hashes > 6 {
        return None;
    }
    if hashes == bytes.len() || bytes[hashes] == b' ' || bytes[hashes] == b'\t' {
        Some(hashes as i32)
    } else {
        None
    }
}

/// Three or more `*`, `-` or `_`, and nothing else but whitespace.
fn is_thematic_break(text: &str) -> bool {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    let marker = bytes[0];
    if marker != b'*' && marker != b'-' && marker != b'_' {
        return false;
    }
    let mut count = 0;
    for &b in bytes {
        if b == marker {
            count += 1;
        } else if b != b' ' && b != b'\t' {
            return false;
        }
    }
    count >= 3
}

fn fence_marker(text: &str) -> Option<(u8, usize)> {
    let bytes = text.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    let indent = idx;
    if indent > 3 || idx >= bytes.len() {
        return None;
    }
    let marker = bytes[idx];
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let mut count = 0;
    while idx < bytes.len() && bytes[idx] == marker {
        count += 1;
        idx += 1;
    }
    if count < 3 {
        return None;
    }
    // A backtick fence's info string may not itself contain a backtick.
    if marker == b'`' && bytes[idx..].contains(&b'`') {
        return None;
    }
    Some((marker, count))
}

/// A `$$` line opens display math, which the inline pass cannot see because it
/// spans lines.
fn math_block_opens(text: &str) -> bool {
    text.trim_start().starts_with("$$")
}

fn is_closing_fence(text: &str, marker: u8, count: usize) -> bool {
    let bytes = text.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    if idx > 3 {
        return false;
    }
    let mut run = 0;
    while idx < bytes.len() && bytes[idx] == marker {
        run += 1;
        idx += 1;
    }
    run >= count && text[idx..].trim().is_empty()
}

/// A setext underline: a run of `=` or `-` and nothing else but whitespace.
fn setext_level(text: &str) -> Option<i32> {
    let bytes = text.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    if idx > 3 || idx >= bytes.len() {
        return None;
    }
    let marker = bytes[idx];
    if marker != b'=' && marker != b'-' {
        return None;
    }
    while idx < bytes.len() && bytes[idx] == marker {
        idx += 1;
    }
    if text[idx..].trim().is_empty() {
        Some(if marker == b'=' { 1 } else { 2 })
    } else {
        None
    }
}

/// A bullet (`-`, `*`, `+`) or ordered (`1.` / `1)`) list marker, returning the
/// offset just past the marker and its following whitespace.
///
/// Markdig does not read a bare `-` with no content as a list item, which is why
/// `-\n-` becomes a setext heading rather than two empty items.
fn list_marker(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    let indent = idx;
    if indent > 3 {
        return None;
    }

    let marker_end;
    let mut digits = 0;
    if bytes[idx].is_ascii_digit() {
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
            digits += 1;
        }
        if digits > 9 || idx >= bytes.len() || (bytes[idx] != b'.' && bytes[idx] != b')') {
            return None;
        }
        idx += 1;
        marker_end = idx;
    } else if matches!(bytes.get(idx), Some(b'-') | Some(b'*') | Some(b'+')) {
        idx += 1;
        marker_end = idx;
    } else {
        return None;
    }

    // Whitespace must follow the marker, and the item must have content: a bare
    // marker on its own line stays a paragraph.
    let mut spaces = 0;
    let mut probe = marker_end;
    while probe < bytes.len() && (bytes[probe] == b' ' || bytes[probe] == b'\t') {
        spaces += 1;
        probe += 1;
    }
    if spaces == 0 {
        return None;
    }
    if probe >= bytes.len() {
        return None;
    }
    if spaces > 4 {
        // More than four spaces of padding leaves the content as its own block.
        Some((marker_end, marker_end + 1))
    } else {
        Some((marker_end, probe))
    }
}

const HTML_KNOWN_TAGS: &[&str] = &[
    "address", "article", "aside", "base", "basefont", "blockquote", "body", "caption", "center",
    "col", "colgroup", "dd", "details", "dialog", "dir", "div", "dl", "dt", "fieldset",
    "figcaption", "figure", "footer", "form", "frame", "frameset", "h1", "h2", "h3", "h4", "h5",
    "h6", "head", "header", "hr", "html", "iframe", "legend", "li", "link", "main", "menu",
    "menuitem", "nav", "noframes", "ol", "optgroup", "option", "p", "param", "search", "section",
    "summary", "table", "tbody", "td", "tfoot", "th", "thead", "title", "tr", "track", "ul",
];

/// Which HTML block start condition applies, if any. The variants carry both the
/// end rule and whether the condition may interrupt a paragraph: CommonMark
/// forbids condition 7 (an arbitrary complete tag on its own line) from doing
/// so, which is why `<blank>` inside a paragraph is inline HTML and not a block.
enum HtmlBlockEnd {
    /// Conditions 1-5: ends at a line containing the token.
    Token(&'static str),
    /// Condition 6: a tag of a known name, ends at the next blank line.
    KnownTag,
    /// Condition 7: any complete tag alone on a line, ends at the next blank
    /// line, and may not interrupt a paragraph.
    CompleteTag,
}

impl HtmlBlockEnd {
    fn may_interrupt_paragraph(&self) -> bool {
        !matches!(self, HtmlBlockEnd::CompleteTag)
    }
}

/// Whether `s` begins with `prefix`, ignoring ASCII case. Comparison is done on
/// bytes so that no lowercased copy of the line has to be allocated.
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Whether `s` begins with `<` followed by `tag`, ignoring ASCII case.
fn starts_with_open_tag(s: &str, tag: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.first() == Some(&b'<')
        && bytes.len() >= tag.len() + 1
        && bytes[1..tag.len() + 1].eq_ignore_ascii_case(tag.as_bytes())
}

fn html_block_end(text: &str) -> Option<HtmlBlockEnd> {
    let trimmed = text.trim_start();
    let bytes = trimmed.as_bytes();

    // Every HTML block start condition begins with `<`, so most lines of most
    // documents stop here without any further work.
    if bytes.first() != Some(&b'<') {
        return None;
    }

    for &(tag, token) in
        &[("script", "</script>"), ("pre", "</pre>"), ("style", "</style>"), ("textarea", "</textarea>")]
    {
        if starts_with_open_tag(trimmed, tag) {
            match bytes.get(tag.len() + 1) {
                None | Some(b' ') | Some(b'\t') | Some(b'>') | Some(b'\n') => {
                    return Some(HtmlBlockEnd::Token(token));
                }
                _ => {}
            }
        }
    }

    if starts_with_ci(trimmed, "<!--") {
        return Some(HtmlBlockEnd::Token("-->"));
    }
    if starts_with_ci(trimmed, "<?") {
        return Some(HtmlBlockEnd::Token("?>"));
    }
    if starts_with_ci(trimmed, "<![cdata[") {
        return Some(HtmlBlockEnd::Token("]]>"));
    }
    if starts_with_ci(trimmed, "<!") && bytes.get(2).is_some_and(|b| b.is_ascii_uppercase()) {
        return Some(HtmlBlockEnd::Token(">"));
    }

    // Condition 6: a complete open or close tag of a known name.
    let mut idx = 1;
    if bytes.get(idx) == Some(&b'/') {
        idx += 1;
    }
    let name_start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_alphanumeric() {
        idx += 1;
    }
    if idx > name_start {
        let name = &trimmed[name_start..idx];
        let known = HTML_KNOWN_TAGS.iter().any(|tag| tag.eq_ignore_ascii_case(name));
        if known && matches!(bytes.get(idx), None | Some(b' ') | Some(b'\t') | Some(b'>') | Some(b'/'))
        {
            return Some(HtmlBlockEnd::KnownTag);
        }
    }

    // Condition 7: a complete tag on a line of its own.
    if is_complete_tag_alone(trimmed) {
        return Some(HtmlBlockEnd::CompleteTag);
    }

    None
}

fn is_complete_tag_alone(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'<') {
        return false;
    }
    let mut idx = 1;
    if bytes.get(idx) == Some(&b'/') {
        idx += 1;
    }
    if !bytes.get(idx).is_some_and(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    while idx < bytes.len() && bytes[idx].is_ascii_alphanumeric() {
        idx += 1;
    }
    // Attributes.
    loop {
        let ws_start = idx;
        while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
            idx += 1;
        }
        if idx == ws_start {
            break;
        }
        if !bytes.get(idx).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b':') {
            break;
        }
        while idx < bytes.len()
            && (bytes[idx].is_ascii_alphanumeric() || matches!(bytes[idx], b'_' | b':' | b'.' | b'-'))
        {
            idx += 1;
        }
        if bytes.get(idx) == Some(&b'=') {
            idx += 1;
            while matches!(bytes.get(idx), Some(b' ') | Some(b'\t')) {
                idx += 1;
            }
            match bytes.get(idx) {
                Some(b'"') | Some(b'\'') => {
                    let quote = bytes[idx];
                    idx += 1;
                    while idx < bytes.len() && bytes[idx] != quote {
                        idx += 1;
                    }
                    if idx >= bytes.len() {
                        return false;
                    }
                    idx += 1;
                }
                Some(b'>') | None => return false,
                _ => {
                    while idx < bytes.len()
                        && !matches!(bytes[idx], b' ' | b'\t' | b'>' | b'"' | b'\'' | b'=')
                    {
                        idx += 1;
                    }
                }
            }
        }
    }
    while matches!(bytes.get(idx), Some(b' ') | Some(b'\t')) {
        idx += 1;
    }
    if bytes.get(idx) == Some(&b'/') {
        idx += 1;
    }
    bytes.get(idx) == Some(&b'>') && text[idx + 1..].trim().is_empty()
}

// ---------------------------------------------------------------------------
// The scanner
// ---------------------------------------------------------------------------

pub fn scan(content: &str) -> Scanned {
    let mut scanner = Scanner { content, out: Scanned::default() };
    let lines = split_lines(content);
    scanner.parse_blocks(&lines, true);
    scanner.out.exclusions.sort_unstable();
    scanner.out
}

struct Scanner<'a> {
    content: &'a str,
    out: Scanned,
}

impl<'a> Scanner<'a> {
    fn exclude(&mut self, start: usize, end: usize) {
        if end > start {
            self.out.exclusions.push((start, end));
        }
    }

    /// Parses blocks whose content regions are `lines`. `allow_front_matter` is
    /// true only at the very start of the document.
    fn parse_blocks(&mut self, lines: &[LineSpan], allow_front_matter: bool) {
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];
            let text = &self.content[line.start..line.end];

            if is_blank(self.content, &line) {
                i += 1;
                continue;
            }

            if allow_front_matter && i == 0 && line.start == 0 && is_front_matter_delimiter(text) {
                if let Some(consumed) = self.front_matter(lines, i) {
                    i += consumed;
                    continue;
                }
            }

            if indent_width(self.content, &line) >= 4 {
                i = self.indented_code(lines, i);
                continue;
            }

            if is_thematic_break(text) {
                i += 1;
                continue;
            }

            if atx_level(text.trim_start()).is_some() && indent_width(self.content, &line) < 4 {
                self.atx_heading(&line);
                i += 1;
                continue;
            }

            if let Some((marker, count)) = fence_marker(text) {
                i = self.fenced_code(lines, i, marker, count);
                continue;
            }

            if let Some(end) = html_block_end(text) {
                i = self.html_block(lines, i, end);
                continue;
            }

            if math_block_opens(text) {
                i = self.math_block(lines, i);
                continue;
            }

            if let Some(rest) = self.blockquote_start(&line) {
                i = self.blockquote(lines, i, rest);
                continue;
            }

            if list_marker(text).is_some() {
                i = self.list_item(lines, i);
                continue;
            }

            if let Some(consumed) = self.link_definitions(lines, i) {
                i += consumed;
                continue;
            }

            i = self.paragraph(lines, i);
        }
    }

    /// Returns the number of lines consumed, or `None` if there is no closing
    /// delimiter and this is not front matter after all.
    fn front_matter(&mut self, lines: &[LineSpan], start: usize) -> Option<usize> {
        let mut i = start + 1;
        while i < lines.len() {
            let text = &self.content[lines[i].start..lines[i].end];
            if is_front_matter_delimiter(text) {
                // An empty block between the delimiters is not front matter;
                // both `---` lines are then thematic breaks.
                if i == start + 1 {
                    return None;
                }
                let end = lines[i].end;
                self.out.front_matter = Some((lines[start].start, end));
                self.exclude(lines[start].start, end);
                return Some(i - start + 1);
            }
            i += 1;
        }
        None
    }

    fn indented_code(&mut self, lines: &[LineSpan], start: usize) -> usize {
        let mut i = start;
        let mut last = start;

        while i < lines.len() {
            if is_blank(self.content, &lines[i]) {
                // A blank line only belongs to the block if code follows it.
                let mut probe = i;
                while probe < lines.len() && is_blank(self.content, &lines[probe]) {
                    probe += 1;
                }
                if probe < lines.len() && indent_width(self.content, &lines[probe]) >= 4 {
                    i = probe;
                    continue;
                }
                break;
            }
            if indent_width(self.content, &lines[i]) < 4 {
                break;
            }
            last = i;
            i += 1;
        }

        self.exclude(lines[start].start, lines[last].next);
        i
    }

    fn fenced_code(&mut self, lines: &[LineSpan], start: usize, marker: u8, count: usize) -> usize {
        let mut i = start + 1;
        while i < lines.len() {
            let text = &self.content[lines[i].start..lines[i].end];
            if is_closing_fence(text, marker, count) {
                self.exclude(lines[start].start, lines[i].next);
                return i + 1;
            }
            i += 1;
        }
        // An unclosed fence runs to the end of the document.
        self.exclude(lines[start].start, lines[lines.len() - 1].next);
        lines.len()
    }

    fn html_block(&mut self, lines: &[LineSpan], start: usize, end: HtmlBlockEnd) -> usize {
        let mut i = start;
        while i < lines.len() {
            let text = &self.content[lines[i].start..lines[i].end];
            let stop = match end {
                HtmlBlockEnd::Token(token) => text.to_ascii_lowercase().contains(token),
                HtmlBlockEnd::KnownTag | HtmlBlockEnd::CompleteTag => {
                    i > start && is_blank(self.content, &lines[i])
                }
            };
            if stop {
                let block_end = match end {
                    HtmlBlockEnd::Token(_) => lines[i].next,
                    // A blank line terminates the block but is not part of it.
                    HtmlBlockEnd::KnownTag | HtmlBlockEnd::CompleteTag => lines[i - 1].next,
                };
                self.exclude(lines[start].start, block_end);
                return if matches!(end, HtmlBlockEnd::Token(_)) { i + 1 } else { i };
            }
            i += 1;
        }
        self.exclude(lines[start].start, lines[lines.len() - 1].next);
        lines.len()
    }

    /// The offset just past a `>` marker, if this line opens or continues a
    /// blockquote.
    /// A `$$` line opens a display-math block running to the next `$$` line, or
    /// to the end of the document if it is never closed.
    fn math_block(&mut self, lines: &[LineSpan], start: usize) -> usize {
        let text = self.content[lines[start].start..lines[start].end].trim();
        if text.len() > 4 && text.ends_with("$$") {
            // Opened and closed on one line.
            self.exclude(lines[start].start, lines[start].end);
            return start + 1;
        }

        let mut i = start + 1;
        while i < lines.len() {
            let body = &self.content[lines[i].start..lines[i].end];
            if body.trim_start().starts_with("$$") {
                self.exclude(lines[start].start, lines[i].end);
                return i + 1;
            }
            i += 1;
        }

        self.exclude(lines[start].start, lines[lines.len() - 1].end);
        lines.len()
    }

    fn blockquote_start(&self, line: &LineSpan) -> Option<usize> {
        let bytes = self.content[line.start..line.end].as_bytes();
        let mut idx = 0;
        while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
            idx += 1;
        }
        if idx > 3 || bytes.get(idx) != Some(&b'>') {
            return None;
        }
        idx += 1;
        if bytes.get(idx) == Some(&b' ') {
            idx += 1;
        }
        Some(line.start + idx)
    }

    fn blockquote(&mut self, lines: &[LineSpan], start: usize, first_rest: usize) -> usize {
        let mut inner: Vec<LineSpan> = Vec::new();
        let mut i = start;

        let first = lines[i];
        inner.push(LineSpan { start: first_rest, end: first.end, next: first.next });
        i += 1;

        while i < lines.len() {
            let line = lines[i];
            if is_blank(self.content, &line) {
                break;
            }
            if let Some(rest) = self.blockquote_start(&line) {
                inner.push(LineSpan { start: rest, end: line.end, next: line.next });
            } else {
                // Lazy continuation: a paragraph line carries into the quote.
                if inner.is_empty() || is_thematic_break(&self.content[line.start..line.end]) {
                    break;
                }
                inner.push(line);
            }
            i += 1;
        }

        self.parse_blocks(&inner, false);
        i
    }

    fn list_item(&mut self, lines: &[LineSpan], start: usize) -> usize {
        let text = &self.content[lines[start].start..lines[start].end];
        let (_, content_start) = match list_marker(text) {
            Some(pair) => pair,
            None => return start + 1,
        };
        let first = lines[start];
        // The item's content is indented to where it starts on the first line.
        let item_indent = content_start;

        let mut inner = vec![LineSpan {
            start: first.start + content_start,
            end: first.end,
            next: first.next,
        }];

        let mut i = start + 1;
        while i < lines.len() {
            let line = lines[i];

            if is_blank(self.content, &line) {
                // A blank line only belongs to the item if indented content
                // follows it; otherwise the item ends here.
                let mut probe = i;
                while probe < lines.len() && is_blank(self.content, &lines[probe]) {
                    probe += 1;
                }
                if probe < lines.len() && indent_width(self.content, &lines[probe]) >= item_indent
                {
                    for blank in i..probe {
                        inner.push(LineSpan {
                            start: lines[blank].end,
                            end: lines[blank].end,
                            next: lines[blank].next,
                        });
                    }
                    i = probe;
                    continue;
                }
                break;
            }

            if indent_width(self.content, &line) >= item_indent {
                let offset = skip_indent(self.content, &line, item_indent);
                inner.push(LineSpan { start: offset, end: line.end, next: line.next });
                i += 1;
                continue;
            }

            // A sibling item or a new block at lower indent ends this item; a
            // plain line is a lazy continuation of its paragraph.
            let text = &self.content[line.start..line.end];
            if list_marker(text).is_some()
                || self.blockquote_start(&line).is_some()
                || is_thematic_break(text)
                || atx_level(text.trim_start()).is_some()
                || fence_marker(text).is_some()
            {
                break;
            }
            inner.push(line);
            i += 1;
        }

        self.parse_blocks(&inner, false);
        i
    }

    /// Parses one or more link reference definitions starting at `start`,
    /// returning how many lines they consumed.
    fn link_definitions(&mut self, lines: &[LineSpan], start: usize) -> Option<usize> {
        let mut consumed = 0;
        let mut any = false;

        while start + consumed < lines.len() {
            match self.one_link_definition(lines, start + consumed) {
                Some(n) => {
                    consumed += n;
                    any = true;
                }
                None => break,
            }
        }

        if any {
            Some(consumed)
        } else {
            None
        }
    }

    /// Tries to parse a single definition beginning on line `start`. A
    /// definition may span up to three lines and must not be followed by more
    /// content on its last line.
    fn one_link_definition(&mut self, lines: &[LineSpan], start: usize) -> Option<usize> {
        let mut spans: Vec<LineSpan> = Vec::new();
        let mut i = start;
        while i < lines.len() && spans.len() < 3 {
            if !spans.is_empty() && is_blank(self.content, &lines[i]) {
                break;
            }
            spans.push(lines[i]);
            i += 1;
        }
        if spans.is_empty() {
            return None;
        }

        let segments = Segments::new(self.content, &spans);
        let parsed = parse_link_definition(&segments)?;

        // Nothing but whitespace may follow on the line the definition ends on;
        // a further definition may start on the next one.
        let rest = segments.text[parsed.end..].split('\n').next().unwrap_or("");
        if !rest.trim().is_empty() {
            return None;
        }

        let to_span = |t: SpannedText| SpannedText {
            text: (segments.source(t.text.0), segments.source(t.text.1)),
            range: (segments.source(t.range.0), segments.source(t.range.1)),
        };

        let fact = LinkDefFact {
            full: (segments.source(0), segments.source(parsed.end)),
            label: (segments.source(parsed.label.0), segments.source(parsed.label.1)),
            url: to_span(parsed.url),
            title: parsed.title.map(to_span),
        };

        // How many whole lines the definition covered.
        let last = parsed.end.saturating_sub(1);
        let used = segments
            .cum
            .iter()
            .enumerate()
            .filter(|(_, cum)| last >= **cum)
            .map(|(idx, _)| idx + 1)
            .max()
            .unwrap_or(1);

        self.exclude(fact.full.0, fact.full.1);
        self.out.link_defs.push(fact);
        Some(used)
    }

    fn atx_heading(&mut self, line: &LineSpan) {
        let text = &self.content[line.start..line.end];
        let trimmed_start = text.len() - text.trim_start().len();
        let level = match atx_level(&text[trimmed_start..]) {
            Some(level) => level,
            None => return,
        };

        let body_start = trimmed_start + level as usize;

        // A closing sequence of `#`s stays out of the span, together with the
        // whitespace on either side of it. Trailing whitespace with no closing
        // sequence stays in, which is what Markdig reports.
        let after_hashes = &text[body_start..];
        let stripped = after_hashes.trim_end();
        let hash_run = stripped.len() - stripped.trim_end_matches('#').len();
        let body = if hash_run > 0
            && (hash_run == stripped.len()
                || stripped.as_bytes()[stripped.len() - hash_run - 1] == b' ')
        {
            stripped[..stripped.len() - hash_run].trim_end()
        } else {
            after_hashes
        };

        // The span starts at the first `#`, not at the line's indentation.
        let block_start = line.start + trimmed_start;
        let block_end = line.start + body_start + body.len();
        let content_start = line.start + body_start;

        self.out.headings.push(HeadingFact {
            block: (block_start, block_end),
            level,
            content: (content_start, block_end),
        });

        let segments = Segments::new(
            self.content,
            &[LineSpan { start: content_start, end: block_end, next: block_end }],
        );
        self.inline(&segments, None);
    }

    /// Consumes a paragraph or a setext heading, returning the next line index.
    fn paragraph(&mut self, lines: &[LineSpan], start: usize) -> usize {
        let mut i = start;
        let mut last_content = start;

        i += 1;
        while i < lines.len() {
            let line = lines[i];
            if is_blank(self.content, &line) {
                break;
            }
            let text = &self.content[line.start..line.end];

            if setext_level(text).is_some() && indent_width(self.content, &line) < 4 {
                self.setext_heading(lines, start, i);
                return i + 1;
            }
            if self.interrupts_paragraph(text, &line) {
                break;
            }
            last_content = i;
            i += 1;
        }

        let spans = &lines[start..=last_content];
        let segments = Segments::new(self.content, spans);
        self.inline(&segments, None);
        last_content + 1
    }

    fn setext_heading(&mut self, lines: &[LineSpan], title_start: usize, underline: usize) {
        let level = setext_level(&self.content[lines[underline].start..lines[underline].end])
            .unwrap_or(2);

        let block_start = lines[title_start].start;
        let block_end = lines[underline].end;

        self.out.headings.push(HeadingFact {
            block: (block_start, block_end),
            level,
            // Only the title lines carry inline content, not the underline.
            content: (block_start, lines[title_start].end),
        });

        let spans = &lines[title_start..underline];
        let segments = Segments::new(self.content, spans);
        self.inline(&segments, None);
    }

    /// Whether a line ends the paragraph being gathered.
    fn interrupts_paragraph(&self, text: &str, line: &LineSpan) -> bool {
        if indent_width(self.content, line) >= 4 {
            // Indented content is a lazy continuation of the paragraph.
            return false;
        }
        if is_thematic_break(text) {
            return true;
        }
        if atx_level(text.trim_start()).is_some() {
            return true;
        }
        if fence_marker(text).is_some() {
            return true;
        }
        if html_block_end(text).is_some_and(|end| end.may_interrupt_paragraph()) {
            return true;
        }
        if self.blockquote_start(line).is_some() {
            return true;
        }
        // Only a list item with content can interrupt a paragraph.
        list_marker(text).is_some()
    }

    // -----------------------------------------------------------------------
    // Inline
    // -----------------------------------------------------------------------

    /// Parses inline content, recording links and the regions that later
    /// wiki-link and tag scanning must skip.
    fn inline(&mut self, segments: &Segments, _unused: Option<()>) {
        let text = segments.text.as_str();
        let bytes = text.as_bytes();
        let mut delimiters: Vec<Delimiter> = Vec::new();
        let mut i = 0;

        while i < bytes.len() {
            match bytes[i] {
                b'\\' => {
                    i += 1;
                    if i < bytes.len() && is_ascii_punctuation(bytes[i]) {
                        i += 1;
                    }
                }
                b'`' => i = self.code_span(segments, i),
                b'<' => i = self.angle(segments, i),
                b'$' => i = self.math(text, segments, i),
                b'[' => {
                    // A `!` immediately before the bracket makes this an image.
                    let is_image = i > 0 && bytes[i - 1] == b'!';
                    let open = if is_image { i - 1 } else { i };
                    let label = label_after(text, i);
                    delimiters.push(Delimiter {
                        open,
                        bracket: i,
                        is_image,
                        inactive: false,
                        label,
                    });
                    i += 1;
                }
                b']' => {
                    i = self.close_bracket(segments, text, &mut delimiters, i);
                }
                _ => i += 1,
            }
        }
    }

    fn code_span(&mut self, segments: &Segments, start: usize) -> usize {
        let text = segments.text.as_str();
        let bytes = text.as_bytes();
        let mut run = 0;
        let mut i = start;
        while i < bytes.len() && bytes[i] == b'`' {
            run += 1;
            i += 1;
        }

        let mut probe = i;
        while probe < bytes.len() {
            if bytes[probe] == b'`' {
                let mut close_run = 0;
                let mut end = probe;
                while end < bytes.len() && bytes[end] == b'`' {
                    close_run += 1;
                    end += 1;
                }
                if close_run == run {
                    self.exclude(segments.source(start), segments.source(end));
                    return end;
                }
                probe = end;
            } else {
                probe += 1;
            }
        }

        // No matching run: the backticks are literal and stay scannable.
        i
    }

    /// Handles `<...>`: autolinks, email autolinks and inline HTML. None of
    /// them produce elements in the original, but all of them hide their
    /// contents from wiki-link and tag scanning.
    fn angle(&mut self, segments: &Segments, start: usize) -> usize {
        let text = segments.text.as_str();
        let rest = &text[start + 1..];

        let end = autolink_end(rest)
            .or_else(|| email_end(rest))
            // `end` is the offset of the closing `>` within `rest`.
            .map(|close| start + 1 + close + 1)
            .or_else(|| inline_html_end(text, start));

        let end = match end {
            Some(end) => end,
            None => return start + 1,
        };

        self.exclude(segments.source(start), segments.source(end));
        end
    }

    /// `$...$` and `$$...$$`, matching Markdig's mathematics extension closely
    /// enough for exclusion purposes.
    fn math(&mut self, text: &str, segments: &Segments, start: usize) -> usize {
        let bytes = text.as_bytes();
        let display = bytes.get(start + 1) == Some(&b'$');
        let opener_len = if display { 2 } else { 1 };
        let content_start = start + opener_len;

        // An opener may not be followed by whitespace.
        if matches!(bytes.get(content_start), None | Some(b' ') | Some(b'\t') | Some(b'\n')) {
            return start + 1;
        }

        let mut i = content_start;
        while i < bytes.len() {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == b'$' {
                let run = if display {
                    bytes.get(i + 1) == Some(&b'$')
                } else {
                    true
                };
                if run {
                    // A closer may not be preceded by whitespace.
                    if i > content_start && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
                        i += 1;
                        continue;
                    }
                    let end = i + opener_len;
                    self.exclude(segments.source(start), segments.source(end));
                    return end;
                }
            }
            i += 1;
        }

        start + 1
    }

    /// Handles `]`: forms a link with the innermost open delimiter.
    fn close_bracket(
        &mut self,
        segments: &Segments,
        text: &str,
        delimiters: &mut Vec<Delimiter>,
        at: usize,
    ) -> usize {
        let delimiter = match delimiters.pop() {
            Some(d) => d,
            None => return at + 1,
        };

        // Links may not nest: an opener deactivated by an enclosing link stays
        // literal text, and so does this bracket.
        if delimiter.inactive {
            return at + 1;
        }

        let bytes = text.as_bytes();
        let after = at + 1;

        // Inline link: `](dest "title")`. When the parentheses do not form a
        // valid destination the original falls through to the reference forms
        // instead of giving up, so `[a](b` still yields a shortcut link.
        if bytes.get(after) == Some(&b'(') {
            if let Some(parsed) = parse_inline_destination(text, after) {
                let end = parsed.close + 1;
                self.push_link(
                    segments, text, &delimiter, end, LinkKind::Inline, None, Some(parsed),
                );
                self.deactivate(delimiters, delimiter.is_image);
                return end;
            }
        }

        // A label has to hold something other than whitespace for a collapsed or
        // shortcut link; otherwise both brackets stay literal text.
        let has_label = match delimiter.label {
            Some((label_start, label_end)) => {
                let (trimmed_start, trimmed_end) = trimmed_label(text, label_start, label_end);
                trimmed_end > trimmed_start
            }
            None => false,
        };

        if bytes.get(after) == Some(&b'[') {
            if bytes.get(after + 1) == Some(&b']') {
                // Collapsed reference: `][]`.
                if !has_label {
                    return at + 1;
                }
                let end = after + 2;
                self.push_link(segments, text, &delimiter, end, LinkKind::Collapsed, None, None);
                self.deactivate(delimiters, delimiter.is_image);
                return end;
            }

            // Full reference: `][ref]`. The `[ref]` label supplies the
            // destination, so the link's own label may be empty.
            if let Some((label, close)) = parse_label(text, after) {
                if label.0 < label.1 {
                    let end = close + 1;
                    self.push_link(
                        segments,
                        text,
                        &delimiter,
                        end,
                        LinkKind::FullReference,
                        Some(label),
                        None,
                    );
                    self.deactivate(delimiters, delimiter.is_image);
                    return end;
                }
            }
            return at + 1;
        }

        // Shortcut reference: `]` with nothing usable after it.
        if !has_label {
            return at + 1;
        }
        self.push_link(segments, text, &delimiter, after, LinkKind::Shortcut, None, None);
        self.deactivate(delimiters, delimiter.is_image);
        after
    }

    /// Records a completed link and hides its destination from later scanning.
    #[allow(clippy::too_many_arguments)]
    fn push_link(
        &mut self,
        segments: &Segments,
        text: &str,
        delimiter: &Delimiter,
        end: usize,
        kind: LinkKind,
        ref_label: Option<(usize, usize)>,
        destination: Option<InlineDestination>,
    ) {
        let to_span = |s: SpannedText| SpannedText {
            text: (segments.source(s.text.0), segments.source(s.text.1)),
            range: (segments.source(s.range.0), segments.source(s.range.1)),
        };
        let plain = |(start, finish): (usize, usize)| SpannedText {
            text: (segments.source(start), segments.source(finish)),
            range: (segments.source(start), segments.source(finish)),
        };

        let raw_label = match delimiter.label {
            Some(span) => trimmed_label(text, span.0, span.1),
            None => (delimiter.bracket + 1, delimiter.bracket + 1),
        };

        let label = if raw_label.0 >= raw_label.1 {
            // Markdig leaves LabelSpan at its default for an empty label, which
            // renders as the document's origin rather than the brackets' place.
            SpannedText { text: (0, 0), range: (0, 0) }
        } else {
            plain(raw_label)
        };

        let url = match (&destination, kind) {
            (Some(d), _) => d.url.map(to_span),
            // A full reference link reports its `[ref]` label as the url, which
            // is how the original distinguishes it from a collapsed one.
            (None, LinkKind::FullReference) => ref_label.map(plain),
            (None, _) => None,
        };
        let title = destination.as_ref().and_then(|d| d.title).map(to_span);

        let full = (segments.source(delimiter.open), segments.source(end));
        let is_image = delimiter.is_image;
        self.out.links.push(LinkFact { full, kind, is_image, label, url, title });

        // A shortcut link's whole source is its label, which stays scannable for
        // wiki links and tags; the other kinds have a destination that must not
        // contribute elements.
        if kind != LinkKind::Shortcut {
            self.exclude(segments.source(delimiter.bracket), full.1);
        }
    }

    /// Links may not nest: opening delimiters before a completed non-image link
    /// become inactive.
    fn deactivate(&mut self, delimiters: &mut Vec<Delimiter>, is_image: bool) {
        if is_image {
            return;
        }
        for d in delimiters.iter_mut() {
            d.inactive = true;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Delimiter {
    /// Offset of the `!` for an image, otherwise of the `[`.
    open: usize,
    bracket: usize,
    is_image: bool,
    inactive: bool,
    /// The label's span as parsed when the bracket opened.
    label: Option<(usize, usize)>,
}

// ---------------------------------------------------------------------------
// Inline helpers
// ---------------------------------------------------------------------------

fn is_ascii_punctuation(b: u8) -> bool {
    matches!(b, b'!' | b'"' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+'
        | b',' | b'-' | b'.' | b'/' | b':' | b';' | b'<' | b'=' | b'>' | b'?' | b'@' | b'['
        | b'\\' | b']' | b'^' | b'_' | b'`' | b'{' | b'|' | b'}' | b'~')
}

fn skip_indent(content: &str, line: &LineSpan, width: usize) -> usize {
    let mut offset = line.start;
    let mut seen = 0;
    for b in content[line.start..line.end].bytes() {
        if seen >= width {
            break;
        }
        match b {
            b' ' => seen += 1,
            b'\t' => seen += 4 - (seen % 4),
            _ => break,
        }
        offset += 1;
    }
    offset
}

/// The span of the label following the `[` at `open`, or `None` if the brackets
/// do not close on this text.
fn label_after(text: &str, open: usize) -> Option<(usize, usize)> {
    parse_label(text, open).map(|(span, _)| span)
}

/// Parses `[...]` starting at `open`, returning the inner span and the offset of
/// the closing bracket. A label may span lines but may not contain a blank one.
fn parse_label(text: &str, open: usize) -> Option<((usize, usize), usize)> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'[') {
        return None;
    }
    let mut i = open + 1;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'\n' => {
                let mut probe = i + 1;
                while matches!(bytes.get(probe), Some(b' ') | Some(b'\t')) {
                    probe += 1;
                }
                if matches!(bytes.get(probe), None | Some(b'\n')) {
                    return None;
                }
                i += 1;
            }
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                if depth == 0 {
                    return Some(((open + 1, i), i));
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Byte length of the character at `at`.
fn len_utf8(text: &str, at: usize) -> usize {
    text[at..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
}

/// Trims whitespace from a label span, which is what Markdig reports.
fn trimmed_label(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut s = start;
    let mut e = end;
    while s < e && (text.as_bytes()[s] as char).is_whitespace() {
        s += len_utf8(text, s);
    }
    while e > s {
        let prev = prev_boundary(text, e);
        if (text.as_bytes()[prev] as char).is_whitespace() {
            e = prev;
        } else {
            break;
        }
    }
    (s, e)
}

fn prev_boundary(text: &str, at: usize) -> usize {
    let mut idx = at - 1;
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

struct InlineDestination {
    close: usize,
    url: Option<SpannedText>,
    title: Option<SpannedText>,
}

/// Parses `(dest "title")` starting at the `(`, returning the offset of the `)`.
fn parse_inline_destination(text: &str, open: usize) -> Option<InlineDestination> {
    let bytes = text.as_bytes();
    let mut i = open + 1;
    let mut depth = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                if depth == 0 {
                    let (url, title) = destination_parts(text, open + 1, i)?;
                    return Some(InlineDestination { close: i, url, title });
                }
                depth -= 1;
                i += 1;
            }
            b'\n' if depth == 0 => return None,
            _ => i += 1,
        }
    }
    None
}

/// Splits the inside of `(...)` into a destination and an optional title, with
/// the spans Markdig reports: a `<...>` destination keeps its brackets in the
/// span, and a title keeps its quotes.
fn destination_parts(
    text: &str,
    start: usize,
    end: usize,
) -> Option<(Option<SpannedText>, Option<SpannedText>)> {
    let bytes = text.as_bytes();
    let mut i = start;
    while i < end && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    if i >= end {
        // An empty destination is allowed: `[a]()`.
        return Some((None, None));
    }

    let url;
    if bytes[i] == b'<' {
        let inner_start = i + 1;
        let mut j = inner_start;
        while j < end && bytes[j] != b'>' {
            if bytes[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        if j >= end {
            return None;
        }
        url = Some(SpannedText { text: (inner_start, j), range: (i, j + 1) });
        i = j + 1;
    } else {
        let url_start = i;
        let mut nest = 0i32;
        while i < end {
            match bytes[i] {
                b'\\' => {
                    i += 2;
                    continue;
                }
                b'(' => nest += 1,
                b')' => {
                    if nest == 0 {
                        break;
                    }
                    nest -= 1;
                }
                b' ' | b'\t' | b'\n' => break,
                _ => {}
            }
            i += 1;
        }
        url = if i > url_start {
            Some(SpannedText { text: (url_start, i), range: (url_start, i) })
        } else {
            None
        };
    }

    while i < end && (bytes[i] as char).is_whitespace() {
        i += 1;
    }

    let (title, consumed) = if i < end && matches!(bytes[i], b'"' | b'\'' | b'(') {
        let opener = bytes[i];
        let closer = if opener == b'(' { b')' } else { opener };
        let delim_start = i;
        let inner_start = i + 1;
        let mut j = inner_start;
        while j < end && bytes[j] != closer {
            if bytes[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        if j < end {
            (Some(SpannedText { text: (inner_start, j), range: (delim_start, j + 1) }), j + 1)
        } else {
            (None, i)
        }
    } else {
        (None, i)
    };

    // Only whitespace may separate the title, or the destination, from the
    // closing paren: `[a](url title)` is not an inline link.
    let mut tail = consumed;
    while tail < end && (bytes[tail] as char).is_whitespace() {
        tail += 1;
    }
    if tail != end {
        return None;
    }

    Some((url, title))
}

/// The offset just past a `<scheme:...>` autolink, or `None`.
fn autolink_end(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let colon = bytes.iter().position(|&b| b == b':')?;
    if colon == 0 || colon > 32 {
        return None;
    }
    if !bytes[..colon].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'-' || *b == b'.') {
        return None;
    }
    let mut i = colon + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'>' => return Some(i),
            b' ' | b'\t' | b'\n' | b'<' => return None,
            _ => i += 1,
        }
    }
    None
}

/// The offset just past a `<someone@example.com>` autolink, or `None`.
fn email_end(rest: &str) -> Option<usize> {
    let close = rest.find('>')?;
    let candidate = &rest[..close];
    if candidate.is_empty() || candidate.contains(' ') || candidate.contains('\n') {
        return None;
    }
    let at = candidate.rfind('@')?;
    let local = &candidate[..at];
    let domain = &candidate[at + 1..];
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return None;
    }
    Some(close)
}

/// The offset just past an inline HTML tag or comment starting at `start`.
fn inline_html_end(text: &str, start: usize) -> Option<usize> {
    let rest = &text[start..];

    if starts_with_ci(rest, "<!--") {
        return rest.find("-->").map(|i| start + i + 3);
    }
    if starts_with_ci(rest, "<?") {
        return rest.find("?>").map(|i| start + i + 2);
    }
    if starts_with_ci(rest, "<![cdata[") {
        return rest.find("]]>").map(|i| start + i + 3);
    }
    if starts_with_ci(rest, "<!") {
        return rest.find('>').map(|i| start + i + 1);
    }

    // An open or close tag.
    let bytes = rest.as_bytes();
    let mut i = 1;
    if bytes.get(i) == Some(&b'/') {
        i += 1;
    }
    if !bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    loop {
        let ws = i;
        while matches!(bytes.get(i), Some(b' ') | Some(b'\t')) {
            i += 1;
        }
        if i == ws {
            break;
        }
        if !bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b':') {
            break;
        }
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b':' | b'.' | b'-'))
        {
            i += 1;
        }
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while matches!(bytes.get(i), Some(b' ') | Some(b'\t')) {
                i += 1;
            }
            match bytes.get(i) {
                Some(b'"') | Some(b'\'') => {
                    let quote = bytes[i];
                    i += 1;
                    while i < bytes.len() && bytes[i] != quote {
                        i += 1;
                    }
                    if i >= bytes.len() {
                        return None;
                    }
                    i += 1;
                }
                Some(b'>') | None => return None,
                _ => {
                    while i < bytes.len()
                        && !matches!(bytes[i], b' ' | b'\t' | b'>' | b'"' | b'\'' | b'=')
                    {
                        i += 1;
                    }
                }
            }
        }
    }
    while matches!(bytes.get(i), Some(b' ') | Some(b'\t')) {
        i += 1;
    }
    if bytes.get(i) == Some(&b'/') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'>') {
        Some(start + i + 1)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Link reference definitions
// ---------------------------------------------------------------------------

struct ParsedDefinition {
    end: usize,
    label: (usize, usize),
    url: SpannedText,
    title: Option<SpannedText>,
}

/// Parses `[label]: destination "title"` within a segmented view. Virtual
/// offsets are relative to `segments`.
fn parse_link_definition(segments: &Segments) -> Option<ParsedDefinition> {
    let text = segments.text.as_str();
    let bytes = text.as_bytes();

    if bytes.first() != Some(&b'[') {
        return None;
    }

    let (label, label_close) = parse_label(text, 0)?;
    if label.0 >= label.1 {
        return None;
    }
    if label_close + 1 >= bytes.len() || bytes[label_close + 1] != b':' {
        return None;
    }

    let mut i = label_close + 2;
    // Whitespace, including at most one line break, may precede the destination.
    i = skip_definition_whitespace(text, i)?;
    if i >= bytes.len() {
        return None;
    }

    let url = if bytes[i] == b'<' {
        let delim_start = i;
        let inner_start = i + 1;
        let mut j = inner_start;
        while j < bytes.len() && bytes[j] != b'>' && bytes[j] != b'\n' {
            if bytes[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'>' {
            return None;
        }
        i = j + 1;
        SpannedText { text: (inner_start, j), range: (delim_start, j + 1) }
    } else {
        let url_start = i;
        while i < bytes.len() && !matches!(bytes[i], b' ' | b'\t' | b'\n' | b'<' | b'>') {
            i += 1;
        }
        if i == url_start {
            return None;
        }
        SpannedText { text: (url_start, i), range: (url_start, i) }
    };

    let after_url = i;
    let title_start = skip_definition_whitespace(text, i);

    let title = match title_start {
        Some(t) if t < bytes.len() && matches!(bytes[t], b'"' | b'\'' | b'(') => {
            let opener = bytes[t];
            let closer = if opener == b'(' { b')' } else { opener };
            let inner_start = t + 1;
            let mut j = inner_start;
            while j < bytes.len() && bytes[j] != closer && bytes[j] != b'\n' {
                if bytes[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            if j < bytes.len() && bytes[j] == closer {
                i = j + 1;
                Some(SpannedText { text: (inner_start, j), range: (t, j + 1) })
            } else {
                i = after_url;
                None
            }
        }
        _ => {
            i = after_url;
            None
        }
    };

    Some(ParsedDefinition { end: i, label, url, title })
}

/// Skips spaces and tabs, plus at most one line break, returning `None` if the
/// whitespace runs to the end of the text.
fn skip_definition_whitespace(text: &str, mut i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut breaks = 0;
    loop {
        match bytes.get(i) {
            Some(b' ') | Some(b'\t') => i += 1,
            Some(b'\n') => {
                breaks += 1;
                if breaks > 1 {
                    return None;
                }
                i += 1;
            }
            Some(_) => return Some(i),
            None => return None,
        }
    }
}
