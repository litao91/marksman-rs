//! The connection graph: which symbols resolve to which definitions.
//!
//! Port of `Marksman.Conn`. Source symbols and derived computations form one
//! connection state; the reverse-reference index answers "who points at this
//! symbol". Computations record their dependencies so that a document change can
//! re-evaluate only what actually depends on it.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use log::trace;

use crate::misc::{indented, Difference};
use crate::mmap::{MMap, MMapDifference};
use crate::names::{DocumentAlias, DocId, InternName};
use crate::graph::{Graph, GraphDifference};
use crate::partitioned_map::PartitionedMap;
use crate::paths::RelPath;
use crate::syms::{CrossRef, Def, Ref as SymRef, Scope, ScopedSym, Sym};
use crate::misc::{LinkLabel, Slug};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DefinitionSelector {
    DocumentTarget(Scope),
    SectionTarget(Scope, String),
    LinkDefinitionTarget(Scope, LinkLabel),
}

impl DefinitionSelector {
    pub fn scope(&self) -> &Scope {
        match self {
            DefinitionSelector::DocumentTarget(scope)
            | DefinitionSelector::SectionTarget(scope, _)
            | DefinitionSelector::LinkDefinitionTarget(scope, _) => scope,
        }
    }

    /// `Def::Doc` is an input to document selection because it supplies the
    /// fallback when a document has no title.
    pub fn for_definition(scope: Scope, def: &Def) -> Vec<DefinitionSelector> {
        match def {
            Def::Doc => vec![DefinitionSelector::DocumentTarget(scope)],
            Def::Title(id) => vec![
                DefinitionSelector::DocumentTarget(scope.clone()),
                DefinitionSelector::SectionTarget(scope, id.clone()),
            ],
            Def::Header(_, id) => vec![DefinitionSelector::SectionTarget(scope, id.clone())],
            Def::LinkDef(label) => {
                vec![DefinitionSelector::LinkDefinitionTarget(scope, label.clone())]
            }
        }
    }

    pub fn reads_definition(&self, definition: &Def) -> bool {
        Self::for_definition(self.scope().clone(), definition).contains(self)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CandidateDocumentResolution {
    pub documents: BTreeSet<DocId>,
    pub aliases_read: BTreeSet<DocumentAlias>,
}

/// Evaluates computations against a folder snapshot. Folder supplies the real
/// implementation; tests supply fakes to exercise dependency tracking.
pub trait OracleApi {
    fn resolve_candidate_documents(&self, name: &InternName) -> CandidateDocumentResolution;
    fn select_definitions(&self, selector: &DefinitionSelector) -> Vec<Def>;
}

pub type Oracle = Arc<dyn OracleApi + Send + Sync>;

/// The document facts Conn needs to update its symbols and lookup dependencies.
#[derive(Clone, Debug)]
pub struct DocumentInput {
    pub id: DocId,
    pub slug: Slug,
    pub path: RelPath,
    pub symbols: BTreeSet<Sym>,
}

#[derive(Clone, Debug)]
pub enum DocumentChange {
    Added(DocumentInput),
    Removed(DocumentInput),
    Replaced(DocumentInput, DocumentInput),
}

#[derive(Clone, Debug, Default)]
pub struct ConnectionChange {
    pub symbol_difference: Difference<ScopedSym>,
    pub invalidated_document_aliases: BTreeSet<DocumentAlias>,
}

impl ConnectionChange {
    pub fn of_documents(markdown_extensions: &[String], changes: &[DocumentChange]) -> ConnectionChange {
        let aliases = |input: &DocumentInput| {
            DocumentAlias::of_document(markdown_extensions, &input.slug, &input.path)
        };

        let mut symbol_difference: Difference<ScopedSym> = Difference::empty();
        let mut invalidated_document_aliases: BTreeSet<DocumentAlias> = BTreeSet::new();

        for change in changes {
            let (before, after): (Option<&DocumentInput>, Option<&DocumentInput>) = match change {
                DocumentChange::Added(current) => (None, Some(current)),
                DocumentChange::Removed(previous) => (Some(previous), None),
                DocumentChange::Replaced(previous, current) => (Some(previous), Some(current)),
            };

            let same_id = matches!((before, after), (Some(p), Some(c)) if p.id == c.id);

            let symbol_change: Difference<ScopedSym> = match (before, after) {
                (Some(previous), Some(current)) if same_id => Difference {
                    added: current
                        .symbols
                        .difference(&previous.symbols)
                        .map(|s| s.scoped_to_doc(current.id.clone()))
                        .collect(),
                    removed: previous
                        .symbols
                        .difference(&current.symbols)
                        .map(|s| s.scoped_to_doc(previous.id.clone()))
                        .collect(),
                },
                _ => Difference {
                    added: after
                        .map(|input| input.symbols.iter().map(|s| s.scoped_to_doc(input.id.clone())).collect())
                        .unwrap_or_default(),
                    removed: before
                        .map(|input| input.symbols.iter().map(|s| s.scoped_to_doc(input.id.clone())).collect())
                        .unwrap_or_default(),
                },
            };

            symbol_difference.added.extend(symbol_change.added);
            symbol_difference.removed.extend(symbol_change.removed);

            let aliases_to_invalidate = match (before, after) {
                (Some(previous), Some(current))
                    if previous.id == current.id
                        && previous.slug == current.slug
                        && previous.path == current.path =>
                {
                    BTreeSet::new()
                }
                (Some(previous), Some(current)) if previous.id != current.id => {
                    let mut both = aliases(previous);
                    both.extend(aliases(current));
                    both
                }
                _ => {
                    let old_aliases: BTreeSet<DocumentAlias> =
                        before.map(aliases).unwrap_or_default();
                    let new_aliases: BTreeSet<DocumentAlias> =
                        after.map(aliases).unwrap_or_default();
                    let mut out: BTreeSet<DocumentAlias> =
                        old_aliases.difference(&new_aliases).cloned().collect();
                    out.extend(new_aliases.difference(&old_aliases).cloned());
                    out
                }
            };

            invalidated_document_aliases.extend(aliases_to_invalidate);
        }

        ConnectionChange { symbol_difference, invalidated_document_aliases }
    }

    pub fn is_empty(&self) -> bool {
        self.symbol_difference.is_empty() && self.invalidated_document_aliases.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnresolvedScope {
    FullyUnknown,
    InScope(Scope),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unresolved {
    Ref(Scope, SymRef),
    Scope(UnresolvedScope),
}

impl Unresolved {
    pub fn compact_format(&self) -> String {
        match self {
            Unresolved::Ref(scope, r) => format!("{r} @ {scope}"),
            Unresolved::Scope(UnresolvedScope::FullyUnknown) => "FullyUnknown".to_string(),
            Unresolved::Scope(UnresolvedScope::InScope(scope)) => format!("{scope}"),
        }
    }
}

impl std::fmt::Display for Unresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.compact_format())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConnectionComputation {
    ResolveCandidateDocuments(InternName),
    SelectDefinitions(DefinitionSelector),
    ResolveReference(Scope, SymRef),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConnectionDependency {
    ExternalInput(DocumentAlias),
    ComputedValue(ConnectionComputation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceResolution {
    pub resolved: BTreeSet<ScopedSym>,
    pub unresolved: BTreeSet<UnresolvedScope>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionValue {
    CandidateDocuments(BTreeSet<DocId>),
    SelectedDefinitions(BTreeSet<Def>),
    ReferenceResolution(ReferenceResolution),
}

fn resolution_graph(
    symbols: &MMap<Scope, Sym>,
    computed_values: &PartitionedMap<ConnectionComputation, ConnectionValue>,
) -> Graph<ScopedSym> {
    let mut graph = Graph::empty();

    computed_values.iter(|node, value| {
        if let (
            ConnectionComputation::ResolveReference(scope, r),
            ConnectionValue::ReferenceResolution(result),
        ) = (node, value)
        {
            for target in &result.resolved {
                graph = graph.add_edge((scope.clone(), Sym::Ref(r.clone())), target.clone());
            }
        }
    });

    for (scope, sym) in symbols.to_seq() {
        if matches!(sym, Sym::Tag(_)) {
            graph = graph.add_edge((scope.clone(), sym.clone()), (Scope::Global, sym));
        }
    }

    graph
}

fn unresolved_graph(
    computed_values: &PartitionedMap<ConnectionComputation, ConnectionValue>,
) -> Graph<Unresolved> {
    let mut graph = Graph::empty();
    computed_values.iter(|node, value| {
        if let (
            ConnectionComputation::ResolveReference(scope, r),
            ConnectionValue::ReferenceResolution(result),
        ) = (node, value)
        {
            for target in &result.unresolved {
                graph = graph.add_edge(
                    Unresolved::Ref(scope.clone(), r.clone()),
                    Unresolved::Scope(target.clone()),
                );
            }
        }
    });
    graph
}

/// Source symbols and derived computations form one connection state. The
/// reverse-reference map indexes resolved references and tag occurrences.
#[derive(Clone, Debug)]
pub struct Conn {
    pub symbols: MMap<Scope, Sym>,
    dependencies: MMap<ConnectionComputation, ConnectionDependency>,
    dependents: MMap<ConnectionDependency, ConnectionComputation>,
    computed_values: PartitionedMap<ConnectionComputation, ConnectionValue>,
    references_by_target: MMap<ScopedSym, ScopedSym>,
}

#[derive(Clone, Debug)]
pub struct ConnDifference {
    pub refs_difference: MMapDifference<Scope, SymRef>,
    pub defs_difference: MMapDifference<Scope, Def>,
    pub tags_difference: MMapDifference<Scope, crate::syms::Tag>,
    pub resolved_difference: GraphDifference<ScopedSym>,
    pub unresolved_difference: GraphDifference<Unresolved>,
    pub state_differences: Vec<String>,
}

impl ConnDifference {
    pub fn is_empty(&self) -> bool {
        self.refs_difference.is_empty()
            && self.defs_difference.is_empty()
            && self.tags_difference.is_empty()
            && self.resolved_difference.is_empty()
            && self.unresolved_difference.is_empty()
            && self.state_differences.is_empty()
    }

    pub fn compact_format(&self) -> String {
        let mut lines: Vec<String> = Vec::new();

        if !self.refs_difference.is_empty() {
            lines.push("Refs difference:".to_string());
            lines.push(indented(2, &self.refs_difference.compact_format()));
        }
        if !self.defs_difference.is_empty() {
            lines.push("Defs difference:".to_string());
            lines.push(indented(2, &self.defs_difference.compact_format()));
        }
        if !self.tags_difference.is_empty() {
            lines.push("Tags difference:".to_string());
            lines.push(indented(2, &self.tags_difference.compact_format()));
        }
        if !self.resolved_difference.is_empty() {
            lines.push("Resolved difference:".to_string());
            lines.push(indented(
                2,
                &self
                    .resolved_difference
                    .compact_format_with(|(src_scope, src_sym), (dst_scope, dst_sym)| {
                        format!("(({src_scope}, {src_sym}), ({dst_scope}, {dst_sym}))")
                    }),
            ));
        }
        if !self.unresolved_difference.is_empty() {
            lines.push("Unresolved difference:".to_string());
            lines.push(indented(
                2,
                &self
                    .unresolved_difference
                    .compact_format_with(|src, dst| format!("({src}, {dst})")),
            ));
        }

        for state in &self.state_differences {
            lines.push(format!("{state} differs"));
        }

        lines.join("\n")
    }
}

fn symbols_of<V, F>(conn: &Conn, choose: F) -> MMap<Scope, V>
where
    V: Ord + Clone,
    F: Fn(&Sym) -> Option<V>,
{
    MMap::of_seq(conn.symbols.to_seq().filter_map(|(scope, sym)| {
        choose(&sym).map(|value| (scope, value))
    }))
}

impl Conn {
    pub fn empty() -> Conn {
        Conn {
            symbols: MMap::empty(),
            dependencies: MMap::empty(),
            dependents: MMap::empty(),
            computed_values: PartitionedMap::empty(),
            references_by_target: MMap::empty(),
        }
    }

    fn computation_dependencies(&self, computation: &ConnectionComputation) -> BTreeSet<ConnectionDependency> {
        self.dependencies.try_find(computation).cloned().unwrap_or_default()
    }

    fn dependent_computations(&self, dependency: &ConnectionDependency) -> BTreeSet<ConnectionComputation> {
        self.dependents.try_find(dependency).cloned().unwrap_or_default()
    }

    fn try_reference_name(scope: &Scope, r: &SymRef) -> Option<InternName> {
        match (scope, r) {
            (Scope::Doc(src), SymRef::CrossRef(cross)) => {
                Some(InternName::mk_unchecked(src.clone(), cross.doc().to_string()))
            }
            _ => None,
        }
    }

    fn selector_for_reference(r: &SymRef, scope: Scope) -> DefinitionSelector {
        match r {
            SymRef::CrossRef(CrossRef::CrossDoc(_)) => DefinitionSelector::DocumentTarget(scope),
            SymRef::CrossRef(CrossRef::CrossSection(_, section))
            | SymRef::IntraRef(crate::syms::IntraRef::IntraSection(section)) => {
                DefinitionSelector::SectionTarget(scope, section.to_string())
            }
            SymRef::IntraRef(crate::syms::IntraRef::IntraLinkDef(label)) => {
                DefinitionSelector::LinkDefinitionTarget(scope, label.clone())
            }
        }
    }

    fn remove_computation(&mut self, computation: &ConnectionComputation) -> BTreeSet<ConnectionDependency> {
        let dependencies_of_computation = self.computation_dependencies(computation);
        let computed_value = ConnectionDependency::ComputedValue(computation.clone());
        let dependents_of_computation = self.dependent_computations(&computed_value);

        for dependency in &dependencies_of_computation {
            self.dependents.remove_value_mut(dependency, computation);
        }
        for dependent in &dependents_of_computation {
            self.dependencies.remove_value_mut(dependent, &computed_value);
        }

        self.dependencies.remove_key_mut(computation);
        self.dependents.remove_key_mut(&computed_value);
        self.computed_values.remove_mut(computation);

        dependencies_of_computation
    }

    /// `ResolveReference` computations are rooted at actual source symbols, not
    /// derived caches, so they are removed by `remove_symbol` and never
    /// garbage-collected here.
    fn is_collectable_computation(computation: &ConnectionComputation) -> bool {
        !matches!(computation, ConnectionComputation::ResolveReference(..))
    }

    fn collect_orphaned_computations(&mut self, dependencies: BTreeSet<ConnectionDependency>) {
        if dependencies.is_empty() {
            return;
        }

        // The input set deduplicates initial work. Collectable computations
        // currently have no computed-value inputs, so removing one cannot
        // enqueue another. The existence check below also makes repeated
        // entries safe if that changes.
        let mut pending: VecDeque<ConnectionComputation> = VecDeque::new();

        let enqueue = |deps: &BTreeSet<ConnectionDependency>, pending: &mut VecDeque<_>| {
            for dependency in deps {
                if let ConnectionDependency::ComputedValue(computation) = dependency {
                    pending.push_back(computation.clone());
                }
            }
        };
        enqueue(&dependencies, &mut pending);

        while let Some(computation) = pending.pop_front() {
            let dep = ConnectionDependency::ComputedValue(computation.clone());
            if Self::is_collectable_computation(&computation)
                && self.dependent_computations(&dep).is_empty()
                && self.computed_values.contains_key(&computation)
            {
                let inputs = self.remove_computation(&computation);
                enqueue(&inputs, &mut pending);
            }
        }
    }

    fn set_computation_dependencies(
        &mut self,
        computation: ConnectionComputation,
        dependencies: BTreeSet<ConnectionDependency>,
    ) {
        let old = self.computation_dependencies(&computation);
        if old == dependencies {
            return;
        }

        let removed: BTreeSet<ConnectionDependency> = old.difference(&dependencies).cloned().collect();
        let added: BTreeSet<ConnectionDependency> = dependencies.difference(&old).cloned().collect();

        for dependency in &removed {
            self.dependents.remove_value_mut(dependency, &computation);
        }
        for dependency in &added {
            self.dependents.add_mut(dependency.clone(), computation.clone());
        }

        self.dependencies.set_values_mut(computation, dependencies);

        self.collect_orphaned_computations(removed);
    }

    fn set_computation_value(&mut self, computation: ConnectionComputation, value: ConnectionValue) -> bool {
        self.computed_values.add_mut(computation, value)
    }

    fn evaluate_candidate_documents(
        &mut self,
        oracle: &Oracle,
        name: &InternName,
    ) -> (BTreeSet<DocId>, bool) {
        let node = ConnectionComputation::ResolveCandidateDocuments(name.clone());
        let resolution = oracle.resolve_candidate_documents(name);

        let dependencies = resolution
            .aliases_read
            .iter()
            .cloned()
            .map(ConnectionDependency::ExternalInput)
            .collect();

        self.set_computation_dependencies(node.clone(), dependencies);
        let changed = self.set_computation_value(
            node,
            ConnectionValue::CandidateDocuments(resolution.documents.clone()),
        );

        (resolution.documents, changed)
    }

    fn evaluate_definition_selection(
        &mut self,
        oracle: &Oracle,
        selector: &DefinitionSelector,
    ) -> (BTreeSet<Def>, bool) {
        let node = ConnectionComputation::SelectDefinitions(selector.clone());
        let scope = selector.scope();

        let definitions = if self.symbols.contains_key(scope) {
            oracle.select_definitions(selector).into_iter().collect()
        } else {
            BTreeSet::new()
        };

        let changed =
            self.set_computation_value(node, ConnectionValue::SelectedDefinitions(definitions.clone()));

        (definitions, changed)
    }

    fn ensure_candidate_documents(&mut self, oracle: &Oracle, name: &InternName) -> BTreeSet<DocId> {
        let node = ConnectionComputation::ResolveCandidateDocuments(name.clone());
        match self.computed_values.try_find(&node) {
            Some(ConnectionValue::CandidateDocuments(documents)) => documents.clone(),
            _ => self.evaluate_candidate_documents(oracle, name).0,
        }
    }

    fn ensure_selected_definitions(
        &mut self,
        oracle: &Oracle,
        selector: &DefinitionSelector,
    ) -> BTreeSet<Def> {
        let node = ConnectionComputation::SelectDefinitions(selector.clone());
        match self.computed_values.try_find(&node) {
            Some(ConnectionValue::SelectedDefinitions(definitions)) => definitions.clone(),
            _ => self.evaluate_definition_selection(oracle, selector).0,
        }
    }

    fn replace_reference_resolution(
        &mut self,
        source: (Scope, SymRef),
        resolution: ReferenceResolution,
    ) {
        let node = ConnectionComputation::ResolveReference(source.0.clone(), source.1.clone());
        let source_symbol = (source.0, Sym::Ref(source.1));

        let old_targets = match self.computed_values.try_find(&node) {
            Some(ConnectionValue::ReferenceResolution(old)) => old.resolved.clone(),
            _ => BTreeSet::new(),
        };

        for target in &old_targets {
            self.references_by_target.remove_value_mut(target, &source_symbol);
        }
        for target in &resolution.resolved {
            self.references_by_target.add_mut(target.clone(), source_symbol.clone());
        }

        self.computed_values
            .add_mut(node, ConnectionValue::ReferenceResolution(resolution));
    }

    fn evaluate_reference(&mut self, oracle: &Oracle, source: (Scope, SymRef)) {
        let (scope, ref sym_ref) = source;
        let mut dependencies: BTreeSet<ConnectionDependency> = BTreeSet::new();

        let scopes: BTreeSet<Scope> = match Self::try_reference_name(&scope, sym_ref) {
            None => BTreeSet::from([scope.clone()]),
            Some(name) => {
                let candidates_node =
                    ConnectionComputation::ResolveCandidateDocuments(name.clone());
                dependencies.insert(ConnectionDependency::ComputedValue(candidates_node));
                let docs = self.ensure_candidate_documents(oracle, &name);
                docs.into_iter().map(Scope::Doc).collect()
            }
        };

        let mut result = ReferenceResolution {
            resolved: BTreeSet::new(),
            unresolved: if scopes.is_empty() {
                BTreeSet::from([UnresolvedScope::FullyUnknown])
            } else {
                BTreeSet::new()
            },
        };

        for target_scope in scopes {
            let selector = Self::selector_for_reference(sym_ref, target_scope.clone());
            let selection = ConnectionComputation::SelectDefinitions(selector.clone());
            dependencies.insert(ConnectionDependency::ComputedValue(selection));
            let definitions = self.ensure_selected_definitions(oracle, &selector);

            if definitions.is_empty() {
                result.unresolved.insert(UnresolvedScope::InScope(target_scope.clone()));
            }

            for def in definitions {
                result.resolved.insert((target_scope.clone(), Sym::Def(def)));
            }
        }

        self.set_computation_dependencies(
            ConnectionComputation::ResolveReference(scope.clone(), sym_ref.clone()),
            dependencies,
        );
        self.replace_reference_resolution((scope, sym_ref.clone()), result);
    }

    fn remove_reference(&mut self, source: (Scope, SymRef)) {
        let node = ConnectionComputation::ResolveReference(source.0.clone(), source.1.clone());
        let source_symbol = (source.0, Sym::Ref(source.1));

        if let Some(ConnectionValue::ReferenceResolution(resolution)) = self.computed_values.try_find(&node) {
            let resolved = resolution.resolved.clone();
            for target in &resolved {
                self.references_by_target.remove_value_mut(target, &source_symbol);
            }
        }

        let dependencies = self.remove_computation(&node);
        self.collect_orphaned_computations(dependencies);
    }

    fn remove_symbol(&mut self, scope: Scope, sym: Sym) {
        match &sym {
            Sym::Ref(r) => self.remove_reference((scope.clone(), r.clone())),
            Sym::Tag(_) => {
                let global = (Scope::Global, sym.clone());
                let local = (scope.clone(), sym.clone());
                self.references_by_target.remove_value_mut(&global, &local);
            }
            Sym::Def(_) => {}
        }
        self.symbols.remove_value_mut(&scope, &sym);
    }

    fn add_symbol(&mut self, scope: Scope, sym: Sym) {
        if matches!(sym, Sym::Tag(_)) {
            self.references_by_target
                .add_mut((Scope::Global, sym.clone()), (scope.clone(), sym.clone()));
        }
        self.symbols.add_mut(scope, sym);
    }

    fn rebuild(&mut self, oracle: &Oracle, initial_work: BTreeSet<ConnectionComputation>) {
        let priority = |node: &ConnectionComputation| match node {
            ConnectionComputation::ResolveCandidateDocuments(_) => 0u8,
            ConnectionComputation::SelectDefinitions(_) => 1,
            ConnectionComputation::ResolveReference(..) => 2,
        };

        let mut work: BTreeSet<(u8, ConnectionComputation)> = BTreeSet::new();
        for node in initial_work {
            work.insert((priority(&node), node));
        }

        let mut evaluated_candidate_document_computations = 0usize;
        let mut evaluated_references = 0usize;

        // Resolve prerequisites before references. A changed value requeues its
        // dependents, and the ordered set suppresses duplicate work.
        while let Some(item) = work.iter().next().cloned() {
            work.remove(&item);
            let (_, node) = item;

            match &node {
                ConnectionComputation::ResolveCandidateDocuments(name)
                    if self.computed_values.contains_key(&node) =>
                {
                    let (_, changed) = self.evaluate_candidate_documents(oracle, name);
                    evaluated_candidate_document_computations += 1;
                    if changed {
                        let dependents = self
                            .dependent_computations(&ConnectionDependency::ComputedValue(node.clone()));
                        for dep in dependents {
                            work.insert((priority(&dep), dep));
                        }
                    }
                }
                ConnectionComputation::SelectDefinitions(selector)
                    if self.computed_values.contains_key(&node) =>
                {
                    let (_, changed) = self.evaluate_definition_selection(oracle, selector);
                    if changed {
                        let dependents = self
                            .dependent_computations(&ConnectionDependency::ComputedValue(node.clone()));
                        for dep in dependents {
                            work.insert((priority(&dep), dep));
                        }
                    }
                }
                ConnectionComputation::ResolveReference(scope, r)
                    if self
                        .symbols
                        .try_find(scope)
                        .is_some_and(|syms| syms.contains(&Sym::Ref(r.clone()))) =>
                {
                    self.evaluate_reference(oracle, (scope.clone(), r.clone()));
                    evaluated_references += 1;
                }
                _ => {}
            }
        }

        trace!(
            "Updated connection graph: #candidate_document_computations={evaluated_candidate_document_computations}, #references={evaluated_references}"
        );
    }

    /// Incrementally updates a shared graph, leaving the original untouched.
    pub fn update_incremental(previous: &Conn, oracle: &Oracle, change: ConnectionChange) -> Conn {
        previous.clone().update(oracle, change)
    }

    pub fn update(self, oracle: &Oracle, change: ConnectionChange) -> Conn {
        if change.is_empty() {
            return self;
        }

        let mut conn = self;

        for (scope, sym) in change.symbol_difference.removed.clone() {
            conn.remove_symbol(scope, sym);
        }
        for (scope, sym) in change.symbol_difference.added.clone() {
            conn.add_symbol(scope, sym);
        }

        let mut dirty_definition_computations: BTreeSet<ConnectionComputation> = BTreeSet::new();
        for (scope, sym) in change
            .symbol_difference
            .added
            .union(&change.symbol_difference.removed)
        {
            if let Some(def) = sym.as_def() {
                for selector in DefinitionSelector::for_definition(scope.clone(), def) {
                    dirty_definition_computations
                        .insert(ConnectionComputation::SelectDefinitions(selector));
                }
            }
        }

        let mut dirty_candidate_document_computations: BTreeSet<ConnectionComputation> =
            BTreeSet::new();
        for key in &change.invalidated_document_aliases {
            for dep in conn.dependent_computations(&ConnectionDependency::ExternalInput(key.clone())) {
                dirty_candidate_document_computations.insert(dep);
            }
        }

        let new_reference_computations: BTreeSet<ConnectionComputation> = change
            .symbol_difference
            .added
            .iter()
            .filter_map(crate::syms::as_scoped_ref)
            .map(|(scope, r)| ConnectionComputation::ResolveReference(scope, r))
            .collect();

        let mut initial = dirty_definition_computations;
        initial.extend(dirty_candidate_document_computations);
        initial.extend(new_reference_computations);

        conn.rebuild(oracle, initial);
        conn
    }

    pub fn mk(oracle: &Oracle, sym_map: &MMap<DocId, Sym>) -> Conn {
        let mut conn = Conn::empty();

        for (doc, sym) in sym_map.to_seq() {
            conn.add_symbol(Scope::Doc(doc), sym);
        }

        // Only references need resolving; definitions are just indexed.
        let sources: Vec<(Scope, SymRef)> = conn
            .symbols
            .to_seq()
            .filter_map(|(scope, sym)| sym.as_ref().map(|r| (scope, r.clone())))
            .collect();
        for source in sources {
            conn.evaluate_reference(oracle, source);
        }

        conn
    }

    pub fn difference(c1: &Conn, c2: &Conn) -> ConnDifference {
        let mut state_differences = Vec::new();
        if c1.dependencies != c2.dependencies {
            state_differences.push("Dependencies".to_string());
        }
        if c1.dependents != c2.dependents {
            state_differences.push("Dependents".to_string());
        }
        if c1.computed_values != c2.computed_values {
            state_differences.push("Computed values".to_string());
        }
        if c1.references_by_target != c2.references_by_target {
            state_differences.push("Reverse reference index".to_string());
        }

        ConnDifference {
            refs_difference: MMap::difference(
                &symbols_of(c1, Sym::as_ref_cloned),
                &symbols_of(c2, Sym::as_ref_cloned),
            ),
            defs_difference: MMap::difference(
                &symbols_of(c1, |s| s.as_def().cloned()),
                &symbols_of(c2, |s| s.as_def().cloned()),
            ),
            tags_difference: MMap::difference(
                &symbols_of(c1, |s| s.as_tag().cloned()),
                &symbols_of(c2, |s| s.as_tag().cloned()),
            ),
            resolved_difference: Graph::difference(
                &resolution_graph(&c1.symbols, &c1.computed_values),
                &resolution_graph(&c2.symbols, &c2.computed_values),
            ),
            unresolved_difference: Graph::difference(
                &unresolved_graph(&c1.computed_values),
                &unresolved_graph(&c2.computed_values),
            ),
            state_differences,
        }
    }

    fn source_contains(&self, scope: &Scope, sym: &Sym) -> bool {
        self.symbols.try_find(scope).is_some_and(|s| s.contains(sym))
    }

    /// Resolves a symbol to what it points at. References and tags are resolved
    /// from materialized results; definitions are answered from the reverse index.
    pub fn resolve(&self, scoped_sym: &ScopedSym) -> BTreeSet<ScopedSym> {
        let (scope, sym) = scoped_sym;
        match sym {
            Sym::Ref(r) if self.source_contains(scope, sym) => {
                let node = ConnectionComputation::ResolveReference(scope.clone(), r.clone());
                match self.computed_values.try_find(&node) {
                    Some(ConnectionValue::ReferenceResolution(result)) => result.resolved.clone(),
                    _ => BTreeSet::new(),
                }
            }
            Sym::Tag(_) if self.source_contains(scope, sym) => {
                BTreeSet::from([(Scope::Global, sym.clone())])
            }
            _ => self.references_by_target.try_find(scoped_sym).cloned().unwrap_or_default(),
        }
    }

    /// Compares materialized reference results without recalculating references.
    /// Unchanged partitions of the computed-value map need no traversal.
    pub fn documents_with_changed_reference_resolutions(before: &Conn, after: &Conn) -> BTreeSet<DocId> {
        let mut changed = BTreeSet::new();
        PartitionedMap::iter_differences(&before.computed_values, &after.computed_values, |node, _, _| {
            if let ConnectionComputation::ResolveReference(Scope::Doc(doc), _) = node {
                changed.insert(doc.clone());
            }
        });
        changed
    }

    pub fn symbols(&self) -> &MMap<Scope, Sym> {
        &self.symbols
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::FolderId;
    use crate::paths::{LocalPath, RootPath};
    use crate::syms::{CrossRef, IntraRef};
    use std::sync::Mutex;
    use std::collections::BTreeMap;

    /// A folder snapshot double: documents by id, with their symbols and titles.
    /// The map is mutable so a test can model a document appearing after the
    /// graph was built, which is what incremental updates rely on.
    struct FakeFolder {
        docs: Mutex<BTreeMap<DocId, BTreeSet<Sym>>>,
    }

    impl FakeFolder {
        fn doc_id(rel: &str) -> DocId {
            let folder = FolderId::of_uri("file:///w");
            DocId::mk_rooted(&folder, LocalPath::of_system(&format!("/w/{rel}")))
        }

        fn sym_map(&self) -> MMap<DocId, Sym> {
            let mut m = MMap::empty();
            for (doc, syms) in self.docs.lock().unwrap().iter() {
                for s in syms {
                    m = m.add(doc.clone(), s.clone());
                }
            }
            m
        }

        fn insert(&self, doc: DocId, syms: BTreeSet<Sym>) {
            self.docs.lock().unwrap().insert(doc, syms);
        }
    }

    impl OracleApi for FakeFolder {
        fn resolve_candidate_documents(&self, name: &InternName) -> CandidateDocumentResolution {
            let slug = name.slug();
            let mut documents = BTreeSet::new();
            for doc in self.docs.lock().unwrap().keys() {
                let stem = Slug::of_string(doc.rel_path_forced().filename_stem());
                if stem == slug {
                    documents.insert(doc.clone());
                }
            }
            let aliases_read = DocumentAlias::of_reference_name(&["md".into()], name);
            CandidateDocumentResolution { documents, aliases_read }
        }

        fn select_definitions(&self, selector: &DefinitionSelector) -> Vec<Def> {
            let scope = selector.scope();
            let Some(doc) = scope.as_doc() else { return Vec::new() };
            let docs = self.docs.lock().unwrap();
            let Some(syms) = docs.get(doc) else { return Vec::new() };

            syms.iter()
                .filter_map(Sym::as_def)
                .filter(|d| selector.reads_definition(d))
                .cloned()
                .collect()
        }
    }

    fn doc_syms(rel: &str, title: &str, extra: Vec<Sym>) -> (DocId, BTreeSet<Sym>) {
        let mut syms: BTreeSet<Sym> = BTreeSet::new();
        syms.insert(Sym::Def(Def::Doc));
        syms.insert(Sym::Def(Def::Title(Slug::of_string(title).to_string())));
        syms.extend(extra);
        (FakeFolder::doc_id(rel), syms)
    }

    #[test]
    fn cross_doc_reference_resolves_to_target_title() {
        let (a, a_syms) = doc_syms("a.md", "A", vec![Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc(
            "B".into(),
        )))]);
        let (b, b_syms) = doc_syms("b.md", "B", vec![]);

        let folder = Arc::new(FakeFolder {
            docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms), (b.clone(), b_syms)])),
        });
        let conn = Conn::mk(&(folder.clone() as Oracle), &folder.sym_map());

        let source = (
            Scope::Doc(a.clone()),
            Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc("B".into()))),
        );
        let resolved = conn.resolve(&source);
        assert!(resolved.contains(&(
            Scope::Doc(b.clone()),
            Sym::Def(Def::Title("b".into()))
        )), "{resolved:?}");
    }

    #[test]
    fn reverse_index_answers_who_points_at_a_document() {
        let (a, a_syms) = doc_syms("a.md", "A", vec![Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc(
            "B".into(),
        )))]);
        let (b, b_syms) = doc_syms("b.md", "B", vec![]);

        let folder = Arc::new(FakeFolder {
            docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms), (b.clone(), b_syms)])),
        });
        let conn = Conn::mk(&(folder.clone() as Oracle), &folder.sym_map());

        let target = (Scope::Doc(b.clone()), Sym::Def(Def::Title("b".into())));
        let incoming = conn.resolve(&target);
        assert_eq!(incoming.len(), 1);
    }

    #[test]
    fn unknown_reference_is_recorded_as_unresolved() {
        let (a, a_syms) = doc_syms(
            "a.md",
            "A",
            vec![Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc("Missing".into())))],
        );
        let folder = Arc::new(FakeFolder { docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms)])) });
        let conn = Conn::mk(&(folder.clone() as Oracle), &folder.sym_map());

        let graph = unresolved_graph(&conn.computed_values);
        assert!(!graph.is_empty());

        let source = (
            Scope::Doc(a),
            Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc("Missing".into()))),
        );
        assert!(conn.resolve(&source).is_empty());
    }

    #[test]
    fn intra_section_reference_resolves_within_the_same_document() {
        let (a, a_syms) = doc_syms(
            "a.md",
            "A",
            vec![
                Sym::Def(Def::Header(2, "sec".into())),
                Sym::Ref(SymRef::IntraRef(IntraRef::IntraSection(Slug::of_string("Sec")))),
            ],
        );
        let folder = Arc::new(FakeFolder { docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms)])) });
        let conn = Conn::mk(&(folder.clone() as Oracle), &folder.sym_map());

        let source = (
            Scope::Doc(a.clone()),
            Sym::Ref(SymRef::IntraRef(IntraRef::IntraSection(Slug::of_string("Sec")))),
        );
        let resolved = conn.resolve(&source);
        assert!(resolved.contains(&(Scope::Doc(a), Sym::Def(Def::Header(2, "sec".into())))));
    }

    #[test]
    fn tags_resolve_to_the_global_scope() {
        let (a, a_syms) =
            doc_syms("a.md", "A", vec![Sym::Tag(crate::syms::Tag("todo".into()))]);
        let folder = Arc::new(FakeFolder { docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms)])) });
        let conn = Conn::mk(&(folder.clone() as Oracle), &folder.sym_map());

        let tag = (Scope::Doc(a), Sym::Tag(crate::syms::Tag("todo".into())));
        let resolved = conn.resolve(&tag);
        assert_eq!(resolved, BTreeSet::from([(Scope::Global, Sym::Tag(crate::syms::Tag("todo".into())))]));
    }

    #[test]
    fn link_definition_reference_resolves_to_its_definition() {
        let label = LinkLabel::of_string("my ref");
        let (a, a_syms) = doc_syms(
            "a.md",
            "A",
            vec![
                Sym::Def(Def::LinkDef(label.clone())),
                Sym::Ref(SymRef::IntraRef(IntraRef::IntraLinkDef(label.clone()))),
            ],
        );
        let folder = Arc::new(FakeFolder { docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms)])) });
        let conn = Conn::mk(&(folder.clone() as Oracle), &folder.sym_map());

        let source = (
            Scope::Doc(a.clone()),
            Sym::Ref(SymRef::IntraRef(IntraRef::IntraLinkDef(label.clone()))),
        );
        assert!(conn
            .resolve(&source)
            .contains(&(Scope::Doc(a), Sym::Def(Def::LinkDef(label)))));
    }

    #[test]
    fn incremental_update_matches_a_full_rebuild() {
        let (a, a_syms) = doc_syms("a.md", "A", vec![Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc(
            "B".into(),
        )))]);
        let (b, b_syms) = doc_syms("b.md", "B", vec![]);

        let folder = Arc::new(FakeFolder {
            docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms.clone()), (b.clone(), b_syms.clone())])),
        });
        let oracle: Oracle = folder.clone();

        let base = Conn::mk(&oracle, &folder.sym_map());

        // Add a third document that A also points at, incrementally.
        let (c, c_syms) = doc_syms("c.md", "C", vec![]);
        folder.insert(c.clone(), c_syms.clone());
        let change = ConnectionChange::of_documents(
            &["md".into()],
            &[DocumentChange::Added(DocumentInput {
                id: c.clone(),
                slug: Slug::of_string("C"),
                path: RelPath("c.md".into()),
                symbols: c_syms,
            })],
        );
        let updated = base.clone().update(&oracle, change);

        let rebuilt = Conn::mk(&oracle, &folder.sym_map());

        let diff = Conn::difference(&updated, &rebuilt);
        assert!(
            diff.refs_difference.is_empty() && diff.defs_difference.is_empty(),
            "{}",
            diff.compact_format()
        );
    }

    #[test]
    fn empty_change_leaves_the_graph_untouched() {
        let (a, a_syms) = doc_syms("a.md", "A", vec![]);
        let folder = Arc::new(FakeFolder { docs: Mutex::new(BTreeMap::from([(a, a_syms)])) });
        let base = Conn::mk(&(folder.clone() as Oracle), &folder.sym_map());
        let updated = base.clone().update(&(folder as Oracle), ConnectionChange::default());
        assert!(Conn::difference(&base, &updated).resolved_difference.is_empty());
    }

    #[test]
    fn changed_reference_resolutions_are_attributed_to_their_document() {
        let (a, a_syms) = doc_syms("a.md", "A", vec![Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc(
            "B".into(),
        )))]);
        let folder = Arc::new(FakeFolder { docs: Mutex::new(BTreeMap::from([(a.clone(), a_syms)])) });
        let oracle: Oracle = folder.clone();

        let before = Conn::mk(&oracle, &folder.sym_map());

        let (b, b_syms) = doc_syms("b.md", "B", vec![]);
        folder.insert(b.clone(), b_syms.clone());
        let change = ConnectionChange::of_documents(
            &["md".into()],
            &[DocumentChange::Added(DocumentInput {
                id: b.clone(),
                slug: Slug::of_string("B"),
                path: RelPath("b.md".into()),
                symbols: b_syms,
            })],
        );
        let after = before.clone().update(&oracle, change);

        let changed = Conn::documents_with_changed_reference_resolutions(&before, &after);
        assert_eq!(changed.iter().collect::<Vec<_>>(), vec![&FakeFolder::doc_id("a.md")]);
    }

    #[test]
    fn definition_selector_for_definition_covers_title_fallback() {
        let scope = Scope::Global;
        assert_eq!(
            DefinitionSelector::for_definition(scope.clone(), &Def::Doc),
            vec![DefinitionSelector::DocumentTarget(scope.clone())]
        );
        let title = DefinitionSelector::for_definition(scope.clone(), &Def::Title("t".into()));
        assert_eq!(title.len(), 2);
        assert!(title.contains(&DefinitionSelector::DocumentTarget(scope.clone())));
        assert!(title.contains(&DefinitionSelector::SectionTarget(scope.clone(), "t".into())));
        assert_eq!(
            DefinitionSelector::for_definition(scope.clone(), &Def::Header(2, "h".into())),
            vec![DefinitionSelector::SectionTarget(scope, "h".into())]
        );
    }

    #[test]
    fn reads_definition_matches_what_for_definition_produces() {
        let folder = FolderId::of_uri("file:///w");
        let scope = Scope::Doc(DocId::mk_rooted(&folder, LocalPath::of_system("/w/a.md")));
        let selector = DefinitionSelector::DocumentTarget(scope.clone());
        assert!(selector.reads_definition(&Def::Doc));
        assert!(selector.reads_definition(&Def::Title("t".into())));
        assert!(!selector.reads_definition(&Def::Header(2, "h".into())));
        assert!(!selector.reads_definition(&Def::LinkDef(LinkLabel::of_string("l"))));
        let _ = RootPath::of_path(crate::paths::AbsPath::of_system("/w"));
    }

    #[test]
    fn difference_format_nests_each_section() {
        let diff = ConnDifference {
            refs_difference: MMapDifference {
                removed_keys: BTreeSet::new(),
                added_keys: BTreeSet::from([Scope::Global]),
                changed_keys: Vec::new(),
            },
            defs_difference: MMap::difference(
                &MMap::<Scope, Def>::empty(),
                &MMap::<Scope, Def>::empty(),
            ),
            tags_difference: MMap::difference(
                &MMap::<Scope, crate::syms::Tag>::empty(),
                &MMap::<Scope, crate::syms::Tag>::empty(),
            ),
            resolved_difference: Graph::difference(
                &Graph::<ScopedSym>::empty(),
                &Graph::<ScopedSym>::empty(),
            ),
            unresolved_difference: Graph::difference(
                &Graph::<Unresolved>::empty(),
                &Graph::<Unresolved>::empty(),
            ),
            state_differences: vec!["computedValues".to_string()],
        };

        // Empty sections are omitted; a non-empty one nests at two spaces, and
        // the key detail inside it at four more, exactly as the original's
        // nested `Indented` does.
        assert_eq!(
            diff.compact_format(),
            "Refs difference:\n  Added keys:\n      Global\ncomputedValues differs"
        );
    }
}
