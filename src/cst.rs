//! The concrete syntax tree: elements with their source text and ranges.
//!
//! Port of `Marksman.Cst`.

use std::collections::BTreeMap;

use lsp_types::{Position, Range};

use crate::ast;
use crate::misc::{self, LinkLabel, Slug};
use crate::names::{InternPath, UrlEncoded, WikiEncoded};
use crate::paths::RelPath;

pub fn fmt_pos(pos: Position) -> String {
    format!("({},{})", pos.line, pos.character)
}

pub fn fmt_range(range: Range) -> String {
    format!("{}-{}", fmt_pos(range.start), fmt_pos(range.end))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Node<A> {
    pub text: String,
    pub range: Range,
    pub data: A,
}

impl<A: Ord> PartialOrd for Node<A> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<A: Ord> Ord for Node<A> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        misc::cmp_range(&self.range, &other.range)
            .then_with(|| self.text.cmp(&other.text))
            .then_with(|| self.data.cmp(&other.data))
    }
}

pub type TextNode = Node<()>;
pub type UrlEncodedNode = Node<UrlEncoded>;
pub type WikiEncodedNode = Node<WikiEncoded>;

impl<A> Node<A> {
    pub fn mk(text: impl Into<String>, range: Range, data: A) -> Node<A> {
        Node { text: text.into(), range, data }
    }

    pub fn as_text(&self) -> TextNode
    where
        A: Clone,
    {
        Node { text: self.text.clone(), range: self.range, data: () }
    }
}

impl Node<()> {
    pub fn mk_text(text: impl Into<String>, range: Range) -> TextNode {
        Node { text: text.into(), range, data: () }
    }
}

pub fn node_text<A>(node: &Node<A>) -> &str {
    &node.text
}

pub fn node_text_opt<A>(node: &Option<Node<A>>, default: &str) -> String {
    node.as_ref().map(|n| n.text.clone()).unwrap_or_else(|| default.to_string())
}

pub fn fmt_text(node: &TextNode) -> String {
    format!("{} @ {}", node.text, fmt_range(node.range))
}

pub fn fmt_url(node: &UrlEncodedNode) -> String {
    format!("{} @ {}", node.text, fmt_range(node.range))
}

pub fn fmt_wiki(node: &WikiEncodedNode) -> String {
    format!("{} @ {}", node.text, fmt_range(node.range))
}

pub fn fmt_opt_text(node: &Option<TextNode>) -> String {
    match node {
        Some(n) => fmt_text(n),
        None => "∅".to_string(),
    }
}

pub fn fmt_opt_url(node: &Option<UrlEncodedNode>) -> String {
    match node {
        Some(n) => fmt_url(n),
        None => "∅".to_string(),
    }
}

pub fn fmt_opt_wiki(node: &Option<WikiEncodedNode>) -> String {
    match node {
        Some(n) => fmt_wiki(n),
        None => "∅".to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WikiLink {
    pub doc: Option<WikiEncodedNode>,
    pub heading: Option<WikiEncodedNode>,
}

/// A resolved wiki-link destination: either a bare title or a path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WikiDest {
    WTitle(String),
    WPath(InternPath),
}

impl WikiDest {
    pub fn encode(&self) -> WikiEncoded {
        match self {
            WikiDest::WTitle(t) => WikiEncoded::encode(t),
            WikiDest::WPath(p) => {
                let rel_path = p.to_rel().to_system().to_string();
                WikiEncoded::mk_unchecked(misc::encode_path_for_wiki(&rel_path))
            }
        }
    }
}

impl WikiLink {
    pub fn dest_doc(&self) -> Option<WikiEncoded> {
        self.doc.as_ref().map(|n| n.data.clone())
    }

    pub fn dest_heading(&self) -> Option<WikiEncoded> {
        self.heading.as_ref().map(|n| n.data.clone())
    }

    pub fn fmt(&self) -> String {
        let mut lines = Vec::new();
        if let Some(d) = &self.doc {
            lines.push(format!("doc={}; {}", d.text, fmt_range(d.range)));
        }
        if let Some(h) = &self.heading {
            lines.push(format!("head={}; {}", h.text, fmt_range(h.range)));
        }
        lines.join("\n")
    }

    pub fn render(doc: Option<&WikiEncoded>, heading: Option<&WikiEncoded>, include_braces: bool) -> String {
        let doc_text = doc.map(WikiEncoded::raw).unwrap_or("");
        let heading_text = heading.map(|h| format!("#{}", h.raw())).unwrap_or_default();
        let link_text = format!("{doc_text}{heading_text}");
        if include_braces {
            format!("[[{link_text}]]")
        } else {
            link_text
        }
    }

    pub fn content_range(&self) -> Option<Range> {
        match (&self.doc, &self.heading) {
            (None, None) => None,
            (Some(d), None) => Some(d.range),
            (None, Some(h)) => Some(h.range),
            (Some(d), Some(h)) => Some(Range { start: d.range.start, end: h.range.end }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MdLink {
    /// `[text](url "title")`
    IL(TextNode, Option<UrlEncodedNode>, Option<TextNode>),
    /// `[text][label]`
    RF(TextNode, TextNode),
    /// `[label][]`
    RC(TextNode),
    /// `[label]`
    RS(TextNode),
}

impl MdLink {
    pub fn fmt(&self) -> String {
        match self {
            MdLink::IL(label, url, title) => format!(
                "IL: label={}; url={}; title={}",
                fmt_text(label),
                fmt_opt_url(url),
                fmt_opt_text(title)
            ),
            MdLink::RF(text, label) => {
                format!("RF: text={}; label={}", fmt_text(text), fmt_text(label))
            }
            MdLink::RC(label) => format!("RC: label={}", fmt_text(label)),
            MdLink::RS(label) => format!("RS: label={}", fmt_text(label)),
        }
    }

    pub fn reference_label(&self) -> Option<&TextNode> {
        match self {
            MdLink::RF(_, label) | MdLink::RC(label) | MdLink::RS(label) => Some(label),
            MdLink::IL(..) => None,
        }
    }

    pub fn render_inline(text: Option<&str>, path: Option<&str>, anchor: Option<&str>) -> String {
        let text = text.unwrap_or("");
        let path = path.unwrap_or("");
        let anchor = anchor.map(|x| format!("#{x}")).unwrap_or_default();
        format!("[{text}]({path}{anchor})")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MdLinkDef {
    pub label: TextNode,
    pub url: UrlEncodedNode,
    pub title: Option<TextNode>,
}

impl MdLinkDef {
    pub fn mk(label: TextNode, url: UrlEncodedNode, title: Option<TextNode>) -> MdLinkDef {
        MdLinkDef { label, url, title }
    }

    pub fn normalized_label(&self) -> LinkLabel {
        LinkLabel::of_string(&self.label.text)
    }

    pub fn title_content(&self) -> Option<&str> {
        self.title.as_ref().map(|t| t.text.as_str())
    }

    pub fn url_content(&self) -> &str {
        &self.url.text
    }

    pub fn fmt(&self) -> String {
        format!(
            "label={}; url={}; title={}",
            fmt_text(&self.label),
            fmt_url(&self.url),
            fmt_opt_text(&self.title)
        )
    }

    pub fn name(&self) -> &str {
        &self.label.text
    }

    pub fn to_abstract(&self) -> ast::MdLinkDef {
        ast::MdLinkDef {
            label: self.label.text.clone(),
            url: self.url.data.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag {
    pub name: TextNode,
}

impl Tag {
    pub fn fmt(&self) -> String {
        format!("name={}; range={}", self.name.text, fmt_range(self.name.range))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Heading {
    pub level: i32,
    pub is_title: bool,
    pub title: TextNode,
    /// Duplicate-heading suffix applied when GLFM heading ids are enabled.
    pub disambiguation: Option<String>,
    /// The source range this heading's content extends over.
    pub scope: Range,
}

impl PartialOrd for Heading {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Heading {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.level
            .cmp(&other.level)
            .then_with(|| self.is_title.cmp(&other.is_title))
            .then_with(|| self.title.cmp(&other.title))
            .then_with(|| self.disambiguation.cmp(&other.disambiguation))
            .then_with(|| misc::cmp_range(&self.scope, &other.scope))
    }
}

impl Heading {
    pub fn name(&self) -> &str {
        &self.title.text
    }

    pub fn slug(&self) -> Slug {
        match &self.disambiguation {
            None => Slug::of_string(self.name()),
            Some(d) => Slug::of_string(&format!("{}-{d}", self.name())),
        }
    }

    pub fn is_title(&self) -> bool {
        self.is_title
    }

    pub fn range(&self) -> Range {
        self.title.range
    }

    pub fn scope(&self) -> Range {
        self.scope
    }

    pub fn to_abstract(&self) -> ast::Heading {
        ast::Heading {
            level: self.level,
            is_title: self.is_title,
            text: self.title.text.clone(),
            id: self.slug(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Element {
    H(Node<Heading>),
    WL(Node<WikiLink>),
    ML(Node<MdLink>),
    MLD(Node<MdLinkDef>),
    T(Node<Tag>),
    YML(TextNode),
}

impl Element {
    pub fn range(&self) -> Range {
        match self {
            Element::H(n) => n.range,
            Element::WL(n) => n.range,
            Element::ML(n) => n.range,
            Element::MLD(n) => n.range,
            Element::T(n) => n.range,
            Element::YML(n) => n.range,
        }
    }

    pub fn range_start(&self) -> Position {
        self.range().start
    }

    pub fn text(&self) -> &str {
        match self {
            Element::H(n) => &n.text,
            Element::WL(n) => &n.text,
            Element::ML(n) => &n.text,
            Element::MLD(n) => &n.text,
            Element::T(n) => &n.text,
            Element::YML(n) => &n.text,
        }
    }

    pub fn as_heading(&self) -> Option<&Node<Heading>> {
        match self {
            Element::H(h) => Some(h),
            _ => None,
        }
    }

    pub fn as_wiki_link(&self) -> Option<&Node<WikiLink>> {
        match self {
            Element::WL(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_link_def(&self) -> Option<&Node<MdLinkDef>> {
        match self {
            Element::MLD(d) => Some(d),
            _ => None,
        }
    }

    pub fn is_decl(&self) -> bool {
        matches!(self, Element::YML(_) | Element::H(_) | Element::MLD(_))
    }

    pub fn is_link(&self) -> bool {
        matches!(self, Element::WL(_) | Element::ML(_))
    }

    pub fn is_title(&self) -> bool {
        self.as_heading().map(|n| n.data.is_title()).unwrap_or(false)
    }

    pub fn to_abstract(&self) -> Option<ast::Element> {
        match self {
            Element::H(h) => Some(ast::Element::H(h.data.to_abstract())),
            Element::WL(w) => Some(ast::Element::WL(ast::WikiLink {
                doc: w.data.doc.as_ref().map(|n| n.data.decode()),
                heading: w.data.heading.as_ref().map(|n| n.data.decode()),
            })),
            Element::ML(l) => match &l.data {
                MdLink::IL(text, url, _) => {
                    let (url_part, anchor) = match url {
                        Some(u) => split_url(&u.text),
                        None => (None, None),
                    };
                    Some(ast::Element::ML(ast::MdLink {
                        text: text.text.clone(),
                        url: url_part.map(|u| misc::url_decode(&u)),
                        anchor: anchor.map(|a| misc::url_decode(&a)),
                    }))
                }
                MdLink::RF(text, label) => {
                    Some(ast::Element::MR(ast::MdRef::Full(text.text.clone(), label.text.clone())))
                }
                MdLink::RC(label) => Some(ast::Element::MR(ast::MdRef::Collapsed(label.text.clone()))),
                MdLink::RS(label) => Some(ast::Element::MR(ast::MdRef::Shortcut(label.text.clone()))),
            },
            Element::MLD(d) => Some(ast::Element::MLD(d.data.to_abstract())),
            Element::T(t) => Some(ast::Element::T(t.data.name.text.clone())),
            Element::YML(_) => None,
        }
    }

    pub fn fmt(&self) -> String {
        match self {
            Element::H(h) => fmt_heading(h),
            Element::WL(x) => fmt_wiki_link(x),
            Element::ML(l) => fmt_md_link(l),
            Element::MLD(r) => fmt_md_link_def(r),
            Element::T(t) => fmt_tag(t),
            Element::YML(y) => fmt_text(y),
        }
    }
}

impl std::fmt::Display for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Element::fmt(self))
    }
}

/// Splits a link destination at the first `#` into its document part and anchor.
pub fn split_url(text: &str) -> (Option<String>, Option<String>) {
    match text.find('#') {
        None => (Some(text.to_string()), None),
        Some(0) => (None, Some(text.trim_start_matches('#').to_string())),
        Some(idx) => (Some(text[..idx].to_string()), Some(text[idx + 1..].to_string())),
    }
}

/// Like [`split_url`] but also reports the sub-ranges, which the parser needs to
/// point go-to-definition at the right part of the link.
pub fn split_url_node(url: &UrlEncodedNode) -> (Option<(String, Range)>, Option<(String, Range)>) {
    let text = &url.text;
    match text.find('#') {
        None => (Some((text.clone(), url.range)), None),
        Some(0) => {
            let anchor_text = text.trim_start_matches('#').to_string();
            let anchor_range = Range { start: misc::next_char(url.range.start, 1), end: url.range.end };
            (None, Some((anchor_text, anchor_range)))
        }
        Some(idx) => {
            let doc_text = text[..idx].to_string();
            let doc_range = Range {
                start: url.range.start,
                end: Position { line: url.range.start.line, character: url.range.start.character + idx as u32 },
            };
            let anchor_text = text[idx + 1..].to_string();
            let anchor_range = Range {
                start: Position {
                    line: url.range.start.line,
                    character: url.range.start.character + idx as u32 + 1,
                },
                end: url.range.end,
            };
            (Some((doc_text, doc_range)), Some((anchor_text, anchor_range)))
        }
    }
}

fn indent_by(text: &str, indent: usize) -> String {
    let pad = " ".repeat(indent);
    misc::lines_of(text)
        .into_iter()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn fmt_heading(node: &Node<Heading>) -> String {
    let inner = &node.data;
    let l1 = format!(
        "H{}: range={}; scope={}",
        inner.level,
        fmt_range(node.range),
        fmt_range(inner.scope)
    );
    let l2 = format!("  text=`{}`", node.text);
    let l3 = format!("  title=`{}` @ {}", inner.title.text, fmt_range(inner.title.range));
    [l1, l2, l3].join("\n")
}

fn fmt_wiki_link(node: &Node<WikiLink>) -> String {
    let first = format!("WL: {}; {}", node.text, fmt_range(node.range));
    let rest = indent_by(&node.data.fmt(), 2);
    if rest.is_empty() {
        first
    } else {
        format!("{first}\n{rest}")
    }
}

fn fmt_md_link(node: &Node<MdLink>) -> String {
    let first = format!("ML: {} @ {}", node.text, fmt_range(node.range));
    let rest = indent_by(&node.data.fmt(), 2);
    format!("{first}\n{rest}")
}

fn fmt_md_link_def(node: &Node<MdLinkDef>) -> String {
    let first = format!("MLD: {} @ {}", node.text, fmt_range(node.range));
    let rest = indent_by(&node.data.fmt(), 2);
    format!("{first}\n{rest}")
}

fn fmt_tag(node: &Node<Tag>) -> String {
    format!("T: {} @ {}", node.data.fmt(), fmt_range(node.range))
}

#[derive(Clone, Debug, Default)]
pub struct Cst {
    pub elements: Vec<Element>,
    /// Parent to children, as indices into `elements`. Storing the elements
    /// themselves would copy every element in the document twice.
    pub child_map: BTreeMap<usize, Vec<usize>>,
}

impl Cst {
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    pub fn children(&self, el: &Element) -> Vec<Element> {
        let Some(idx) = self.elements.iter().position(|e| e == el) else {
            return Vec::new();
        };
        match self.child_map.get(&idx) {
            Some(children) => children.iter().map(|i| self.elements[*i].clone()).collect(),
            None => Vec::new(),
        }
    }

    /// Headings that are not nested under any other heading. For
    /// `## L2 / # L1 / ## L3` the top level headings are L2 and L1, in document
    /// order: the running minimum level only ever decreases, so a deeper heading
    /// after a shallower one is nested.
    pub fn top_level_headings(&self) -> Vec<Node<Heading>> {
        let mut heads: Vec<Node<Heading>> = Vec::new();
        let mut level = 999i32;

        for el in &self.elements {
            if let Element::H(heading) = el {
                if heading.data.level <= level {
                    level = heading.data.level;
                    heads.push(heading.clone());
                }
            }
        }

        heads
    }

    pub fn element_at_pos(&self, pos: Position) -> Option<&Element> {
        self.elements
            .iter()
            .find(|el| misc::range_contains_inclusive(&el.range(), pos))
    }
}

/// Re-exported so callers can build a path without importing `paths`.
pub fn rel_path(s: impl Into<String>) -> RelPath {
    RelPath(s.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_splits_at_first_hash() {
        assert_eq!(split_url("a.md#sec"), (Some("a.md".into()), Some("sec".into())));
        assert_eq!(split_url("#sec"), (None, Some("sec".into())));
        assert_eq!(split_url("a.md"), (Some("a.md".into()), None));
    }

    #[test]
    fn top_level_headings_skip_nested_ones() {
        let heading = |level: i32, text: &str, line: u32| {
            Element::H(Node::mk(
                text.to_string(),
                misc::range(line, 0, line, text.len() as u32),
                Heading {
                    level,
                    is_title: false,
                    title: TextNode::mk_text(text, misc::range(line, 0, line, text.len() as u32)),
                    disambiguation: None,
                    scope: misc::range(line, 0, line, text.len() as u32),
                },
            ))
        };
        let cst = Cst {
            elements: vec![heading(2, "L2", 0), heading(1, "L1", 1), heading(2, "L3", 2)],
            child_map: BTreeMap::new(),
        };
        let headings = cst.top_level_headings();
        let titles: Vec<&str> = headings.iter().map(|h| h.data.name()).collect();
        assert_eq!(titles, vec!["L2", "L1"]);
    }

    #[test]
    fn element_formatting_matches_the_original() {
        let wl = Element::WL(Node::mk(
            "[[note]]".to_string(),
            misc::range(0, 0, 0, 8),
            WikiLink {
                doc: Some(Node::mk("note", misc::range(0, 2, 0, 6), WikiEncoded::mk_unchecked("note"))),
                heading: None,
            },
        ));
        assert_eq!(wl.fmt(), "WL: [[note]]; (0,0)-(0,8)\n  doc=note; (0,2)-(0,6)");
    }

    #[test]
    fn heading_slug_applies_disambiguation() {
        let heading = Heading {
            level: 2,
            is_title: false,
            title: TextNode::mk_text("Dup", misc::range(0, 0, 0, 3)),
            disambiguation: Some("1".into()),
            scope: misc::range(0, 0, 0, 3),
        };
        assert_eq!(heading.slug().to_string(), "dup-1");
    }

    #[test]
    fn wiki_link_content_range_spans_doc_and_heading() {
        let wl = WikiLink {
            doc: Some(Node::mk("d", misc::range(0, 2, 0, 3), WikiEncoded::mk_unchecked("d"))),
            heading: Some(Node::mk("h", misc::range(0, 4, 0, 5), WikiEncoded::mk_unchecked("h"))),
        };
        assert_eq!(wl.content_range(), Some(misc::range(0, 2, 0, 5)));
    }
}
