//! Completion for wiki links, inline links, reference links, anchors and tags.
//!
//! Port of `Marksman.Compl`. Candidates carry a `filterText` so that the client's
//! own fuzzy matcher does the narrowing; the server only decides which context
//! the cursor is in and which candidates apply.

use std::collections::BTreeSet;

use lsp_types::{CompletionItem, CompletionItemKind, Documentation, Position, Range, TextEdit};
use log::trace;

use crate::config::{ComplWikiStyle, Config};
use crate::cst::{
    self, Element, MdLink, MdLinkDef, Node, TextNode, UrlEncodedNode, WikiEncodedNode, WikiLink,
};
use crate::doc::Doc;
use crate::folder::Folder;
use crate::misc::{self, LinkLabel, Slug};
use crate::names::{InternName, InternPath, UrlEncoded, WikiEncoded};
use crate::paths::{CanonDocPath, RelPath, RootedRelPath};
use crate::refs::FileLink;
use crate::text::{Cursor, Line, Text};

/// A partially typed element, reconstructed from the cursor's line.
#[derive(Clone, Debug)]
pub enum PartialElement {
    WikiLink {
        dest: Option<WikiEncodedNode>,
        heading: Option<WikiEncodedNode>,
        range: Range,
    },
    InlineLink {
        text: Option<TextNode>,
        path: Option<UrlEncodedNode>,
        anchor: Option<UrlEncodedNode>,
        range: Range,
    },
    ReferenceLink {
        label: Option<TextNode>,
        range: Range,
    },
    TagOpening(Position),
}

impl std::fmt::Display for PartialElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartialElement::WikiLink { dest, heading, range } => write!(
                f,
                "WL {}: dest={}; heading={}",
                cst::fmt_range(*range),
                cst::fmt_opt_wiki(dest),
                cst::fmt_opt_wiki(heading)
            ),
            PartialElement::InlineLink { text, path, anchor, range } => write!(
                f,
                "IL {}: text={}; path={}; anchor={}",
                cst::fmt_range(*range),
                cst::fmt_opt_text(text),
                cst::fmt_opt_url(path),
                cst::fmt_opt_url(anchor)
            ),
            PartialElement::ReferenceLink { label, range } => {
                write!(f, "RL {}: label={}", cst::fmt_range(*range), cst::fmt_opt_text(label))
            }
            PartialElement::TagOpening(pos) => write!(f, "TO: cursorPos=({},{})", pos.line, pos.character),
        }
    }
}

/// Splits a text node at the first `#` into its document part and anchor,
/// tagging both halves with `coder`.
pub fn url_of_text_node<T: Clone>(
    node: &TextNode,
    coder: impl Fn(&str) -> T,
) -> (Option<Node<T>>, Option<Node<T>>) {
    let text = &node.text;
    match text.find('#') {
        None => (Some(Node::mk(text.clone(), node.range, coder(text))), None),
        Some(0) => {
            let anchor_text = text.trim_start_matches('#').to_string();
            let range = Range { start: misc::next_char(node.range.start, 1), ..node.range };
            (None, Some(Node::mk(anchor_text.clone(), range, coder(&anchor_text))))
        }
        Some(idx) => {
            let doc_text = text[..idx].to_string();
            let doc_range = Range {
                end: Position { line: node.range.start.line, character: node.range.start.character + idx as u32 },
                ..node.range
            };
            let anchor_text = text[idx + 1..].to_string();
            let anchor_range = Range {
                start: Position {
                    line: node.range.start.line,
                    character: node.range.start.character + idx as u32 + 1,
                },
                ..node.range
            };
            (
                Some(Node::mk(doc_text.clone(), doc_range, coder(&doc_text))),
                Some(Node::mk(anchor_text.clone(), anchor_range, coder(&anchor_text))),
            )
        }
    }
}

fn url_of_url_node(url: &UrlEncodedNode) -> (Option<UrlNode>, Option<UrlNode>) {
    url_of_text_node(&url.as_text(), |s: &str| UrlEncoded::mk_unchecked(s))
}

pub type UrlNode = Node<UrlEncoded>;
pub type WikiNode = Node<WikiEncoded>;

fn cursor_pos(cursor: &Option<Cursor>) -> Option<Position> {
    cursor.as_ref().map(Cursor::pos)
}

fn is_link_stop(c: char) -> bool {
    c.is_whitespace() || c == ']' || c == '[' || c == ')' || c == '('
}

fn find_backward(cursor: &Cursor, pred: impl Fn(char) -> bool) -> Option<Cursor> {
    cursor.try_find_char_matching(|c| c.backward(), pred)
}

fn find_forward(cursor: &Cursor, pred: impl Fn(char) -> bool) -> Option<Cursor> {
    cursor.try_find_char_matching(|c| c.forward(), pred)
}

impl PartialElement {
    pub fn range(&self) -> Range {
        match self {
            PartialElement::WikiLink { range, .. }
            | PartialElement::InlineLink { range, .. }
            | PartialElement::ReferenceLink { range, .. } => *range,
            // An empty range: nothing has been typed yet.
            PartialElement::TagOpening(pos) => Range { start: *pos, end: *pos },
        }
    }

    pub fn link_in_line(line: &Line, pos: Position) -> Option<PartialElement> {
        let start_cursor = match line.to_cursor_at(pos) {
            Some(cursor) => cursor.backward(),
            None if line.ends_at(pos) => line.end_cursor(),
            None => None,
        }?;

        let preceding_punct = find_backward(&start_cursor, |c| c == '[' || c == '(')?;
        let line_range = line.range();
        let line_end = line_range.end;

        match (preceding_punct.backward_char(), preceding_punct.char()) {
            (Some('['), '[') => {
                // A wiki link: the two opening brackets are adjacent.
                let element_start = preceding_punct.backward()?;

                let input_end = start_cursor.forward().and_then(|c| find_forward(&c, is_link_stop));

                let element_end = match &input_end {
                    Some(end_cursor) if end_cursor.char() == ']' => end_cursor.forward(),
                    other => other.clone(),
                };

                let element_range_end = cursor_pos(&element_end).unwrap_or(line_end);
                let element_range = Range { start: element_start.pos(), end: element_range_end };

                let (dest, heading) = preceding_punct
                    .forward()
                    .filter(|x| x.pos() < element_range_end)
                    .map(|input_start| {
                        let input_range_start = input_start.pos();
                        let input_range_end = cursor_pos(&input_end).unwrap_or(line_end);
                        let input_range = Range { start: input_range_start, end: input_range_end };

                        let input_node = Node::mk_text(
                            line.text.substring(input_range),
                            input_range,
                        );
                        url_of_text_node(&input_node, |s: &str| WikiEncoded::mk_unchecked(s))
                    })
                    .unwrap_or((None, None));

                Some(PartialElement::WikiLink { dest, heading, range: element_range })
            }
            (_, '[') => {
                let element_start = preceding_punct.clone();
                let input_start = preceding_punct.forward();

                let input_end = start_cursor
                    .forward()
                    .and_then(|c| find_forward(&c, |c| c.is_whitespace() || c == ']'));

                let element_end = match &input_end {
                    Some(end_cursor) if end_cursor.char() == ']' => end_cursor.forward(),
                    other => other.clone(),
                };

                let element_range_end = cursor_pos(&element_end).unwrap_or(line_end);

                let input_range = Range {
                    start: cursor_pos(&input_start).unwrap_or(line_end),
                    end: cursor_pos(&input_end).unwrap_or(line_end),
                };

                let input = if misc::range_is_empty(&input_range) {
                    None
                } else {
                    input_start.as_ref().map(|_| line.text.substring(input_range))
                };

                let input_node =
                    input.map(|text| Node::mk_text(text.clone(), input_range));

                let element_range = Range { start: element_start.pos(), end: element_range_end };

                Some(PartialElement::ReferenceLink { label: input_node, range: element_range })
            }
            (_, '(') => {
                // An inline link URL cannot contain whitespace; bail out if the
                // nearest boundary going back is past the opening paren.
                let double_check_start =
                    find_backward(&start_cursor, |c| c.is_whitespace() || matches!(c, '[' | ']' | '(' | ')'))?;
                if !Cursor::is_before_or_at(&double_check_start, &preceding_punct) {
                    return None;
                }

                let preceding_closing_bracket =
                    preceding_punct.backward().filter(|c| c.char() == ']');

                let potential_start = preceding_closing_bracket
                    .as_ref()
                    .and_then(Cursor::backward)
                    .and_then(|c| find_backward(&c, |ch| matches!(ch, '[' | ']' | '(' | ')')));

                let (element_start, text_node) = match &potential_start {
                    Some(c) if c.char() == '[' => {
                        // A proper inline link start: `[text](`
                        let text_start = c.forward();
                        let text_end = preceding_punct.backward();

                        let text_node = match (&text_start, &text_end) {
                            (Some(s), Some(e)) => {
                                let range = Range { start: s.pos(), end: e.pos() };
                                let text = line.text.substring(range);
                                Some(Node::mk_text(text, range))
                            }
                            _ => None,
                        };
                        (c.clone(), text_node)
                    }
                    _ => (preceding_punct.clone(), None),
                };

                let label_start = preceding_punct.forward();
                let label_end =
                    start_cursor.forward().and_then(|c| find_forward(&c, is_link_stop));

                let label_range = Range {
                    start: cursor_pos(&label_start).unwrap_or(line_end),
                    end: cursor_pos(&label_end).unwrap_or(line_end),
                };

                let label_node = if misc::range_is_empty(&label_range) {
                    None
                } else {
                    Some(Node::mk_text(line.text.substring(label_range), label_range))
                };

                let (path, anchor) = label_node
                    .as_ref()
                    .map(|n| url_of_text_node(n, |s: &str| UrlEncoded::mk_unchecked(s)))
                    .unwrap_or((None, None));

                let element_end = match &label_end {
                    Some(end_cursor) if end_cursor.char() == ')' => end_cursor.forward(),
                    other => other.clone(),
                };
                let element_range_end = cursor_pos(&element_end).unwrap_or(line_end);
                let element_range = Range { start: element_start.pos(), end: element_range_end };

                Some(PartialElement::InlineLink { text: text_node, path, anchor, range: element_range })
            }
            _ => None,
        }
    }

    pub fn tag_opening_in_line(line: &Line, pos: Position) -> Option<PartialElement> {
        let potential_hash = line
            .to_cursor_at(pos)
            .and_then(|c| c.backward())
            // The cursor can be at EOF, so also check the end of the line.
            .or_else(|| line.end_cursor())
            .map(|c| c.char());

        match potential_hash {
            Some('#') => Some(PartialElement::TagOpening(pos)),
            _ => None,
        }
    }

    pub fn in_line(line: &Line, pos: Position) -> Option<PartialElement> {
        PartialElement::link_in_line(line, pos)
            .or_else(|| PartialElement::tag_opening_in_line(line, pos))
    }

    pub fn in_text(text: &Text, pos: Position) -> Option<PartialElement> {
        Line::of_pos(text.clone(), pos).and_then(|l| PartialElement::in_line(&l, pos))
    }
}

#[derive(Clone, Debug)]
pub enum Completable {
    E(Element),
    PE(PartialElement),
}

impl Completable {
    pub fn is_partial(&self) -> bool {
        matches!(self, Completable::PE(_))
    }

    pub fn range(&self) -> Range {
        match self {
            Completable::E(el) => el.range(),
            Completable::PE(pel) => pel.range(),
        }
    }
}

impl std::fmt::Display for Completable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Completable::E(el) => write!(f, "E {}", el.fmt()),
            Completable::PE(pel) => write!(f, "PE {pel}"),
        }
    }
}

/// What the cursor is asking to be completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    WikiDoc(String),
    WikiHeadingInSrcDoc(String),
    WikiHeadingInOtherDoc { dest_part: String, heading_part: String },
    Reference(String),
    InlineDoc(String),
    InlineAnchorInSrcDoc(String),
    InlineAnchorInOtherDoc { path_part: String, anchor_part: String },
    Tag(String),
}

impl Prompt {
    pub fn of_completable(pos: Position, compl: &Completable) -> Option<Prompt> {
        let element_range = compl.range();
        if !misc::range_contains_inclusive(&element_range, pos) {
            return None;
        }

        match compl {
            // No completion inside a heading, a link definition or front matter.
            Completable::E(Element::H(_))
            | Completable::E(Element::MLD(_))
            | Completable::E(Element::YML(_)) => None,
            Completable::E(Element::WL(node)) => match (&node.data.doc, &node.data.heading) {
                (_, None) => Some(Prompt::WikiDoc(cst::node_text_opt(&node.data.doc, ""))),
                (None, Some(heading)) => Some(Prompt::WikiHeadingInSrcDoc(heading.text.clone())),
                (Some(doc), Some(heading)) => {
                    if misc::range_contains_inclusive(&doc.range, pos) {
                        Some(Prompt::WikiDoc(doc.text.clone()))
                    } else {
                        Some(Prompt::WikiHeadingInOtherDoc {
                            dest_part: doc.text.clone(),
                            heading_part: heading.text.clone(),
                        })
                    }
                }
            },
            Completable::E(Element::ML(node)) => match &node.data {
                MdLink::RF(_, label) | MdLink::RC(label) | MdLink::RS(label) => {
                    Some(Prompt::Reference(label.text.clone()))
                }
                MdLink::IL(_, None, _) => Some(Prompt::InlineDoc(String::new())),
                MdLink::IL(_, Some(url), _) => {
                    let (path, anchor) = url_of_url_node(url);
                    match (path, anchor) {
                        (Some(path), None) => Some(Prompt::InlineDoc(path.text)),
                        (None, Some(anchor)) => Some(Prompt::InlineAnchorInSrcDoc(anchor.text)),
                        (Some(path), Some(anchor)) => {
                            if misc::range_contains_inclusive(&path.range, pos) {
                                Some(Prompt::InlineDoc(path.text))
                            } else {
                                Some(Prompt::InlineAnchorInOtherDoc {
                                    path_part: path.text,
                                    anchor_part: anchor.text,
                                })
                            }
                        }
                        (None, None) => None,
                    }
                }
            },
            Completable::E(Element::T(node)) => Some(Prompt::Tag(node.data.name.text.clone())),
            Completable::PE(PartialElement::WikiLink { dest, heading, .. }) => match (dest, heading)
            {
                (_, None) => Some(Prompt::WikiDoc(cst::node_text_opt(dest, ""))),
                (None, Some(heading)) => Some(Prompt::WikiHeadingInSrcDoc(heading.text.clone())),
                (Some(dest), Some(heading)) => Some(Prompt::WikiHeadingInOtherDoc {
                    dest_part: dest.text.clone(),
                    heading_part: heading.text.clone(),
                }),
            },
            Completable::PE(PartialElement::InlineLink { path, anchor, .. }) => match (path, anchor)
            {
                (_, None) => Some(Prompt::InlineDoc(cst::node_text_opt(path, ""))),
                (None, Some(anchor)) => Some(Prompt::InlineAnchorInSrcDoc(anchor.text.clone())),
                (Some(path), Some(anchor)) => Some(Prompt::InlineAnchorInOtherDoc {
                    path_part: path.text.clone(),
                    anchor_part: anchor.text.clone(),
                }),
            },
            Completable::PE(PartialElement::ReferenceLink { label, .. }) => {
                Some(Prompt::Reference(cst::node_text_opt(label, "")))
            }
            Completable::PE(PartialElement::TagOpening(_)) => Some(Prompt::Tag(String::new())),
        }
    }
}

/// How a wiki link to `doc` should be spelled, per the configured style.
pub fn wiki_target_link(config: &Config, doc: &Doc) -> cst::WikiDest {
    let doc_path = doc.path_from_root();

    match config.compl_wiki_style() {
        ComplWikiStyle::TitleSlug => cst::WikiDest::WTitle(misc::slugify(&doc.name())),
        ComplWikiStyle::FileStem => {
            let name = doc_path.filename_stem().to_string();
            cst::WikiDest::WPath(InternPath::Approx(RelPath(name)))
        }
        ComplWikiStyle::FilePathStem => {
            let rel_path = CanonDocPath::mk(&config.core_markdown_file_extensions(), &doc_path)
                .to_rel();
            cst::WikiDest::WPath(InternPath::ExactAbs(RootedRelPath::mk(
                doc.root_path().clone(),
                crate::paths::LocalPath::Rel(rel_path),
            )))
        }
    }
}

fn item(label: impl Into<String>) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: Some(CompletionItemKind::REFERENCE),
        ..Default::default()
    }
}

pub mod completions {
    use super::*;

    pub fn wiki_doc(config: &Config, pos: Position, compl: &Completable, doc: &Doc) -> Option<CompletionItem> {
        let target_name = doc.name();
        let target_link = wiki_target_link(config, doc);

        let (input, heading, range) = match compl {
            Completable::E(Element::WL(node)) => {
                (node.data.doc.clone(), node.data.heading.clone(), node.range)
            }
            Completable::PE(PartialElement::WikiLink { dest, heading, range }) => {
                (dest.clone(), heading.clone(), *range)
            }
            _ => return None,
        };

        let input_range = input
            .as_ref()
            .map(|n| n.range)
            .unwrap_or_else(|| Range { start: pos, end: pos });

        match heading {
            None => {
                let new_text = WikiLink::render(
                    Some(&target_link.encode()),
                    None,
                    compl.is_partial(),
                );
                let filter_text = WikiLink::render(
                    Some(&WikiEncoded::mk_unchecked(target_name.clone())),
                    None,
                    compl.is_partial(),
                );

                let range = if compl.is_partial() { range } else { input_range };

                Some(CompletionItem {
                    detail: Some(doc.path_from_root().to_system().to_string()),
                    text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit { range, new_text })),
                    filter_text: Some(filter_text),
                    ..item(target_name)
                })
            }
            Some(_) => {
                let new_text = WikiEncoded::raw(&target_link.encode()).to_string();
                Some(CompletionItem {
                    detail: Some(doc.path_from_root().to_system().to_string()),
                    text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                        range: input_range,
                        new_text,
                    })),
                    filter_text: Some(target_name.clone()),
                    ..item(target_name)
                })
            }
        }
    }

    pub fn wiki_heading_in_src_doc(
        compl: &Completable,
        completion_heading: &str,
    ) -> Option<CompletionItem> {
        let (input, range) = match compl {
            Completable::E(Element::WL(node)) => match (&node.data.doc, &node.data.heading) {
                (None, Some(input)) => (Some(input.clone()), node.range),
                _ => return None,
            },
            Completable::PE(PartialElement::WikiLink { dest: None, heading: Some(input), range }) => {
                (Some(input.clone()), *range)
            }
            _ => return None,
        };

        let input = input?;
        let new_text = WikiLink::render(
            None,
            Some(&WikiEncoded::encode(completion_heading)),
            compl.is_partial(),
        );

        let range = if compl.is_partial() {
            range
        } else {
            // The `#` that precedes the input is part of what gets replaced.
            Range {
                start: Position {
                    line: input.range.start.line,
                    character: input.range.start.character.saturating_sub(1),
                },
                end: input.range.end,
            }
        };

        Some(CompletionItem {
            text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit { range, new_text: new_text.clone() })),
            filter_text: Some(new_text),
            ..item(completion_heading.to_string())
        })
    }

    pub fn wiki_heading_in_other_doc(
        config: &Config,
        compl: &Completable,
        doc: &Doc,
        heading: &str,
    ) -> Option<CompletionItem> {
        let label = format!("{} / {heading}", doc.name());

        let (dest_part, heading_part, range) = match compl {
            Completable::E(Element::WL(node)) => {
                match (&node.data.doc, &node.data.heading) {
                    (Some(dest_part), Some(heading_part)) => {
                        (dest_part.clone(), heading_part.clone(), node.range)
                    }
                    _ => return None,
                }
            }
            Completable::PE(PartialElement::WikiLink {
                dest: Some(dest_part),
                heading: Some(heading_part),
                range,
            }) => (dest_part.clone(), heading_part.clone(), *range),
            _ => return None,
        };

        let target_link = wiki_target_link(config, doc);

        let new_text = WikiLink::render(
            Some(&target_link.encode()),
            Some(&WikiEncoded::encode(heading)),
            compl.is_partial(),
        );

        let filter_text = WikiLink::render(
            Some(&dest_part.data),
            Some(&WikiEncoded::mk_unchecked(heading.to_string())),
            compl.is_partial(),
        );

        let range = if compl.is_partial() {
            range
        } else {
            Range { start: dest_part.range.start, end: heading_part.range.end }
        };

        Some(CompletionItem {
            detail: Some(doc.path_from_root().to_system().to_string()),
            text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit { range, new_text })),
            filter_text: Some(filter_text),
            ..item(label)
        })
    }

    pub fn reference(pos: Position, compl: &Completable, def: &MdLinkDef) -> Option<CompletionItem> {
        let (label, range) = match compl {
            Completable::E(Element::ML(node)) => match &node.data {
                MdLink::RF(_, label) | MdLink::RC(label) | MdLink::RS(label) => {
                    (Some(label.clone()), node.range)
                }
                _ => return None,
            },
            Completable::PE(PartialElement::ReferenceLink { label, range }) => {
                (label.clone(), *range)
            }
            _ => return None,
        };

        let label_range = label
            .as_ref()
            .map(|n| n.range)
            .unwrap_or_else(|| Range { start: pos, end: pos });

        let range = if compl.is_partial() { range } else { label_range };
        let link_def_label = def.label.text.clone();

        let new_text = if compl.is_partial() {
            format!("[{link_def_label}]")
        } else {
            link_def_label.clone()
        };

        Some(CompletionItem {
            detail: def.title.as_ref().map(|t| t.text.clone()),
            documentation: Some(Documentation::String(def.url.text.clone())),
            text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: new_text.clone(),
            })),
            filter_text: Some(new_text),
            ..item(link_def_label)
        })
    }

    pub fn inline_doc(pos: Position, compl: &Completable, doc: &Doc) -> Option<CompletionItem> {
        let target_path = doc.path_from_root().to_system().to_string();
        let target_path_encoded = misc::abs_path_url_encode(&target_path);

        let detail = {
            let name = doc.name();
            if name != target_path {
                Some(name)
            } else {
                None
            }
        };

        let edit = |range: Range, new_text: String, filter_text: Option<String>| {
            CompletionItem {
                detail: detail.clone(),
                text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit { range, new_text })),
                filter_text,
                ..item(target_path.clone())
            }
        };

        match compl {
            Completable::E(Element::ML(node)) => match &node.data {
                MdLink::IL(_, None, _) => Some(edit(
                    Range { start: pos, end: pos },
                    target_path_encoded,
                    None,
                )),
                MdLink::IL(_, Some(url), _) => {
                    let (path, _) = url_of_url_node(url);
                    let path = path?;
                    Some(edit(path.range, target_path_encoded, None))
                }
                _ => None,
            },
            Completable::PE(PartialElement::InlineLink {
                text: Some(_),
                path,
                anchor: Some(_),
                ..
            }) => {
                let range = path
                    .as_ref()
                    .map(|n| n.range)
                    .unwrap_or_else(|| Range { start: pos, end: pos });
                Some(edit(range, target_path_encoded, None))
            }
            Completable::PE(PartialElement::InlineLink { text: Some(text), path: None, anchor: None, range }) => {
                let new_text = MdLink::render_inline(Some(&text.text), Some(&target_path_encoded), None);
                Some(edit(*range, new_text.clone(), Some(new_text)))
            }
            _ => None,
        }
    }

    pub fn inline_anchor_in_src_doc(
        compl: &Completable,
        completion_heading: &str,
    ) -> Option<CompletionItem> {
        let heading_slug = misc::slugify(completion_heading);

        match compl {
            Completable::E(Element::ML(node)) => match &node.data {
                MdLink::IL(_, Some(url), _) => {
                    let (path, anchor) = url_of_url_node(url);
                    match (path, anchor) {
                        (None, Some(anchor)) => Some(CompletionItem {
                            text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                                range: anchor.range,
                                new_text: heading_slug.clone(),
                            })),
                            filter_text: Some(heading_slug),
                            ..item(completion_heading.to_string())
                        }),
                        _ => None,
                    }
                }
                _ => None,
            },
            Completable::PE(PartialElement::InlineLink {
                text: Some(text),
                path: None,
                anchor: Some(_),
                range,
            }) => {
                let new_text = format!("[{}](#{heading_slug})", text.text);
                Some(CompletionItem {
                    text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                        range: *range,
                        new_text: new_text.clone(),
                    })),
                    filter_text: Some(new_text),
                    ..item(completion_heading.to_string())
                })
            }
            _ => None,
        }
    }

    pub fn inline_anchor_in_other_doc(
        compl: &Completable,
        target_doc: &Doc,
        target_heading: &str,
    ) -> Option<CompletionItem> {
        let target_path = target_doc.path_from_root().to_system().to_string();
        let target_path_encoded = misc::abs_path_url_encode(&target_path);
        let label = format!("{target_path} / {target_heading}");

        let detail = {
            let name = target_doc.name();
            if name != target_path {
                Some(name)
            } else {
                None
            }
        };

        match compl {
            Completable::E(Element::ML(node)) => match &node.data {
                MdLink::IL(_, Some(url), _) => {
                    let (path, anchor) = url_of_url_node(url);
                    match (path, anchor) {
                        (Some(path), Some(anchor)) => {
                            let new_text = format!("{target_path_encoded}#{}", misc::slugify(target_heading));
                            let new_range = Range { start: path.range.start, end: anchor.range.end };
                            let filter_text = format!("{target_path_encoded}#{target_heading}");

                            Some(CompletionItem {
                                detail,
                                text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                                    range: new_range,
                                    new_text,
                                })),
                                filter_text: Some(filter_text),
                                ..item(label)
                            })
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            Completable::PE(PartialElement::InlineLink {
                text: Some(text),
                path: Some(_),
                anchor: Some(_),
                range,
            }) => {
                let new_text = format!(
                    "[{}]({target_path_encoded}#{})",
                    text.text,
                    misc::slugify(target_heading)
                );
                let filter_text = format!("[{}]({target_path_encoded}#{target_heading})", text.text);

                Some(CompletionItem {
                    detail,
                    text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                        range: *range,
                        new_text,
                    })),
                    filter_text: Some(filter_text),
                    ..item(label)
                })
            }
            _ => None,
        }
    }

    pub fn tag(compl: &Completable, tag_name: &str, num_usages: usize) -> Option<CompletionItem> {
        let range = match compl {
            Completable::E(Element::T(node)) => Some(node.data.name.range),
            Completable::PE(pe @ PartialElement::TagOpening(_)) => Some(pe.range()),
            _ => None,
        }?;

        let detail = format!("{num_usages} usages");

        Some(CompletionItem {
            detail: Some(detail),
            text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: tag_name.to_string(),
            })),
            ..item(tag_name.to_string())
        })
    }
}

pub mod candidates {
    use super::*;

    pub fn find_doc_candidates(
        folder: &Folder,
        src_doc: &Doc,
        dest_part: Option<&InternName>,
    ) -> Vec<Doc> {
        let candidates: Vec<Doc> = match dest_part {
            None => folder.docs(),
            Some(name) => FileLink::filter_fuzzy_matching_docs(folder, name),
        };

        candidates.into_iter().filter(|d| d != src_doc).collect()
    }

    pub fn find_heading_candidates(
        folder: &Folder,
        src_doc: &Doc,
        dest_part: Option<&InternName>,
        heading_part: &str,
    ) -> Vec<(Doc, String)> {
        let mut target_docs: Vec<Doc> = match dest_part {
            Some(name) => FileLink::filter_fuzzy_matching_docs(folder, name),
            None => vec![src_doc.clone()],
        };

        if dest_part.is_some() {
            target_docs.retain(|d| d != src_doc);
        }

        let input_slug = Slug::of_string(heading_part);

        target_docs
            .into_iter()
            .flat_map(|d| {
                // There may be several headings with the same name; the set
                // removes duplicates from the completion candidates.
                let headings: BTreeSet<String> = d
                    .index()
                    .headings
                    .iter()
                    // Titles are completed as documents, not as headings.
                    .filter(|h| !h.data.is_title())
                    .map(|h| h.data.name().to_string())
                    .filter(|h| input_slug.is_subsequence(&Slug::of_string(h)))
                    .collect();
                headings.into_iter().map(move |h| (d.clone(), h))
            })
            .collect()
    }

    pub fn find_link_def_candidates(src_doc: &Doc, input: &str) -> Vec<MdLinkDef> {
        let input_label = LinkLabel::of_string(input);
        src_doc
            .index()
            .filter_link_defs(|label| input_label.is_subsequence_of(label))
            .into_iter()
            .map(|node| node.data.clone())
            .collect()
    }

    pub fn find_tag_candidates(folder: &Folder, input: &str) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();

        for doc in folder.docs() {
            for tag in &doc.index().tags {
                let tag_name = &tag.data.name.text;
                if misc::is_subsequence_of(&input.to_lowercase(), &tag_name.to_lowercase())
                    && input != tag_name
                {
                    match counts.iter_mut().find(|(name, _)| name == tag_name) {
                        Some((_, n)) => *n += 1,
                        None => counts.push((tag_name.clone(), 1)),
                    }
                }
            }
        }

        // `Seq.countBy id` in the original preserves first-appearance order,
        // which is what the completion list shows.
        counts
    }
}

/// The element or partially typed element the cursor sits in.
pub fn find_completable_at_pos(doc: &Doc, pos: Position) -> Option<Completable> {
    let link = || doc.index().link_at_pos(pos).map(Completable::E);

    let tag = || {
        doc.index()
            .tags
            .iter()
            // Inclusive because we want to cover the cursor sitting right after
            // the tag's end.
            .find(|t| misc::range_contains_inclusive(&t.data.name.range, pos))
            .map(|t| Completable::E(Element::T(t.clone())))
    };

    let partial_element = || {
        PartialElement::in_text(doc.text(), pos).map(Completable::PE)
    };

    // The priority is generally link > partial element > tag. However, when the
    // partial element is a tag opening, check for a proper tag first. In
    // particular this means `[[#f` completes as a wiki link rather than a tag.
    match link() {
        some @ Some(_) => some,
        None => match partial_element() {
            tag_opening @ Some(Completable::PE(PartialElement::TagOpening(_))) => {
                tag().or(tag_opening)
            }
            Some(pe) => Some(pe),
            None => tag(),
        },
    }
}

pub fn find_candidates_for_compl(
    folder: &Folder,
    src_doc: &Doc,
    pos: Position,
    compl: &Completable,
) -> Vec<CompletionItem> {
    let config = folder.config_or_default();

    match Prompt::of_completable(pos, compl) {
        None => Vec::new(),
        Some(Prompt::WikiDoc(input)) => {
            let dest_part = InternName::mk_unchecked(src_doc.id().clone(), input);
            candidates::find_doc_candidates(folder, src_doc, Some(&dest_part))
                .into_iter()
                .filter_map(|doc| completions::wiki_doc(&config, pos, compl, &doc))
                .collect()
        }
        Some(Prompt::WikiHeadingInSrcDoc(input)) => {
            candidates::find_heading_candidates(folder, src_doc, None, &input)
                .into_iter()
                .map(|(_, heading)| heading)
                .filter_map(|heading| completions::wiki_heading_in_src_doc(compl, &heading))
                .collect()
        }
        Some(Prompt::WikiHeadingInOtherDoc { dest_part, heading_part }) => {
            let dest_part = InternName::mk_unchecked(src_doc.id().clone(), dest_part);
            candidates::find_heading_candidates(folder, src_doc, Some(&dest_part), &heading_part)
                .into_iter()
                .filter_map(|(doc, heading)| {
                    completions::wiki_heading_in_other_doc(&config, compl, &doc, &heading)
                })
                .collect()
        }
        Some(Prompt::Reference(input)) => candidates::find_link_def_candidates(src_doc, &input)
            .into_iter()
            .filter_map(|def| completions::reference(pos, compl, &def))
            .collect(),
        Some(Prompt::InlineDoc(input)) => {
            let exts = config.core_markdown_file_extensions();
            let cand = match InternName::mk_checked(&exts, src_doc.id().clone(), input.clone()) {
                None if input.is_empty() => candidates::find_doc_candidates(folder, src_doc, None),
                None => Vec::new(),
                Some(dest_part) => {
                    candidates::find_doc_candidates(folder, src_doc, Some(&dest_part))
                }
            };
            cand.into_iter()
                .filter_map(|doc| completions::inline_doc(pos, compl, &doc))
                .collect()
        }
        Some(Prompt::InlineAnchorInSrcDoc(input)) => {
            candidates::find_heading_candidates(folder, src_doc, None, &input)
                .into_iter()
                .map(|(_, heading)| heading)
                .filter_map(|heading| completions::inline_anchor_in_src_doc(compl, &heading))
                .collect()
        }
        Some(Prompt::InlineAnchorInOtherDoc { path_part, anchor_part }) => {
            let exts = config.core_markdown_file_extensions();
            let cand = match InternName::mk_checked(&exts, src_doc.id().clone(), path_part) {
                None => Vec::new(),
                Some(dest_part) => candidates::find_heading_candidates(
                    folder,
                    src_doc,
                    Some(&dest_part),
                    &anchor_part,
                ),
            };
            cand.into_iter()
                .filter_map(|(doc, heading)| {
                    completions::inline_anchor_in_other_doc(compl, &doc, &heading)
                })
                .collect()
        }
        Some(Prompt::Tag(input)) => candidates::find_tag_candidates(folder, &input)
            .into_iter()
            .filter_map(|(name, usages)| completions::tag(compl, &name, usages))
            .collect(),
    }
}

pub fn find_candidates_in_doc(folder: &Folder, doc: &Doc, pos: Position) -> Vec<CompletionItem> {
    match find_completable_at_pos(doc, pos) {
        None => {
            trace!("No completion point found");
            Vec::new()
        }
        Some(compl) => {
            trace!("Found completion point: comp={compl}");
            find_candidates_for_compl(folder, doc, pos, &compl)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::FolderId;
    use crate::text::mk_text;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-compl-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn load(dir: &std::path::Path) -> Folder {
        let id = FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        Folder::try_load(None, "test", id).unwrap()
    }

    fn doc_named(folder: &Folder, name: &str) -> Doc {
        folder.docs().into_iter().find(|d| d.name() == name).unwrap()
    }

    fn doc_of(content: &str) -> Doc {
        Doc::mk(
            &crate::config::ParserSettings::default(),
            crate::names::DocId::mk_rooted(
                &FolderId::of_uri("file:///w"),
                crate::paths::LocalPath::of_system("/w/a.md"),
            ),
            Some(1),
            mk_text(content),
        )
        .unwrap()
    }

    fn edit_text(item: &CompletionItem) -> String {
        match &item.text_edit {
            Some(lsp_types::CompletionTextEdit::Edit(e)) => e.new_text.clone(),
            other => panic!("expected a text edit, got {other:?}"),
        }
    }

    fn edit_range(item: &CompletionItem) -> Range {
        match &item.text_edit {
            Some(lsp_types::CompletionTextEdit::Edit(e)) => e.range,
            other => panic!("expected a text edit, got {other:?}"),
        }
    }

    #[test]
    fn detects_a_partial_wiki_link() {
        let doc = doc_of("[[no\n");
        let compl = find_completable_at_pos(&doc, Position { line: 0, character: 4 }).unwrap();
        assert_eq!(
            Prompt::of_completable(Position { line: 0, character: 4 }, &compl),
            Some(Prompt::WikiDoc("no".into()))
        );
    }

    #[test]
    fn detects_a_partial_wiki_link_with_heading() {
        let doc = doc_of("[[note#hea\n");
        let pos = Position { line: 0, character: 10 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(
            Prompt::of_completable(pos, &compl),
            Some(Prompt::WikiHeadingInOtherDoc { dest_part: "note".into(), heading_part: "hea".into() })
        );
    }

    #[test]
    fn detects_an_intra_document_wiki_heading() {
        let doc = doc_of("[[#hea\n");
        let pos = Position { line: 0, character: 6 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(
            Prompt::of_completable(pos, &compl),
            Some(Prompt::WikiHeadingInSrcDoc("hea".into()))
        );
    }

    #[test]
    fn detects_a_complete_wiki_link() {
        let doc = doc_of("[[note]]\n");
        let pos = Position { line: 0, character: 4 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert!(!compl.is_partial());
        assert_eq!(Prompt::of_completable(pos, &compl), Some(Prompt::WikiDoc("note".into())));
    }

    #[test]
    fn detects_a_partial_inline_link_path() {
        let doc = doc_of("[text](not\n");
        let pos = Position { line: 0, character: 10 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(Prompt::of_completable(pos, &compl), Some(Prompt::InlineDoc("not".into())));
    }

    #[test]
    fn detects_a_partial_inline_link_anchor() {
        let doc = doc_of("[text](note.md#he\n");
        let pos = Position { line: 0, character: 17 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(
            Prompt::of_completable(pos, &compl),
            Some(Prompt::InlineAnchorInOtherDoc { path_part: "note.md".into(), anchor_part: "he".into() })
        );
    }

    #[test]
    fn detects_a_reference_link_prompt() {
        let doc = doc_of("[lab\n\n[lab]: /url\n");
        let pos = Position { line: 0, character: 4 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(Prompt::of_completable(pos, &compl), Some(Prompt::Reference("lab".into())));
    }

    #[test]
    fn detects_a_tag_opening() {
        let doc = doc_of("text #\n");
        let pos = Position { line: 0, character: 6 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(Prompt::of_completable(pos, &compl), Some(Prompt::Tag(String::new())));
    }

    #[test]
    fn a_complete_tag_wins_over_a_tag_opening() {
        let doc = doc_of("text #ta\n");
        let pos = Position { line: 0, character: 8 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(Prompt::of_completable(pos, &compl), Some(Prompt::Tag("ta".into())));
    }

    #[test]
    fn wiki_heading_beats_tag_opening_inside_a_wiki_link() {
        let doc = doc_of("[[#f\n");
        let pos = Position { line: 0, character: 4 };
        let compl = find_completable_at_pos(&doc, pos).unwrap();
        assert_eq!(
            Prompt::of_completable(pos, &compl),
            Some(Prompt::WikiHeadingInSrcDoc("f".into()))
        );
    }

    #[test]
    fn no_completion_on_plain_text() {
        let doc = doc_of("just prose here\n");
        assert!(find_completable_at_pos(&doc, Position { line: 0, character: 5 }).is_none());
    }

    #[test]
    fn no_completion_inside_a_heading() {
        let doc = doc_of("# Heading text\n");
        assert!(find_completable_at_pos(&doc, Position { line: 0, character: 5 }).is_none());
    }

    #[test]
    fn wiki_doc_candidates_are_offered() {
        let dir = temp_dir("wikidoc");
        write(&dir, "a.md", "# A\n\n[[\n");
        write(&dir, "my note.md", "# My Note\n");
        write(&dir, "other.md", "# Other\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 2, character: 2 });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"My Note"), "{labels:?}");
        assert!(labels.contains(&"Other"), "{labels:?}");
        assert!(!labels.contains(&"A"), "the source doc is not a candidate");

        let mine = items.iter().find(|i| i.label == "My Note").unwrap();
        assert_eq!(mine.kind, Some(CompletionItemKind::REFERENCE));
        assert_eq!(mine.detail.as_deref(), Some("my note.md"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wiki_style_controls_the_inserted_text() {
        let dir = temp_dir("wikistyle");
        write(&dir, "a.md", "# A\n\n[[\n");
        write(&dir, "sub/My Note.md", "# My Note\n");

        let by_title = load(&dir);
        let doc = doc_named(&by_title, "A");
        let items = find_candidates_in_doc(&by_title, &doc, Position { line: 2, character: 2 });
        let mine = items.iter().find(|i| i.label == "My Note").unwrap();
        assert_eq!(edit_text(mine), "[[my-note]]");

        let by_path = {
            let id = FolderId::of_uri(crate::paths::system_path_to_uri_string(
                dir.to_str().unwrap(),
            ));
            let config = Config {
                compl_wiki_style: Some(ComplWikiStyle::FilePathStem),
                ..Config::empty()
            };
            Folder::try_load(Some(config), "test", id).unwrap()
        };
        let doc = doc_named(&by_path, "A");
        let items = find_candidates_in_doc(&by_path, &doc, Position { line: 2, character: 2 });
        let mine = items.iter().find(|i| i.label == "My Note").unwrap();
        assert_eq!(edit_text(mine), "[[sub/My Note]]");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn filters_doc_candidates_by_the_typed_prefix() {
        let dir = temp_dir("filter");
        write(&dir, "a.md", "# A\n\n[[my\n");
        write(&dir, "my note.md", "# My Note\n");
        write(&dir, "other.md", "# Other\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 2, character: 4 });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["My Note"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn heading_candidates_exclude_titles_and_duplicates() {
        let dir = temp_dir("headings");
        write(&dir, "a.md", "# A\n\n[[#s\n");
        write(&dir, "a.md", "# A\n\n## Sub\n\n## Sub\n\n[[#s\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 6, character: 4 });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["Sub"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reference_link_candidates_come_from_local_definitions() {
        let dir = temp_dir("refcand");
        write(&dir, "a.md", "See [la\n\n[label one]: /one \"Title One\"\n[label two]: /two\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "a");

        // Both labels contain "la" as a subsequence.
        let items = find_candidates_in_doc(&folder, &doc, Position { line: 0, character: 7 });
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["label one", "label two"]);
        assert_eq!(items[0].detail.as_deref(), Some("Title One"));
        assert_eq!(items[0].documentation, Some(Documentation::String("/one".into())));
        // A partial reference link is completed with its brackets.
        assert_eq!(
            items[0].text_edit,
            Some(lsp_types::CompletionTextEdit::Edit(TextEdit {
                range: Range::new(Position::new(0, 4), Position::new(0, 7)),
                new_text: "[label one]".to_string(),
            }))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tag_candidates_report_usage_counts() {
        let dir = temp_dir("tagcand");
        write(&dir, "a.md", "# A\n\n#to\n");
        write(&dir, "b.md", "# B\n\n#todo and #todo\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 2, character: 3 });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "todo");
        assert_eq!(items[0].detail.as_deref(), Some("2 usages"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inline_doc_completion_inserts_an_encoded_path() {
        let dir = temp_dir("inlinedoc");
        write(&dir, "a.md", "# A\n\n[text](\n");
        write(&dir, "my note.md", "# My Note\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 2, character: 7 });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "my note.md");
        // A partially typed link is rewritten whole so the text is preserved.
        assert_eq!(edit_text(&items[0]), "[text](/my%20note.md)");
        assert_eq!(items[0].detail.as_deref(), Some("My Note"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_wiki_link_replaces_the_whole_element() {
        let dir = temp_dir("partial-range");
        write(&dir, "a.md", "# A\n\n[[my\n");
        write(&dir, "my note.md", "# My Note\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 2, character: 4 });
        assert_eq!(edit_range(&items[0]).start.character, 0);
        assert_eq!(edit_text(&items[0]), "[[my-note]]");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn complete_wiki_link_replaces_only_the_typed_part() {
        let dir = temp_dir("complete-range");
        write(&dir, "a.md", "# A\n\n[[my]]\n");
        write(&dir, "my note.md", "# My Note\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 2, character: 4 });
        assert_eq!(edit_range(&items[0]).start.character, 2);
        assert_eq!(edit_text(&items[0]), "my-note");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_anchor_completion_rewrites_the_whole_link() {
        let dir = temp_dir("anchor");
        write(&dir, "a.md", "# A\n\n## Some Heading\n\n[text](#so\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 4, character: 10 });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Some Heading");
        assert_eq!(edit_text(&items[0]), "[text](#some-heading)");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn complete_anchor_completion_replaces_only_the_anchor() {
        let dir = temp_dir("anchor-complete");
        write(&dir, "a.md", "# A\n\n## Some Heading\n\n[text](#so)\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 4, character: 10 });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Some Heading");
        assert_eq!(edit_text(&items[0]), "some-heading");
        assert_eq!(edit_range(&items[0]).start.character, 8);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anchor_completion_in_another_document() {
        let dir = temp_dir("anchor-other");
        write(&dir, "a.md", "# A\n\n[text](other.md#so\n");
        write(&dir, "other.md", "# Other\n\n## Some Heading\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let items = find_candidates_in_doc(&folder, &doc, Position { line: 2, character: 18 });
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "other.md / Some Heading");
        assert_eq!(edit_text(&items[0]), "[text](/other.md#some-heading)");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wiki_target_link_follows_the_configured_style() {
        let dir = temp_dir("target");
        write(&dir, "sub/My Note.md", "# My Note\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "My Note");

        assert_eq!(
            wiki_target_link(&Config::default_config(), &doc).encode().raw(),
            "my-note"
        );

        let stem = Config { compl_wiki_style: Some(ComplWikiStyle::FileStem), ..Config::empty() };
        assert_eq!(wiki_target_link(&stem, &doc).encode().raw(), "My Note");

        let path_stem = Config {
            compl_wiki_style: Some(ComplWikiStyle::FilePathStem),
            ..Config::empty()
        };
        assert_eq!(wiki_target_link(&path_stem, &doc).encode().raw(), "sub/My Note");
        std::fs::remove_dir_all(&dir).ok();
    }
}
