//! Table of contents generation and detection.
//!
//! Port of `Marksman.Toc`.

use lsp_types::Range;

use crate::cst::Heading;
use crate::doc::Doc;
use crate::index::Index;
use crate::misc::{self, Slug};
use crate::text::{Text, DOCUMENT_BEGINNING};

pub const START_MARKER: &str = "<!--toc:start-->";
pub const END_MARKER: &str = "<!--toc:end-->";

pub type Title = String;
pub type EntryLevel = i32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub level: EntryLevel,
    pub title: Title,
    pub link: Slug,
}

impl Entry {
    pub fn mk(level: EntryLevel, title: Title) -> Entry {
        let link = Slug::of_string(&title);
        Entry { level, title, link }
    }

    pub fn render_link(&self, min_level: EntryLevel) -> String {
        let offset = "  ".repeat((self.level - min_level).max(0) as usize);
        format!("{offset}- [{}](#{})", self.title, self.link.as_str())
    }

    pub fn from_heading(heading: &Heading) -> Entry {
        Entry {
            level: heading.level,
            link: heading.slug(),
            title: heading.title.text.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertionPoint {
    After(Range),
    Replacing(Range),
    DocumentBeginning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableOfContents {
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DetectState {
    BeforeMarker,
    Collecting(Range),
    Collected(Range),
}

impl TableOfContents {
    pub fn mk(include_levels: &[i32], index: &Index) -> Option<TableOfContents> {
        let headings: Vec<Entry> = index
            .headings
            .iter()
            .map(|x| &x.data)
            .filter(|h| include_levels.contains(&h.level))
            .map(Entry::from_heading)
            .collect();

        if index.headings.is_empty() {
            None
        } else {
            Some(TableOfContents { entries: headings })
        }
    }

    pub fn insertion_point(doc: &Doc) -> InsertionPoint {
        let index = doc.index();

        match index.titles.as_slice() {
            [single_title] => InsertionPoint::After(single_title.range),
            _ => match &index.yaml_front_matter {
                None => InsertionPoint::DocumentBeginning,
                Some(yml) => InsertionPoint::After(yml.range),
            },
        }
    }

    pub fn render(&self) -> String {
        let offset = match self.entries.iter().min_by_key(|x| x.level) {
            None => 1,
            Some(min) => min.level,
        };

        let toc_links: Vec<String> =
            self.entries.iter().map(|x| x.render_link(offset)).collect();

        let mut lines = vec![START_MARKER.to_string()];
        lines.extend(toc_links);
        lines.push(END_MARKER.to_string());

        lines.join("\n")
    }

    /// Locates an existing marked table of contents, inclusive of both markers.
    pub fn detect(text: &Text) -> Option<Range> {
        let max_index = text.line_map.num_lines();
        let mut state = DetectState::BeforeMarker;

        for i in 0..max_index {
            if i == max_index {
                break;
            }
            let line_range = text.line_content_range(i);
            let line_content = text.line_content(i);
            let is_start_marker = line_content.trim() == START_MARKER;
            let is_end_marker = line_content.trim() == END_MARKER;

            state = match state {
                DetectState::BeforeMarker => {
                    if is_start_marker {
                        DetectState::Collecting(line_range)
                    } else {
                        DetectState::BeforeMarker
                    }
                }
                DetectState::Collecting(range) => {
                    let to_this_line = Range { end: line_range.end, ..range };
                    if is_end_marker {
                        DetectState::Collected(to_this_line)
                    } else {
                        DetectState::Collecting(to_this_line)
                    }
                }
                collected @ DetectState::Collected(_) => collected,
            };

            if matches!(state, DetectState::Collected(_)) {
                break;
            }
        }

        match state {
            DetectState::Collected(range) => Some(range),
            DetectState::BeforeMarker => None,
            other => {
                log::warn!("TOC detection failed - end marker was not found: finalState={other:?}");
                None
            }
        }
    }

    /// Compares rendered tables of contents line by line so that mixed and
    /// platform-specific line endings do not cause spurious updates.
    pub fn is_same(toc_a: &str, toc_b: &str) -> bool {
        let content_a: Vec<&str> = misc::lines_of(
            misc::trim_both(toc_a, START_MARKER, END_MARKER).trim(),
        );
        let content_b: Vec<&str> = misc::lines_of(
            misc::trim_both(toc_b, START_MARKER, END_MARKER).trim(),
        );

        content_a.len() == content_b.len() && content_a == content_b
    }

    pub fn document_beginning() -> Range {
        DOCUMENT_BEGINNING
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ParserSettings;
    use crate::doc::Doc;
    use crate::names::{DocId, FolderId};
    use crate::paths::LocalPath;
    use crate::text::mk_text;

    fn doc_of(content: &str) -> Doc {
        Doc::mk(
            &ParserSettings::default(),
            DocId::mk_rooted(&FolderId::of_uri("file:///w"), LocalPath::of_system("/w/a.md")),
            Some(1),
            mk_text(content),
        )
        .unwrap()
    }

    fn toc_of(content: &str, levels: &[i32]) -> TableOfContents {
        let doc = doc_of(content);
        TableOfContents::mk(levels, doc.index()).unwrap()
    }

    const ALL_LEVELS: [i32; 6] = [1, 2, 3, 4, 5, 6];

    #[test]
    fn no_headings_means_no_table_of_contents() {
        let doc = doc_of("just text\n");
        assert!(TableOfContents::mk(&ALL_LEVELS, doc.index()).is_none());
    }

    #[test]
    fn renders_links_relative_to_the_shallowest_level() {
        let toc = toc_of("# T1\n## T2\n### T3\n## T4\n### T5\n", &ALL_LEVELS);
        assert_eq!(
            toc.render(),
            "<!--toc:start-->\n- [T1](#t1)\n  - [T2](#t2)\n    - [T3](#t3)\n  - [T4](#t4)\n    - [T5](#t5)\n<!--toc:end-->"
        );
    }

    #[test]
    fn deeper_starting_level_is_not_indented() {
        let toc = toc_of("## T1\n## T2\n### T3\n#### T4\n", &ALL_LEVELS);
        assert_eq!(
            toc.render(),
            "<!--toc:start-->\n- [T1](#t1)\n- [T2](#t2)\n  - [T3](#t3)\n    - [T4](#t4)\n<!--toc:end-->"
        );
    }

    #[test]
    fn included_levels_filter_entries_but_keep_their_depth() {
        // Indentation is relative to the shallowest included level, so a gap in
        // the included levels still shows up as indentation.
        let toc = toc_of("# T1\n## T2\n### T3\n", &[1, 3]);
        assert_eq!(
            toc.render(),
            "<!--toc:start-->\n- [T1](#t1)\n    - [T3](#t3)\n<!--toc:end-->"
        );
    }

    #[test]
    fn duplicate_headings_get_disambiguated_anchors() {
        let toc = toc_of("## T1\n## T1\n", &ALL_LEVELS);
        assert_eq!(
            toc.render(),
            "<!--toc:start-->\n- [T1](#t1)\n- [T1](#t1-1)\n<!--toc:end-->"
        );
    }

    #[test]
    fn titles_with_punctuation_get_slugified_anchors() {
        let toc = toc_of("# Hello, World!\n", &ALL_LEVELS);
        assert_eq!(
            toc.render(),
            "<!--toc:start-->\n- [Hello, World!](#hello-world)\n<!--toc:end-->"
        );
    }

    #[test]
    fn insertion_point_follows_a_single_title() {
        let doc = doc_of("# T1\n\n## T2\n");
        match TableOfContents::insertion_point(&doc) {
            InsertionPoint::After(range) => {
                assert_eq!(range.start.line, 0);
                assert_eq!(range.end.line, 0);
            }
            other => panic!("expected After, got {other:?}"),
        }
    }

    #[test]
    fn insertion_point_is_document_beginning_without_a_title() {
        let doc = doc_of("## T1\n## T2\n");
        assert_eq!(TableOfContents::insertion_point(&doc), InsertionPoint::DocumentBeginning);
    }

    #[test]
    fn insertion_point_follows_front_matter_when_there_is_no_title() {
        let doc = doc_of("---\na: 1\n---\n\n## T1\n## T2\n");
        match TableOfContents::insertion_point(&doc) {
            InsertionPoint::After(range) => assert_eq!(range.start.line, 0),
            other => panic!("expected After front matter, got {other:?}"),
        }
    }

    #[test]
    fn detects_a_marked_block_including_its_markers() {
        let text = mk_text("intro\n\n<!--toc:start-->\n- [A](#a)\n<!--toc:end-->\n\n# A\n");
        let range = TableOfContents::detect(&text).unwrap();
        assert_eq!(range.start.line, 2);
        assert_eq!(range.end.line, 4);
    }

    #[test]
    fn detection_finds_nothing_without_markers() {
        let text = mk_text("# A\n\n- [A](#a)\n");
        assert_eq!(TableOfContents::detect(&text), None);
    }

    #[test]
    fn detection_needs_both_markers() {
        let text = mk_text("<!--toc:start-->\n- [A](#a)\n");
        assert_eq!(TableOfContents::detect(&text), None);
    }

    #[test]
    fn comparison_ignores_markers_and_line_endings() {
        assert!(TableOfContents::is_same(
            "<!--toc:start-->\n- [A](#a)\n<!--toc:end-->",
            "<!--toc:start-->\r\n- [A](#a)\r\n<!--toc:end-->"
        ));
        assert!(!TableOfContents::is_same(
            "<!--toc:start-->\n- [A](#a)\n<!--toc:end-->",
            "<!--toc:start-->\n- [B](#b)\n<!--toc:end-->"
        ));
    }
}
