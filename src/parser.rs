//! Markdown parsing into the concrete syntax tree.
//!
//! Port of `Marksman.Parser`. The original builds on Markdig with two custom
//! inline parsers; here [`crate::markdown`] supplies block structure, links,
//! link reference definitions and the regions that scanning must skip, while
//! wiki-links and tags are scanned directly from the source. Scanning is
//! restricted to the regions the parser left open, so that code, HTML, math and
//! link destinations cannot contribute elements.

use std::collections::HashMap;

use lsp_types::{Position, Range};

use crate::config::ParserSettings;
use crate::markdown;
use crate::cst::{self, Cst, Element, Heading, MdLink, MdLinkDef, Node, Tag as CstTag, TextNode, WikiLink};
use crate::misc::Slug;
use crate::names::{UrlEncoded, WikiEncoded};
use crate::structure::Structure;
use crate::text::Text;

/// Byte ranges in which no inline element may be recognised.
struct Exclusions {
    ranges: Vec<(usize, usize)>,
}

impl Exclusions {
    fn new() -> Exclusions {
        Exclusions { ranges: Vec::new() }
    }

    fn push(&mut self, range: (usize, usize)) {
        if range.0 < range.1 {
            self.ranges.push(range);
        }
    }

    fn normalize(&mut self) {
        self.ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (s, e) in self.ranges.drain(..) {
            match merged.last_mut() {
                Some((_, last_end)) if s <= *last_end => {
                    if e > *last_end {
                        *last_end = e;
                    }
                }
                _ => merged.push((s, e)),
            }
        }
        self.ranges = merged;
    }

    /// Maximal contiguous spans outside the exclusions, split at line breaks.
    fn regions(&self, content: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut cursor = 0usize;

        for &(s, e) in &self.ranges {
            if s > cursor {
                split_at_newlines(content, cursor, s, &mut out);
            }
            cursor = cursor.max(e);
        }
        if cursor < content.len() {
            split_at_newlines(content, cursor, content.len(), &mut out);
        }
        out
    }

    fn contains(&self, start: usize, end: usize) -> bool {
        self.ranges.iter().any(|(s, e)| start < *e && *s < end)
    }
}

fn split_at_newlines(content: &str, start: usize, end: usize, out: &mut Vec<(usize, usize)>) {
    let mut region_start = start;
    for (idx, _) in content[start..end].match_indices('\n') {
        let abs = start + idx;
        if abs > region_start {
            out.push((region_start, abs));
        }
        region_start = abs + 1;
    }
    if end > region_start {
        out.push((region_start, end));
    }
}

fn range_of(text: &Text, start: usize, end: usize) -> Range {
    text.range_of_offsets(start, end)
}

/// One recognised `[[doc#heading|title]]` occurrence, as inclusive byte spans.
struct WikiMatch {
    full: (usize, usize),
    doc: Option<(usize, usize)>,
    heading: Option<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WikiState {
    Doc,
    Heading,
    Title,
    End,
}

fn byte_span(chars: &[(usize, char)], start_idx: usize, end_idx: usize) -> (usize, usize) {
    if end_idx <= start_idx || start_idx >= chars.len() {
        let byte = chars.get(start_idx).map(|c| c.0).unwrap_or_else(|| chars[0].0);
        (byte, byte)
    } else {
        let last = end_idx - 1;
        (chars[start_idx].0, chars[last].0 + chars[last].1.len_utf8())
    }
}

/// Runs the finite state machine from the original `WikiLinkParser`.
fn try_parse_wiki(chars: &[(usize, char)], i: usize) -> Option<(WikiMatch, usize)> {
    let j = i + 2;
    if j >= chars.len() {
        return None;
    }

    let mut doc: Option<(usize, usize)> = None;
    let mut heading: Option<(usize, usize)> = None;

    let mut state;
    let mut k;
    let mut doc_start = None;
    let mut head_start = None;

    match chars[j].1 {
        '#' => {
            head_start = Some(j + 1);
            k = j + 1;
            state = WikiState::Heading;
        }
        '|' => {
            k = j + 1;
            state = WikiState::Title;
        }
        ']' => {
            k = j + 1;
            state = WikiState::End;
        }
        '\n' | '\r' | '\0' => return None,
        _ => {
            doc_start = Some(j);
            k = j + 1;
            state = WikiState::Doc;
        }
    }

    while k < chars.len() {
        let ch = chars[k].1;
        let escaped = k > 0 && chars[k - 1].1 == '\\';

        match state {
            WikiState::Doc => match ch {
                '#' if !escaped => {
                    doc = Some(byte_span(chars, doc_start.unwrap_or(k), k));
                    head_start = Some(k + 1);
                    state = WikiState::Heading;
                    k += 1;
                }
                '|' if !escaped => {
                    doc = Some(byte_span(chars, doc_start.unwrap_or(k), k));
                    state = WikiState::Title;
                    k += 1;
                }
                ']' if !escaped => {
                    doc = Some(byte_span(chars, doc_start.unwrap_or(k), k));
                    state = WikiState::End;
                    k += 1;
                }
                '\n' | '\r' | '\0' => return None,
                _ => k += 1,
            },
            WikiState::Heading => match ch {
                '|' if !escaped => {
                    heading = Some(byte_span(chars, head_start.unwrap_or(k), k));
                    state = WikiState::Title;
                    k += 1;
                }
                ']' if !escaped => {
                    heading = Some(byte_span(chars, head_start.unwrap_or(k), k));
                    state = WikiState::End;
                    k += 1;
                }
                '\n' | '\r' | '\0' => return None,
                _ => k += 1,
            },
            WikiState::Title => match ch {
                ']' if !escaped => {
                    state = WikiState::End;
                    k += 1;
                }
                '\n' | '\r' | '\0' => return None,
                _ => k += 1,
            },
            WikiState::End => {
                if ch == ']' {
                    let full = (chars[i].0, chars[k].0 + ch.len_utf8());
                    return Some((WikiMatch { full, doc, heading }, k + 1));
                }
                return None;
            }
        }
    }

    None
}

fn scan_wiki_links(content: &str, region: (usize, usize), exclusions: &Exclusions) -> Vec<WikiMatch> {
    let chars: Vec<(usize, char)> = content[region.0..region.1]
        .char_indices()
        .map(|(i, c)| (region.0 + i, c))
        .collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i].1 == '[' && chars[i + 1].1 == '[' {
            if let Some((m, next)) = try_parse_wiki(&chars, i) {
                if !exclusions.contains(m.full.0, m.full.1) {
                    out.push(m);
                }
                i = next.max(i + 1);
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Markdig's `IsAlphaNumeric` is ASCII-only, so a tag name is ASCII too: `#タグ`
/// is not a tag.
fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/'
}

struct TagMatch {
    full: (usize, usize),
    name: (usize, usize),
}

/// Port of the original `TagsParser`: a `#` starts a tag only when it is not
/// glued to a preceding word and is followed by at least one tag character.
fn scan_tags(content: &str, region: (usize, usize)) -> Vec<TagMatch> {
    let mut out = Vec::new();
    let mut it = content[region.0..region.1].char_indices().peekable();

    while let Some((rel, c)) = it.next() {
        if c != '#' {
            continue;
        }
        let start = region.0 + rel;
        let prev = content[..start].chars().next_back();
        if prev.is_some_and(|p| p.is_ascii_alphanumeric()) {
            continue;
        }

        let mut end = start + 1;
        while let Some(&(next_rel, next_c)) = it.peek() {
            if !is_tag_char(next_c) {
                break;
            }
            end = region.0 + next_rel + next_c.len_utf8();
            it.next();
        }

        if end > start + 1 {
            out.push(TagMatch { full: (start, end), name: (start + 1, end) });
        }
    }

    out
}

pub fn scrape_text(parser_settings: &ParserSettings, text: &Text) -> Vec<Element> {
    let content = &text.content;
    let scanned = markdown::scan(content);

    let mut exclusions = Exclusions::new();
    for span in &scanned.exclusions {
        exclusions.push(*span);
    }

    let mut elements: Vec<Element> = Vec::new();

    for heading in &scanned.headings {
        let (start, end) = heading.block;
        let range = range_of(text, start, end);
        let node = build_heading(parser_settings, text, content, start, end, heading.level, range);
        elements.push(Element::H(node));
    }

    let def_elements: Vec<Element> =
        scanned.link_defs.iter().map(|def| build_link_def(text, content, def)).collect();

    exclusions.normalize();

    // Wiki links are scanned before Markdown links are materialised: `[[x]]`
    // also reads as a shortcut link over `x`, and the wiki reading wins,
    // matching the original's inline parser priority.
    let regions = exclusions.regions(content);
    let mut wiki_ranges = Vec::new();
    let mut wiki_elements = Vec::new();
    let mut tag_elements = Vec::new();
    for region in &regions {
        for m in scan_wiki_links(content, *region, &exclusions) {
            wiki_ranges.push(m.full);
            wiki_elements.push(build_wiki_link(text, content, m));
        }
        for t in scan_tags(content, *region) {
            // A wiki link is consumed whole, so a `#` inside one is a heading
            // anchor rather than a tag.
            if wiki_ranges.iter().any(|w| w.0 <= t.full.0 && t.full.1 <= w.1) {
                continue;
            }
            tag_elements.push(build_tag(text, content, t));
        }
    }

    for link in &scanned.links {
        if wiki_ranges.iter().any(|w| link.full.0 < w.1 && w.0 < link.full.1) {
            continue;
        }
        elements.push(build_md_link(text, content, link));
    }

    elements.extend(def_elements);
    elements.extend(wiki_elements);
    elements.extend(tag_elements);

    if let Some((start, end)) = scanned.front_matter {
        elements.push(Element::YML(Node::mk_text(
            content[start..end].to_string(),
            range_of(text, start, end),
        )));
    }

    elements
}


fn build_heading(
    parser_settings: &ParserSettings,
    text: &Text,
    content: &str,
    start: usize,
    end: usize,
    level: i32,
    heading_range: Range,
) -> Node<Heading> {
    let full_text = &content[start..end];
    let title0 = full_text.trim_start_matches(|c| c == ' ' || c == '#');
    let prefix_len = full_text.len() - title0.len();
    // A setext heading's block span includes its underline, and the title keeps
    // it: only trailing spaces are trimmed, exactly as the original does.
    let title = title0.trim_end_matches(' ');
    let suffix_len = title0.len() - title.len();

    let title_start = start + prefix_len;
    let title_end = end - suffix_len;
    let title_range = range_of(text, title_start, title_end);

    Node::mk(
        full_text.to_string(),
        heading_range,
        Heading {
            level,
            is_title: parser_settings.title_from_heading && level <= 1,
            title: TextNode::mk_text(title.to_string(), title_range),
            disambiguation: None,
            scope: heading_range,
        },
    )
}

/// GitLab-flavoured heading ids disambiguate repeated headings with a `-N` suffix.
fn apply_disambiguation(parser_settings: &ParserSettings, elements: &mut [Element]) {
    if !parser_settings.glfm_heading_ids {
        return;
    }
    let mut last_heading_no: HashMap<Slug, i32> = HashMap::new();

    for el in elements.iter_mut() {
        if let Element::H(node) = el {
            let slug = node.data.slug();
            let num = match last_heading_no.get(&slug) {
                Some(v) => v + 1,
                None => 0,
            };
            last_heading_no.insert(slug, num);
            if num > 0 {
                node.data.disambiguation = Some(num.to_string());
            }
        }
    }
}

fn build_wiki_link(text: &Text, content: &str, m: WikiMatch) -> Element {
    let mk_node = |span: Option<(usize, usize)>| -> Option<cst::WikiEncodedNode> {
        span.map(|(s, e)| {
            Node::mk(
                content[s..e].to_string(),
                range_of(text, s, e),
                WikiEncoded::mk_unchecked(content[s..e].to_string()),
            )
        })
    };

    let wiki = WikiLink { doc: mk_node(m.doc), heading: mk_node(m.heading) };
    let range = range_of(text, m.full.0, m.full.1);
    Element::WL(Node::mk(content[m.full.0..m.full.1].to_string(), range, wiki))
}

fn build_tag(text: &Text, content: &str, t: TagMatch) -> Element {
    let name_node = TextNode::mk_text(
        content[t.name.0..t.name.1].to_string(),
        range_of(text, t.name.0, t.name.1),
    );
    let range = range_of(text, t.full.0, t.full.1);
    Element::T(Node::mk(
        content[t.full.0..t.full.1].to_string(),
        range,
        CstTag { name: name_node },
    ))
}

fn build_md_link(text: &Text, content: &str, link: &markdown::LinkFact) -> Element {
    let node_text = |s: markdown::SpannedText| {
        TextNode::mk_text(
            content[s.text.0..s.text.1].to_string(),
            range_of(text, s.range.0, s.range.1),
        )
    };
    let node_url = |s: markdown::SpannedText| {
        let url = content[s.text.0..s.text.1].to_string();
        Node::mk(url.clone(), range_of(text, s.range.0, s.range.1), UrlEncoded::mk_unchecked(url))
    };

    let label = node_text(link.label);

    let data = match link.kind {
        markdown::LinkKind::Inline => {
            MdLink::IL(label, link.url.map(node_url), link.title.map(node_text))
        }
        // The original reports a full reference link's url as the text of its
        // `[ref]` label, which is how it tells a full reference from a collapsed
        // one.
        markdown::LinkKind::FullReference => MdLink::RF(label, node_text(link.url.unwrap())),
        markdown::LinkKind::Collapsed => MdLink::RC(label),
        markdown::LinkKind::Shortcut => MdLink::RS(label),
    };

    Element::ML(Node::mk(
        content[link.full.0..link.full.1].to_string(),
        range_of(text, link.full.0, link.full.1),
        data,
    ))
}


fn build_link_def(text: &Text, content: &str, def: &markdown::LinkDefFact) -> Element {
    let url_text = content[def.url.text.0..def.url.text.1].to_string();

    let data = MdLinkDef::mk(
        TextNode::mk_text(
            content[def.label.0..def.label.1].to_string(),
            range_of(text, def.label.0, def.label.1),
        ),
        Node::mk(
            url_text.clone(),
            range_of(text, def.url.range.0, def.url.range.1),
            UrlEncoded::mk_unchecked(url_text),
        ),
        def.title.map(|t| {
            TextNode::mk_text(
                content[t.text.0..t.text.1].to_string(),
                range_of(text, t.range.0, t.range.1),
            )
        }),
    );

    Element::MLD(Node::mk(
        content[def.full.0..def.full.1].to_string(),
        range_of(text, def.full.0, def.full.1),
        data,
    ))
}


fn sort_elements(text: &Text, elements: &mut Vec<Element>) {
    let offsets = |el: &Element| {
        let range = el.range();
        (
            text.position_to_offset(range.start),
            text.position_to_offset(range.end),
        )
    };
    // A stable sort keeps source order for elements that start at the same offset.
    let mut keyed: Vec<((usize, usize), usize)> =
        elements.iter().enumerate().map(|(i, el)| (offsets(el), i)).collect();
    keyed.sort_by_key(|(off, i)| (*off, *i));

    // Permuting by index moves each element once instead of cloning it.
    let mut old: Vec<Option<Element>> =
        std::mem::take(elements).into_iter().map(Some).collect();
    elements.reserve(keyed.len());
    for (_, i) in keyed {
        elements.push(old[i].take().expect("a permutation visits each index once"));
    }
}

pub fn build_cst(text: &Text, mut input_elements: Vec<Element>) -> Cst {
    sort_elements(text, &mut input_elements);

    let mut scope_map: HashMap<usize, Position> = HashMap::new();
    let mut child_map: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut output_elements: Vec<Element> = Vec::with_capacity(input_elements.len());
    // Most recent heading first.
    let mut head_stack: Vec<(usize, i32)> = Vec::new();

    for (idx, el) in input_elements.into_iter().enumerate() {
        if let Element::H(cur_head) = &el {
            let cur_level = cur_head.data.level;
            // Close every heading on the stack that is nested deeper than, or
            // equal to, this one: its scope ends where this heading starts.
            let mut close_upto = 0;
            for (stack_idx, level) in head_stack.iter() {
                if *level >= cur_level {
                    scope_map.insert(*stack_idx, cur_head.data.scope.start);
                    close_upto += 1;
                } else {
                    break;
                }
            }
            head_stack.drain(..close_upto);
        }

        // The parent is whatever still encloses this element, so it is read
        // before a heading pushes itself onto the stack.
        if let Some(&(parent_idx, _)) = head_stack.first() {
            child_map.entry(parent_idx).or_default().push(idx);
        }

        if let Element::H(cur_head) = &el {
            head_stack.insert(0, (idx, cur_head.data.level));
        }

        output_elements.push(el);
    }

    // Headings left open run to the end of the document.
    for (idx, _) in head_stack.iter() {
        scope_map.insert(*idx, text.end_range().start);
    }

    for (header_idx, scope_end) in scope_map {
        if let Element::H(header) = &mut output_elements[header_idx] {
            header.data.scope = Range { start: header.data.scope.start, end: scope_end };
        } else {
            panic!("Unexpected non-heading element at idx {header_idx}");
        }
    }

    // Sort into document order, remembering where each element landed so the
    // child map can hold indices rather than copies.
    let offsets = |el: &Element| {
        let range = el.range();
        (text.position_to_offset(range.start), text.position_to_offset(range.end))
    };

    let mut indexed: Vec<(usize, Element)> = output_elements.into_iter().enumerate().collect();
    // Stable, so elements sharing an offset keep their source order.
    indexed.sort_by(|(_, a), (_, b)| offsets(a).cmp(&offsets(b)));

    let mut new_index = vec![0usize; indexed.len()];
    for (position, (old_index, _)) in indexed.iter().enumerate() {
        new_index[*old_index] = position;
    }
    let elements: Vec<Element> = indexed.into_iter().map(|(_, el)| el).collect();

    let child_map = child_map
        .into_iter()
        .map(|(parent_idx, child_indices)| {
            let mut children: Vec<usize> = child_indices.iter().map(|i| new_index[*i]).collect();
            children.sort_by(|a, b| offsets(&elements[*a]).cmp(&offsets(&elements[*b])));
            (new_index[parent_idx], children)
        })
        .collect();

    Cst { elements, child_map }
}

pub fn parse(parser_settings: &ParserSettings, text: &Text) -> Structure {
    if text.content.is_empty() {
        return Structure::of_cst(
            parser_settings,
            Cst { elements: Vec::new(), child_map: Default::default() },
        );
    }

    let mut flat = scrape_text(parser_settings, text);
    apply_disambiguation(parser_settings, &mut flat);
    let cst = build_cst(text, flat);
    Structure::of_cst(parser_settings, cst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::mk_text;

    fn scrape(content: &str) -> Vec<Element> {
        let text = mk_text(content);
        let mut flat = scrape_text(&ParserSettings::default(), &text);
        apply_disambiguation(&ParserSettings::default(), &mut flat);
        let cst = build_cst(&text, flat);
        cst.elements
    }

    fn fmt_all(elements: &[Element]) -> Vec<String> {
        elements.iter().map(Element::fmt).collect()
    }

    #[test]
    fn parse_empty() {
        assert!(scrape("").is_empty());
    }

    #[test]
    fn parse_wiki_link() {
        assert_eq!(
            fmt_all(&scrape("[[note]]")),
            vec!["WL: [[note]]; (0,0)-(0,8)\n  doc=note; (0,2)-(0,6)"]
        );
    }

    #[test]
    fn parse_wiki_link_with_heading() {
        assert_eq!(
            fmt_all(&scrape("[[note#heading]]")),
            vec!["WL: [[note#heading]]; (0,0)-(0,16)\n  doc=note; (0,2)-(0,6)\n  head=heading; (0,7)-(0,14)"]
        );
    }

    #[test]
    fn parse_wiki_link_with_surrounding_text() {
        assert_eq!(
            fmt_all(&scrape("Before [[N]]")),
            vec!["WL: [[N]]; (0,7)-(0,12)\n  doc=N; (0,9)-(0,10)"]
        );
        assert_eq!(
            fmt_all(&scrape("[[note]]! Other")),
            vec!["WL: [[note]]; (0,0)-(0,8)\n  doc=note; (0,2)-(0,6)"]
        );
    }

    #[test]
    fn parse_wiki_link_empty_heading() {
        let els = scrape("[[T#]]");
        assert_eq!(fmt_all(&els), vec!["WL: [[T#]]; (0,0)-(0,6)\n  doc=T; (0,2)-(0,3)\n  head=; (0,4)-(0,4)"]);
    }

    #[test]
    fn parse_wiki_link_with_title() {
        assert_eq!(
            fmt_all(&scrape("[[note|My Title]]")),
            vec!["WL: [[note|My Title]]; (0,0)-(0,17)\n  doc=note; (0,2)-(0,6)"]
        );
    }

    #[test]
    fn parse_intra_wiki_link() {
        assert_eq!(
            fmt_all(&scrape("[[#heading]]")),
            vec!["WL: [[#heading]]; (0,0)-(0,12)\n  head=heading; (0,3)-(0,10)"]
        );
    }

    #[test]
    fn parse_unterminated_wiki_link_is_text() {
        assert!(scrape("[[cp1").iter().all(|e| !matches!(e, Element::WL(_))));
    }

    #[test]
    fn parse_heading() {
        // An unclosed heading's scope runs to the phantom end-of-document line.
        assert_eq!(
            fmt_all(&scrape("# Title text")),
            vec!["H1: range=(0,0)-(0,12); scope=(0,0)-(1,0)\n  text=`# Title text`\n  title=`Title text` @ (0,2)-(0,12)"]
        );
    }

    #[test]
    fn parse_nested_headings_set_scopes() {
        let els = scrape("# H1 \n## H2.1\n## H2.2\n");
        let fmts = fmt_all(&els);
        assert_eq!(fmts.len(), 3);
        // A heading governs its nested headings, so H1 runs to the end while the
        // two H2s end where their sibling or the document ends.
        assert!(fmts[0].contains("scope=(0,0)-(3,0)"), "{fmts:?}");
        assert!(fmts[1].contains("scope=(1,0)-(2,0)"), "{fmts:?}");
        assert!(fmts[2].contains("scope=(2,0)-(3,0)"), "{fmts:?}");
    }

    #[test]
    fn setext_heading_keeps_its_underline() {
        // Markdig's span for a setext heading covers the underline, so the
        // element's text and its title both include it; only trailing spaces
        // are trimmed from the title.
        assert_eq!(
            fmt_all(&scrape("Foo\n-\n")),
            vec![
                "H2: range=(0,0)-(1,1); scope=(0,0)-(2,0)\n  text=`Foo\n-`\n  title=`Foo\n-` @ (0,0)-(1,1)"
            ]
        );
    }

    #[test]
    fn duplicate_headings_are_disambiguated() {
        let els = scrape("# A\n# A\n# A\n");
        let slugs: Vec<String> = els
            .iter()
            .filter_map(|e| e.as_heading().map(|h| h.data.slug().to_string()))
            .collect();
        assert_eq!(slugs, vec!["a", "a-1", "a-2"]);
    }

    #[test]
    fn emoji_heading_has_empty_slug() {
        let els = scrape("## 45\n## 🚀");
        assert_eq!(fmt_all(&els)[0], "H2: range=(0,0)-(0,5); scope=(0,0)-(1,0)\n  text=`## 45`\n  title=`45` @ (0,3)-(0,5)");
        let slugs: Vec<String> = els.iter().filter_map(|e| e.as_heading().map(|h| h.data.slug().to_string())).collect();
        assert_eq!(slugs, vec!["45", ""]);
    }

    #[test]
    fn parse_inline_link() {
        let els = scrape("[text](dest.md#anch \"ti\")");
        assert_eq!(
            fmt_all(&els),
            vec!["ML: [text](dest.md#anch \"ti\") @ (0,0)-(0,25)\n  IL: label=text @ (0,1)-(0,5); url=dest.md#anch @ (0,7)-(0,19); title=ti @ (0,20)-(0,24)"]
        );
    }

    #[test]
    fn parse_reference_links() {
        let els = scrape("[full][lab]\n[coll][]\n[short]\n\n[lab]: /u\n");
        let fmts = fmt_all(&els);
        assert!(fmts[0].starts_with("ML: [full][lab]"), "{fmts:?}");
        assert!(fmts[0].contains("RF: text=full @ (0,1)-(0,5); label=lab @ (0,7)-(0,10)"), "{fmts:?}");
        assert!(fmts[1].contains("RC: label=coll @ (1,1)-(1,5)"), "{fmts:?}");
        assert!(fmts[2].contains("RS: label=short @ (2,1)-(2,6)"), "{fmts:?}");
        assert!(fmts[3].starts_with("MLD:"), "{fmts:?}");
    }

    #[test]
    fn parse_link_definition() {
        let els = scrape("[My Label]: /some/url \"A Title\"\n");
        assert_eq!(
            fmt_all(&els),
            vec!["MLD: [My Label]: /some/url \"A Title\" @ (0,0)-(0,31)\n  label=My Label @ (0,1)-(0,9); url=/some/url @ (0,12)-(0,21); title=A Title @ (0,22)-(0,31)"]
        );
    }

    #[test]
    fn link_definition_inside_code_block_is_ignored() {
        let els = scrape("```\n[a]: /u\n```\n");
        assert!(els.iter().all(|e| !matches!(e, Element::MLD(_))));
    }

    #[test]
    fn duplicate_labels_are_both_reported() {
        // Only the first binds as a reference target, but the original reports
        // every definition as an element.
        let els = scrape("[a]: /u1\n[a]: /u2\n");
        let defs: Vec<&Element> = els.iter().filter(|e| matches!(e, Element::MLD(_))).collect();
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn parse_tags() {
        let els = scrape("a #tag and #nested/sub-tag end");
        let tags: Vec<String> = els
            .iter()
            .filter_map(|e| match e {
                Element::T(n) => Some(n.data.name.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tags, vec!["tag", "nested/sub-tag"]);
    }

    #[test]
    fn tag_glued_to_a_word_is_not_a_tag() {
        let els = scrape("C# and a#b");
        assert!(els.iter().all(|e| !matches!(e, Element::T(_))));
    }

    #[test]
    fn no_elements_inside_code() {
        let els = scrape("`[[a]] #t`\n\n```\n[[b]] #u\n```\n\n    [[c]] #v\n");
        assert!(els.iter().all(|e| !matches!(e, Element::WL(_) | Element::T(_))));
    }

    #[test]
    fn no_tag_inside_link_destination() {
        let els = scrape("[t](note.md#sec)");
        assert!(els.iter().all(|e| !matches!(e, Element::T(_))));
    }

    #[test]
    fn yaml_front_matter_is_captured() {
        let els = scrape("---\ntitle: X\n---\n\n# X\n");
        assert!(matches!(els.first(), Some(Element::YML(_))));
        assert!(els.iter().any(|e| matches!(e, Element::H(_))));
    }

    #[test]
    fn dashes_mid_document_are_not_front_matter() {
        let els = scrape("# T\n\n---\n");
        assert!(els.iter().all(|e| !matches!(e, Element::YML(_))));
    }

    #[test]
    fn image_links_are_elements() {
        let els = scrape("![alt](pic.md)");
        assert!(els.iter().any(|e| matches!(e, Element::ML(n) if matches!(n.data, MdLink::IL(..)))));
    }

    #[test]
    fn elements_are_ordered_by_position() {
        let els = scrape("# H\n[[a]] text #t\n[ref]: /u\n");
        let text = mk_text("# H\n[[a]] text #t\n[ref]: /u\n");
        let offsets: Vec<usize> = els
            .iter()
            .map(|e| text.position_to_offset(e.range().start))
            .collect();
        let mut sorted = offsets.clone();
        sorted.sort();
        assert_eq!(offsets, sorted);
    }
}
