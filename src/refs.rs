//! Reference resolution: what a link points at, and what points at a target.
//!
//! Port of `Marksman.Refs`.

use lsp_types::Range;

use crate::config::ComplWikiStyle;
use crate::cst::{self, Element, MdLinkDef, Node, Tag, UrlEncodedNode, WikiEncodedNode};
use crate::doc::Doc;
use crate::folder::Folder;
use crate::misc::{self, Slug};
use crate::names::{DocId, InternName};
use crate::syms::{Def, IntraRef, Ref as SymRef, Scope, Sym};

pub type InternNameNode = Node<InternName>;

impl InternNameNode {
    pub fn of_wiki_unchecked(src: &DocId, node: &WikiEncodedNode) -> InternNameNode {
        let name = InternName::mk_unchecked(src.clone(), node.data.decode());
        Node { text: node.text.clone(), range: node.range, data: name }
    }

    pub fn of_url_checked(
        configured_exts: &[String],
        src: &DocId,
        node: &UrlEncodedNode,
    ) -> Option<InternNameNode> {
        InternName::mk_checked(configured_exts, src.clone(), node.data.decode()).map(|name| Node {
            text: node.text.clone(),
            range: node.range,
            data: name,
        })
    }
}

/// How a completion or rename should spell a link to a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileLinkKind {
    FilePath,
    FileName,
    FileStem,
    Title,
}

/// Classifies how a reference name matches a destination document. The configured
/// wiki style decides whether a title match outranks a path match.
pub fn detect_file_link_kind(
    compl_style: ComplWikiStyle,
    src_doc: &DocId,
    name: &str,
    dest_doc: &Doc,
) -> FileLinkKind {
    let name_slug = Slug::of_string(name);
    let name_path = InternName::mk_unchecked(src_doc.clone(), name.to_string()).try_as_path();

    let link_kind_as_path = |doc: &Doc| -> Option<FileLinkKind> {
        name_path.as_ref().and_then(|path| {
            let rel_path = path.to_rel();
            let doc_path = doc.path_from_root();
            if doc_path == rel_path {
                Some(FileLinkKind::FilePath)
            } else if doc_path.filename_stem() == rel_path.filename_stem() {
                Some(FileLinkKind::FileStem)
            } else if doc_path.filename() == rel_path.filename() {
                Some(FileLinkKind::FileName)
            } else {
                None
            }
        })
    };

    let title_matches = dest_doc.slug() == name_slug;
    let path_kind = link_kind_as_path(dest_doc);

    match (compl_style, title_matches, path_kind) {
        (ComplWikiStyle::TitleSlug, true, _) => FileLinkKind::Title,
        (_, true, None) => FileLinkKind::Title,
        (_, _, Some(file_kind)) => file_kind,
        (_, _, None) => FileLinkKind::FileName,
    }
}

#[derive(Clone, Debug)]
pub struct FileLink {
    pub link: String,
    pub kind: FileLinkKind,
    pub doc: Doc,
}

impl FileLink {
    pub fn is_fuzzy_match_doc(name: &InternName, doc: &Doc) -> bool {
        let by_title = name.slug().is_substring(&doc.slug());

        let by_path = || match name.try_as_path() {
            Some(link_root_path) => {
                let link = misc::abs_path_url_encode(link_root_path.to_rel().to_system());
                let doc = misc::abs_path_url_encode(doc.path_from_root().to_system());
                doc.contains(&link)
            }
            None => false,
        };

        by_title || by_path()
    }

    pub fn filter_matching_docs(folder: &Folder, name: &InternName) -> Vec<FileLink> {
        let completion_style = folder.config_or_default().compl_wiki_style();

        folder
            .filter_docs_by_name(name)
            .into_iter()
            .map(|doc| {
                let kind = detect_file_link_kind(completion_style, name.src(), name.name(), &doc);
                FileLink { link: name.name().to_string(), kind, doc }
            })
            .collect()
    }

    pub fn filter_fuzzy_matching_docs(folder: &Folder, name: &InternName) -> Vec<Doc> {
        folder
            .docs()
            .into_iter()
            .filter(|doc| FileLink::is_fuzzy_match_doc(name, doc))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum DocLink {
    Explicit(FileLink),
    Implicit(Doc),
}

impl DocLink {
    pub fn doc(&self) -> &Doc {
        match self {
            DocLink::Explicit(fl) => &fl.doc,
            DocLink::Implicit(doc) => doc,
        }
    }
}

/// A resolved reference target.
#[derive(Clone, Debug)]
pub enum Dest {
    Doc(FileLink),
    Heading(DocLink, Node<cst::Heading>),
    LinkDef(Doc, Node<MdLinkDef>),
    Tag(Doc, Node<Tag>),
}

impl Dest {
    pub fn doc(&self) -> &Doc {
        match self {
            Dest::Doc(fl) => &fl.doc,
            Dest::LinkDef(doc, _) | Dest::Tag(doc, _) => doc,
            Dest::Heading(doc_link, _) => doc_link.doc(),
        }
    }

    /// The whole source span of the target element. Go-to-definition lands on
    /// the heading line rather than on the title text within it.
    pub fn range(&self) -> Range {
        match self {
            Dest::Doc(fl) => match fl.doc.title() {
                Some(t) => t.range,
                None => fl.doc.text().full_range(),
            },
            Dest::Heading(_, heading) => heading.range,
            Dest::LinkDef(_, link_def) => link_def.range,
            Dest::Tag(_, tag) => tag.range,
        }
    }

    pub fn scope(&self) -> Range {
        match self {
            Dest::Doc(fl) => fl.doc.text().full_range(),
            Dest::Heading(_, heading) => heading.data.scope(),
            Dest::LinkDef(_, link_def) => link_def.range,
            Dest::Tag(_, tag) => tag.range,
        }
    }

    pub fn uri(&self) -> &str {
        self.doc().uri()
    }

    pub fn location(&self) -> lsp_types::Location {
        lsp_types::Location { uri: crate::uri::parse(self.uri()), range: self.range() }
    }

    fn detect_file_link(
        compl_style: ComplWikiStyle,
        src_doc_id: &DocId,
        src_sym: &Sym,
        dest_doc: Doc,
    ) -> Option<FileLink> {
        match src_sym.as_ref() {
            Some(SymRef::CrossRef(r)) => Some(FileLink {
                link: r.doc().to_string(),
                kind: detect_file_link_kind(compl_style, src_doc_id, r.doc(), &dest_doc),
                doc: dest_doc,
            }),
            _ => None,
        }
    }

    fn detect_doc_link(
        compl_style: ComplWikiStyle,
        src_doc_id: &DocId,
        src_sym: &Sym,
        dest_doc: Doc,
    ) -> Option<DocLink> {
        match src_sym.as_ref() {
            Some(SymRef::CrossRef(r)) => Some(DocLink::Explicit(FileLink {
                link: r.doc().to_string(),
                kind: detect_file_link_kind(compl_style, src_doc_id, r.doc(), &dest_doc),
                doc: dest_doc,
            })),
            Some(SymRef::IntraRef(IntraRef::IntraSection(_))) => {
                Some(DocLink::Implicit(dest_doc))
            }
            _ => None,
        }
    }

    pub fn try_resolve_sym(folder: &Folder, doc: &Doc, src_sym: &Sym) -> Vec<Dest> {
        let compl_style = folder.config_or_default().compl_wiki_style();

        let scoped_sym = src_sym.scoped_to_doc(doc.id().clone());
        let dest_syms = folder.conn().resolve(&scoped_sym);

        let mut out = Vec::new();
        for (dest_scope, dest_sym) in dest_syms {
            let Scope::Doc(dest_doc_id) = dest_scope else { continue };
            let dest_doc = folder.find_doc_by_id(&dest_doc_id);

            match dest_sym {
                Sym::Def(Def::Doc) => {
                    if let Some(fl) =
                        Dest::detect_file_link(compl_style, doc.id(), src_sym, dest_doc)
                    {
                        out.push(Dest::Doc(fl));
                    }
                }
                Sym::Def(Def::Title(_)) | Sym::Def(Def::Header(..)) => {
                    let doc_link =
                        Dest::detect_doc_link(compl_style, doc.id(), src_sym, dest_doc.clone());
                    for node in dest_doc
                        .structure()
                        .find_concrete_for_symbol(&dest_sym)
                        .iter()
                        .filter_map(Element::as_heading)
                    {
                        if let Some(link) = &doc_link {
                            out.push(Dest::Heading(link.clone(), node.clone()));
                        }
                    }
                }
                Sym::Def(Def::LinkDef(_)) => {
                    for node in dest_doc
                        .structure()
                        .find_concrete_for_symbol(&dest_sym)
                        .iter()
                        .filter_map(Element::as_link_def)
                    {
                        out.push(Dest::LinkDef(dest_doc.clone(), node.clone()));
                    }
                }
                Sym::Ref(_) | Sym::Tag(_) => {}
            }
        }

        out
    }

    pub fn try_resolve_element(folder: &Folder, doc: &Doc, element: &Element) -> Vec<Dest> {
        match doc.structure().try_find_symbol_for_concrete(element) {
            None => Vec::new(),
            Some(sym) => Dest::try_resolve_sym(folder, doc, sym),
        }
    }

    fn find_tag_refs(
        include_decl: bool,
        folder: &Folder,
        src_doc_id: &DocId,
        src_el: &Element,
        tag: &crate::syms::Tag,
    ) -> Vec<(Doc, Element)> {
        let src_doc = folder.find_doc_by_id(src_doc_id);

        let refs: Vec<(Doc, Element)> = folder
            .conn()
            .resolve(&(Scope::Global, Sym::Tag(tag.clone())))
            .into_iter()
            .flat_map(|(scope, r)| {
                let Scope::Doc(dest_doc_id) = scope else { return Vec::new() };
                let dest_doc = folder.find_doc_by_id(&dest_doc_id);
                dest_doc
                    .structure()
                    .find_concrete_for_symbol(&r)
                    .into_iter()
                    .map(|el| (dest_doc.clone(), el))
                    .collect::<Vec<_>>()
            })
            .collect();

        let mut out: Vec<(Doc, Element)> = refs
            .into_iter()
            .filter(|(d, e)| d != &src_doc || e != src_el)
            .collect();
        if include_decl {
            out.insert(0, (src_doc, src_el.clone()));
        }
        out
    }

    fn find_def_refs(
        include_decl: bool,
        folder: &Folder,
        in_doc_id: &DocId,
        src_el: Option<&Element>,
        def: &Def,
    ) -> Vec<(Doc, Element)> {
        let in_doc = folder.find_doc_by_id(in_doc_id);

        let decls: Vec<Element> = if include_decl {
            match src_el {
                None => in_doc
                    .structure()
                    .find_concrete_for_symbol(&Sym::Def(def.clone()))
                    .into_iter()
                    .collect(),
                Some(src_el) => vec![src_el.clone()],
            }
        } else {
            Vec::new()
        };

        // Whenever 'find references' is invoked on a title we should also look
        // for references to the document itself.
        let (defs, filter): (Vec<Def>, Box<dyn Fn(&Sym) -> bool>) = match def {
            Def::LinkDef(_) | Def::Header(..) => (vec![def.clone()], Box::new(|_| true)),
            Def::Doc => {
                let headers: Vec<Def> = in_doc
                    .structure()
                    .symbols()
                    .iter()
                    .filter_map(Sym::as_def)
                    .filter(|d| d.is_header_or_title())
                    .cloned()
                    .collect();
                let mut defs = vec![Def::Doc];
                defs.extend(headers);
                (defs, Box::new(|sym: &Sym| sym.is_ref_with_explicit_doc()))
            }
            Def::Title(id) => {
                let headers: Vec<Def> = in_doc
                    .structure()
                    .symbols()
                    .iter()
                    .filter_map(Sym::as_def)
                    .filter(|d| d.is_header_or_title())
                    .cloned()
                    .collect();
                let mut defs = vec![Def::Doc];
                defs.extend(headers);
                // Required to pick up internal references to titles, e.g.
                //   # Title
                //   [[#title]]
                let wanted = Slug::of_string(id);
                (
                    defs,
                    Box::new(move |sym: &Sym| {
                        sym.is_ref_with_explicit_doc()
                            || sym
                                .as_ref()
                                .and_then(SymRef::try_section)
                                .is_some_and(|name| *name == wanted)
                    }),
                )
            }
        };
        let mut out = Vec::new();
        for decl in decls {
            out.push((in_doc.clone(), decl));
        }

        for d in &defs {
            let refs = folder
                .conn()
                .resolve(&(Scope::Doc(in_doc_id.clone()), Sym::Def(d.clone())));
            for (scope, r) in refs {
                if !filter(&r) {
                    continue;
                }
                let Scope::Doc(dest_doc_id) = scope else { continue };
                let dest_doc = folder.find_doc_by_id(&dest_doc_id);
                for el in dest_doc.structure().find_concrete_for_symbol(&r) {
                    out.push((dest_doc.clone(), el));
                }
            }
        }

        out
    }

    fn find_ref_refs(
        include_decl: bool,
        folder: &Folder,
        in_doc_id: &DocId,
        r: &SymRef,
    ) -> Vec<(Doc, Element)> {
        let defs: Vec<(DocId, Def)> = folder
            .conn()
            .resolve(&(Scope::Doc(in_doc_id.clone()), Sym::Ref(r.clone())))
            .into_iter()
            .filter_map(|(scope, sym)| match (scope, sym.as_def()) {
                (Scope::Doc(doc_id), Some(def)) => Some((doc_id, def.clone())),
                _ => None,
            })
            .collect();

        let mut out = Vec::new();
        for (doc, def) in defs {
            out.extend(Dest::find_def_refs(include_decl, folder, &doc, None, &def));
        }
        out
    }

    /// Finds elements referencing `el`. When `el` is a link, it is resolved to
    /// its destination first and then references to the destination are found.
    pub fn find_element_refs(
        include_decl: bool,
        folder: &Folder,
        src_doc: &Doc,
        src_el: &Element,
    ) -> Vec<(Doc, Element)> {
        let Some(sym) = src_doc.structure().try_find_symbol_for_concrete(src_el) else {
            return Vec::new();
        };
        let sym = sym.clone();

        let mut refs = match sym {
            Sym::Tag(tag) => {
                Dest::find_tag_refs(include_decl, folder, src_doc.id(), src_el, &tag)
            }
            Sym::Def(def) => {
                Dest::find_def_refs(include_decl, folder, src_doc.id(), Some(src_el), &def)
            }
            Sym::Ref(r) => Dest::find_ref_refs(include_decl, folder, src_doc.id(), &r),
        };

        refs.sort_by(|(d1, e1), (d2, e2)| {
            d1.id().cmp(d2.id()).then_with(|| misc::cmp_range(&e1.range(), &e2.range()))
        });
        refs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::FolderId;
    use crate::text::mk_text;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-refs-{name}"));
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
        let folder_id =
            FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        Folder::try_load(None, "test", folder_id).unwrap()
    }

    fn doc_named(folder: &Folder, name: &str) -> Doc {
        folder.docs().into_iter().find(|d| d.name() == name).unwrap()
    }

    #[test]
    fn resolves_a_wiki_link_to_its_target_document() {
        let dir = temp_dir("resolve");
        write(&dir, "a.md", "# A\n\n[[B]]\n");
        write(&dir, "b.md", "# B\n");
        let folder = load(&dir);

        let a = doc_named(&folder, "A");
        let link = a
            .index()
            .wiki_links
            .first()
            .map(|n| Element::WL(n.clone()))
            .unwrap();

        let dests = Dest::try_resolve_element(&folder, &a, &link);
        assert_eq!(dests.len(), 1);
        assert_eq!(dests[0].doc().name(), "B");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_incoming_references_to_a_document() {
        let dir = temp_dir("incoming");
        write(&dir, "a.md", "# A\n\n[[Target]]\n");
        write(&dir, "b.md", "# B\n\n[[Target]]\n");
        write(&dir, "target.md", "# Target\n");
        let folder = load(&dir);

        let target = doc_named(&folder, "Target");
        let heading = target
            .index()
            .titles
            .first()
            .map(|n| Element::H(n.clone()))
            .unwrap();

        let refs = Dest::find_element_refs(false, &folder, &target, &heading);
        assert_eq!(refs.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn include_decl_adds_the_declaration_itself() {
        let dir = temp_dir("decl");
        write(&dir, "a.md", "# A\n\n[[Target]]\n");
        write(&dir, "target.md", "# Target\n");
        let folder = load(&dir);

        let target = doc_named(&folder, "Target");
        let heading = target
            .index()
            .titles
            .first()
            .map(|n| Element::H(n.clone()))
            .unwrap();

        assert_eq!(Dest::find_element_refs(true, &folder, &target, &heading).len(), 2);
        assert_eq!(Dest::find_element_refs(false, &folder, &target, &heading).len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_references_to_a_tag() {
        let dir = temp_dir("tags");
        write(&dir, "a.md", "# A\n\n#todo and more\n");
        write(&dir, "b.md", "# B\n\n#todo\n");
        let folder = load(&dir);

        let a = doc_named(&folder, "A");
        let tag_el = a
            .index()
            .tags
            .first()
            .map(|n| Element::T(n.clone()))
            .unwrap();

        let refs = Dest::find_element_refs(true, &folder, &a, &tag_el);
        assert_eq!(refs.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_references_to_a_link_definition() {
        let dir = temp_dir("linkdef");
        write(&dir, "a.md", "See [ref] and [ref].\n\n[ref]: /url\n");
        let folder = load(&dir);

        let a = doc_named(&folder, "a");
        let def = a
            .index()
            .link_defs
            .first()
            .map(|n| Element::MLD(n.clone()))
            .unwrap();

        let refs = Dest::find_element_refs(true, &folder, &a, &def);
        assert_eq!(refs.len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unresolved_link_has_no_destinations() {
        let dir = temp_dir("broken");
        write(&dir, "a.md", "# A\n\n[[Missing]]\n");
        let folder = load(&dir);

        let a = doc_named(&folder, "A");
        let link = a
            .index()
            .wiki_links
            .first()
            .map(|n| Element::WL(n.clone()))
            .unwrap();
        assert!(Dest::try_resolve_element(&folder, &a, &link).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_link_kind_prefers_title_when_configured() {
        let dir = temp_dir("kind");
        write(&dir, "dir/My Note.md", "# My Note\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "My Note");
        let src = FolderId::of_uri("file:///x");
        let src_id = DocId::mk_rooted(&src, crate::paths::LocalPath::of_system("/x/a.md"));

        assert_eq!(
            detect_file_link_kind(ComplWikiStyle::TitleSlug, &src_id, "My Note", &doc),
            FileLinkKind::Title
        );
        assert_eq!(
            detect_file_link_kind(ComplWikiStyle::FileStem, &src_id, "dir/My Note.md", &doc),
            FileLinkKind::FilePath
        );
        assert_eq!(
            detect_file_link_kind(ComplWikiStyle::FileStem, &src_id, "My Note.md", &doc),
            FileLinkKind::FileStem
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fuzzy_matching_accepts_substring_titles_and_paths() {
        let dir = temp_dir("fuzzy");
        write(&dir, "dir/My Note.md", "# My Note\n");
        write(&dir, "other.md", "# Other\n");
        let folder = load(&dir);
        let src = folder.docs().into_iter().next().unwrap();

        let by_title = InternName::mk_unchecked(src.id().clone(), "note");
        assert_eq!(FileLink::filter_fuzzy_matching_docs(&folder, &by_title).len(), 1);

        let by_path = InternName::mk_unchecked(src.id().clone(), "dir/My");
        assert_eq!(FileLink::filter_fuzzy_matching_docs(&folder, &by_path).len(), 1);

        let no_match = InternName::mk_unchecked(src.id().clone(), "zzz");
        assert!(FileLink::filter_fuzzy_matching_docs(&folder, &no_match).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dest_range_falls_back_to_the_whole_document() {
        let doc = Doc::mk(
            &crate::config::ParserSettings::default(),
            DocId::mk_rooted(
                &FolderId::of_uri("file:///w"),
                crate::paths::LocalPath::of_system("/w/untitled.md"),
            ),
            Some(1),
            mk_text("no heading\n"),
        )
        .unwrap();
        let dest = Dest::Doc(FileLink {
            link: "untitled".into(),
            kind: FileLinkKind::Title,
            doc: doc.clone(),
        });
        assert_eq!(dest.range(), doc.text().full_range());
        assert_eq!(dest.uri(), doc.uri());
    }
}
