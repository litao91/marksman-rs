//! Semantic tokens for wiki links, reference links and tags.
//!
//! Port of `Marksman.Semato`.

use lsp_types::Range;

use crate::cst::{MdLink, WikiLink};
use crate::index::Index;
use crate::misc::cmp_range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenType {
    WikiLink,
    RefLink,
    Tag,
}

impl TokenType {
    pub fn to_lsp_name(self) -> &'static str {
        match self {
            TokenType::WikiLink => "class",
            TokenType::RefLink => "class",
            TokenType::Tag => "enumMember",
        }
    }

    pub fn to_num(self) -> u32 {
        match self {
            TokenType::WikiLink => 0,
            TokenType::RefLink => 1,
            TokenType::Tag => 2,
        }
    }

    pub fn mapping() -> Vec<&'static str> {
        [TokenType::WikiLink, TokenType::RefLink, TokenType::Tag]
            .iter()
            .map(|t| t.to_lsp_name())
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub range: Range,
    pub typ: TokenType,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}@{:?}", self.typ, self.range)
    }
}

/// Modifiers are not used in the current encoding.
const MODIFIERS: u32 = 0;

fn is_single_line(tok: &Token) -> bool {
    tok.range.start.line == tok.range.end.line
}

fn len_single_line(tok: &Token) -> u32 {
    tok.range.end.character - tok.range.start.character
}

fn delta_encode(prev_tok: Option<&Token>, cur_tok: &Token) -> [u32; 5] {
    debug_assert!(is_single_line(cur_tok));

    let (delta_line, delta_char) = match prev_tok {
        None => (cur_tok.range.start.line, cur_tok.range.start.character),
        Some(prev) => {
            let delta_line = cur_tok.range.start.line - prev.range.start.line;
            let delta_char = if delta_line == 0 {
                cur_tok.range.start.character - prev.range.start.character
            } else {
                cur_tok.range.start.character
            };
            (delta_line, delta_char)
        }
    };

    [delta_line, delta_char, len_single_line(cur_tok), cur_tok.typ.to_num(), MODIFIERS]
}

/// Delta-encodes tokens in the form LSP expects: five numbers per token,
/// relative to the previous token's start.
pub fn encode_all(tokens: impl IntoIterator<Item = Token>) -> Vec<u32> {
    let mut tokens: Vec<Token> = tokens
        .into_iter()
        // Multiline tokens are messy to support, so they are filtered out.
        .filter(is_single_line)
        .collect();
    tokens.sort_by(|a, b| cmp_range(&a.range, &b.range));

    let mut encoded = Vec::with_capacity(tokens.len() * 5);
    let mut prev: Option<&Token> = None;
    for cur in &tokens {
        encoded.extend_from_slice(&delta_encode(prev, cur));
        prev = Some(cur);
    }
    encoded
}

pub fn of_index(index: &Index) -> Vec<Token> {
    let mut tokens = Vec::new();

    for link in &index.wiki_links {
        if let Some(range) = WikiLink::content_range(&link.data) {
            tokens.push(Token { range, typ: TokenType::WikiLink });
        }
    }

    for link in &index.md_links {
        if let Some(label) = MdLink::reference_label(&link.data) {
            tokens.push(Token { range: label.range, typ: TokenType::RefLink });
        }
    }

    for tag in &index.tags {
        tokens.push(Token { range: tag.range, typ: TokenType::Tag });
    }

    tokens
}

pub fn is_in_range(range: &Range, token: &Token) -> bool {
    token.range.start >= range.start && token.range.end <= range.end
}

pub fn in_range(range: &Range, tokens: impl IntoIterator<Item = Token>) -> Vec<Token> {
    tokens.into_iter().filter(|t| is_in_range(range, t)).collect()
}

pub fn of_index_encoded(index: &Index) -> Vec<u32> {
    encode_all(of_index(index))
}

pub fn of_index_encoded_in_range(index: &Index, range: &Range) -> Vec<u32> {
    encode_all(in_range(range, of_index(index)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ParserSettings;
    use crate::parser::parse;
    use crate::structure::Structure;
    use crate::text::mk_text;

    fn index_of(content: &str) -> Index {
        let text = mk_text(content);
        let structure = parse(&ParserSettings::default(), &text);
        Index::of_cst(Structure::concrete_elements(&structure))
    }

    #[test]
    fn token_type_names_form_the_legend() {
        assert_eq!(TokenType::mapping(), vec!["class", "class", "enumMember"]);
        assert_eq!(TokenType::WikiLink.to_num(), 0);
        assert_eq!(TokenType::Tag.to_num(), 2);
    }

    #[test]
    fn collects_wiki_links_ref_links_and_tags() {
        let index = index_of("[[w]] [r][lab] #tag\n\n[lab]: /u\n");
        let tokens = of_index(&index);
        let kinds: Vec<TokenType> = tokens.iter().map(|t| t.typ).collect();
        assert!(kinds.contains(&TokenType::WikiLink));
        assert!(kinds.contains(&TokenType::RefLink));
        assert!(kinds.contains(&TokenType::Tag));
    }

    #[test]
    fn inline_links_produce_no_tokens() {
        let index = index_of("[t](dest.md)\n");
        assert!(of_index(&index).is_empty());
    }

    #[test]
    fn encoding_is_relative_and_sorted() {
        let index = index_of("[[a]] text [[b]]\n#t\n");
        let encoded = of_index_encoded(&index);
        assert_eq!(encoded.len() % 5, 0);

        // Tokens are emitted in document order with non-decreasing line deltas.
        let mut prev_line = 0u32;
        for chunk in encoded.chunks(5) {
            assert!(chunk[0] >= prev_line || prev_line == 0);
            prev_line = chunk[0];
        }
        assert_eq!(&encoded[0..5], &[0, 2, 1, 0, 0]);
    }

    #[test]
    fn multiline_tokens_are_dropped() {
        let tokens = vec![
            Token { range: Range::new(lsp_types::Position::new(0, 0), lsp_types::Position::new(1, 3)), typ: TokenType::Tag },
            Token { range: Range::new(lsp_types::Position::new(2, 0), lsp_types::Position::new(2, 3)), typ: TokenType::Tag },
        ];
        assert_eq!(encode_all(tokens).len(), 5);
    }

    #[test]
    fn range_filter_keeps_contained_tokens() {
        let index = index_of("[[a]]\n[[b]]\n");
        let range = Range::new(lsp_types::Position::new(0, 0), lsp_types::Position::new(0, 5));
        assert_eq!(of_index_encoded_in_range(&index, &range).len(), 5);
    }

    #[test]
    fn wiki_link_token_covers_only_the_content() {
        let index = index_of("[[note]]\n");
        let tokens = of_index(&index);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].range.start.character, 2);
        assert_eq!(tokens[0].range.end.character, 6);
    }
}
