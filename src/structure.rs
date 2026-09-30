//! The parsed form of a document: concrete tree, abstract tree, symbols and the
//! mappings between them.
//!
//! Port of `Marksman.Structure`.

use std::collections::BTreeSet;

use crate::ast::Ast;
use crate::config::ParserSettings;
use crate::cst::{self, Cst};
use crate::mapping::Mapping;
use crate::syms::{Def, Sym};

#[derive(Clone, Debug)]
pub struct Structure {
    cst: Cst,
    ast: Ast,
    sym: BTreeSet<Sym>,
    c2a: Mapping<cst::Element, crate::ast::Element>,
    a2s: Mapping<crate::ast::Element, Sym>,
}

impl Structure {
    pub fn cst(&self) -> &Cst {
        &self.cst
    }

    pub fn ast(&self) -> &Ast {
        &self.ast
    }

    pub fn symbols(&self) -> &BTreeSet<Sym> {
        &self.sym
    }

    pub fn abstract_elements(&self) -> &[crate::ast::Element] {
        &self.ast.elements
    }

    pub fn concrete_elements(&self) -> &[cst::Element] {
        &self.cst.elements
    }

    pub fn try_find_matching_abstract(&self, cel: &cst::Element) -> Option<&crate::ast::Element> {
        self.c2a.image(cel)
    }

    pub fn find_matching_abstract(&self, cel: &cst::Element) -> &crate::ast::Element {
        self.try_find_matching_abstract(cel).unwrap_or_else(|| {
            panic!("No matching abstract element for: {}", cst::Element::fmt(cel))
        })
    }

    pub fn try_find_concrete_for_abstract(
        &self,
        ael: &crate::ast::Element,
    ) -> Option<BTreeSet<cst::Element>> {
        self.c2a.try_pre_image(ael)
    }

    pub fn find_concrete_for_abstract(&self, ael: &crate::ast::Element) -> BTreeSet<cst::Element> {
        let cels = self.try_find_concrete_for_abstract(ael).unwrap_or_default();
        if cels.is_empty() {
            panic!("No matching concrete element for: {}", ael.compact_format());
        }
        cels
    }

    pub fn try_find_symbol_for_abstract(&self, ael: &crate::ast::Element) -> Option<&Sym> {
        self.a2s.image(ael)
    }

    pub fn try_find_symbol_for_concrete(&self, cel: &cst::Element) -> Option<&Sym> {
        let ael = self.try_find_matching_abstract(cel)?;
        self.try_find_symbol_for_abstract(ael)
    }

    pub fn find_abstract_for_symbol(&self, sym: &Sym) -> BTreeSet<crate::ast::Element> {
        self.a2s.pre_image(sym)
    }

    pub fn find_concrete_for_symbol(&self, sym: &Sym) -> BTreeSet<cst::Element> {
        let mut out = BTreeSet::new();
        for ael in self.find_abstract_for_symbol(sym) {
            out.extend(self.find_concrete_for_abstract(&ael));
        }
        out
    }

    pub fn of_cst(parser_settings: &ParserSettings, cst: Cst) -> Structure {
        let mut abs: Vec<crate::ast::Element> = Vec::new();
        let mut syms: BTreeSet<Sym> = BTreeSet::new();
        syms.insert(Sym::Def(Def::Doc));

        let mut c2a = Mapping::empty();
        let mut a2s = Mapping::empty();

        for cel in &cst.elements {
            if let Some(ael) = cel.to_abstract() {
                let sym = ael.to_sym(parser_settings);
                c2a.add_mut(cel.clone(), ael.clone());
                if let Some(sym) = sym {
                    syms.insert(sym.clone());
                    a2s.add_mut(ael.clone(), sym);
                }
                abs.push(ael);
            }
        }

        Structure {
            cst,
            ast: Ast { elements: abs },
            sym: syms,
            c2a,
            a2s,
        }
    }
}

pub fn concrete_elements(structure: &Structure) -> &[cst::Element] {
    structure.concrete_elements()
}

pub fn symbols(structure: &Structure) -> &BTreeSet<Sym> {
    structure.symbols()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cst::Element as CstElement;
    use crate::text::mk_text;

    fn structure_of(content: &str) -> Structure {
        crate::parser::parse(&ParserSettings::default(), &mk_text(content))
    }

    #[test]
    fn every_document_declares_itself() {
        let s = structure_of("plain text\n");
        assert!(s.symbols().contains(&Sym::Def(Def::Doc)));
    }

    #[test]
    fn concrete_and_abstract_elements_are_mapped() {
        let s = structure_of("# Title\n");
        let cel = &s.concrete_elements()[0];
        let ael = s.find_matching_abstract(cel);
        assert_eq!(ael.compact_format(), "# Title {title}");
        assert_eq!(
            s.try_find_symbol_for_abstract(ael),
            Some(&Sym::Def(Def::Title("title".into())))
        );
    }

    #[test]
    fn symbols_can_be_traced_back_to_source_elements() {
        let s = structure_of("[[note]]\n");
        let sym = Sym::Ref(crate::syms::Ref::CrossRef(crate::syms::CrossRef::CrossDoc("note".into())));
        let concretes = s.find_concrete_for_symbol(&sym);
        assert_eq!(concretes.len(), 1);
        assert!(matches!(concretes.iter().next().unwrap(), CstElement::WL(_)));
    }

    #[test]
    fn yaml_front_matter_contributes_no_abstract_element() {
        let s = structure_of("---\na: 1\n---\n\n# T\n");
        assert!(s.concrete_elements().iter().any(|e| matches!(e, CstElement::YML(_))));
        assert_eq!(s.abstract_elements().len(), 1);
    }
}
