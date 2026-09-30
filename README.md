# marksman-rs

A Rust port of [marksman](https://github.com/artempyanykh/marksman), the Markdown
language server. It speaks the same LSP protocol, reads the same
`.marksman.toml`, and produces the same completions, diagnostics, go-to-definition
targets, code lenses and rename edits as the original F# implementation.

The module layout mirrors the original file-for-file, so the two can be read side
by side:

| Rust | F# | Purpose |
| --- | --- | --- |
| `src/misc.rs` | `Misc.fs` | slugify, subsequence matching, URL/wiki encoding, link labels, set differences |
| `src/text.rs` | `Text.fs` | line map, document text, cursor/span/line views |
| `src/paths.rs` | `Paths.fs` | `AbsPath`/`RelPath`/`RootPath`/`RootedRelPath`, URI conversion |
| `src/names.rs` | `Names.fs` | `DocId`, `FolderId`, `InternName`, `DocumentAlias` |
| `src/syms.rs` | `Syms.fs` | the symbol model: `Def`, `Ref`, `Tag`, `Scope` |
| `src/cst.rs` | `Cst.fs` | concrete syntax tree nodes and elements |
| `src/ast.rs` | `Ast.fs` | abstract elements and their projection onto symbols |
| `src/parser.rs` | `Parser.fs` | Markdown → CST, heading disambiguation, scope computation |
| `src/index.rs` | `Index.fs` | per-document lookup tables |
| `src/structure.rs` | `Structure.fs` | CST + AST + symbols and the mappings between them |
| `src/mapping.rs` | `Mapping.fs` | a many-to-one map with an inverse index |
| `src/mmap.rs` | `MMap.fs` | ordered multi-map |
| `src/graph.rs` | `Graph.fs` | undirected graph |
| `src/partitioned_map.rs` | `PartitionedMap.fs` | copy-on-write partitioned map for snapshot diffing |
| `src/suffix_tree.rs` | `SuffixTree.fs` | reversed-path trie for suffix matching |
| `src/doc.rs` | `Doc.fs` | a document: identity, text, structure, index |
| `src/config.rs` | `Config.fs` | `.marksman.toml` parsing, layering, `ParserSettings` |
| `src/gitignore.rs` | `GitIgnore.fs` | ignore-file glob matching |
| `src/folder.rs` | `Folder.fs` | a workspace folder, its indexes and its oracle |
| `src/workspace.rs` | `Workspace.fs` | the set of folders plus user configuration |
| `src/conn.rs` | `Conn.fs` | the connection graph and its dependency tracking |
| `src/compl.rs` | `Compl.fs` | completion |
| `src/refs.rs` | `Refs.fs` | reference resolution and find-references |
| `src/symbols.rs` | `Symbols.fs` | document and workspace symbols |
| `src/toc.rs` | `Toc.fs` | table-of-contents generation and detection |
| `src/code_actions.rs` | `CodeActions.fs` | code actions |
| `src/refactor.rs` | `Refactor.fs` | rename |
| `src/diag.rs` | `Diag.fs` | diagnostics and incremental recomputation |
| `src/lenses.rs` | `Lenses.fs` | code lenses |
| `src/semato.rs` | `Semato.fs` | semantic tokens |
| `src/state.rs` | `State.fs` | client description and server state |
| `src/fatality.rs` | `Fatality.fs` | crash report for unrecoverable state failures |
| `src/server.rs` | `Server.fs` | capabilities, handlers, transport, background tasks |
| `src/main.rs` | `Program.fs` | CLI entry point |
| `src/uri.rs` | — | adapts internal URI strings to `lsp_types::Uri` |

## Building and running

```sh
cargo build --release        # binary at target/release/marksman
cargo test                   # unit + integration tests
```

The CLI matches the original:

```sh
marksman                     # start the LSP server on stdin/stdout, verbosity 2
marksman server -v 3         # same, with debug logging
marksman --version
```

Editor configuration is unchanged — point your client at the `marksman` binary
exactly as you would the original.

## Design notes

**Markdown parsing.** The original builds on Markdig with two custom inline
parsers. This port uses `pulldown-cmark` for block structure, links and
code/metadata spans, and hand-written scanners for wiki-links, tags and link
reference definitions. Scanning is restricted to the regions pulldown reports as
inline text, so code spans, code blocks, HTML, math and link destinations cannot
contribute elements. pulldown also reports source spans for link reference
definitions, which the original reads from Markdig's syntax tree.

**Positions.** LSP `Position.character` counts UTF-16 code units, matching the
.NET `char` indexing of the original. Internally everything is UTF-8 byte
offsets; `Text::range_of_offsets` converts between the two the way the
original's `sourceSpanToRange` does, deriving the end position from the last
*included* character so that an offset at end-of-document does not spill onto
the phantom line the line map keeps for appending.

**Immutability.** The original threads immutable values through every update,
relying on F#'s persistent collections for structural sharing. The port keeps
that discipline where it matters and mutates in place where it does not:
`Folder` shares its `data`, `lookup` and `conn` behind `Arc`, `PartitionedMap`
partitions are copy-on-write and a write that changes nothing leaves the
partition pointer alone, cached diagnostics are `Arc<Vec<Diagnostic>>` so a
document unaffected by an edit keeps the very same vector, and `Conn` mutates
through `&mut self`. Naively cloning a map per update makes graph construction
quadratic — an early version of this port did, and loading 1500 documents took
8.6 s until the maps became copy-on-write, after which it took 0.6 s.

**Concurrency.** The original serializes everything through F# `MailboxProcessor`
actors. This port runs on tokio: a `state_actor` task owns `State` and applies
mutations from a `StateHandle`, handing out `Arc<State>` snapshots for reads.
Read-only requests therefore run concurrently against an immutable snapshot
instead of queueing behind the actor, while document notifications are still
awaited in order. Diagnostics and status are separate tasks; the diagnostics
task keeps the original's 200 ms grace period so a burst of edits coalesces into
one publication. CPU-bound work runs on `spawn_blocking`.

## Known deviations

- **Glob library.** `globset` accepts patterns the .NET `GlobExpressions` library
  rejected. The original's `issue_218` test asserted a false negative that came
  from that rejection; here the pattern works.
- **Slugify.** `.NET` classifies characters with `Char.IsPunctuation` /
  `Char.IsSymbol`; the port uses `char::is_alphanumeric`. They agree on every
  ASCII and CJK input that reaches a heading and differ only for unassigned and
  control characters.
- **URI encoding.** Internal URI strings follow the original exactly, including
  leaving non-Latin-1 characters unescaped. `lsp_types::Uri` requires valid
  URIs, so outbound non-ASCII is percent-encoded at the protocol boundary and
  inbound URIs carrying raw spaces or non-ASCII are normalized rather than
  rejected.
- **Setext headings inside lists.** For `A\n\n-\n-` Markdig reads the two `-`
  lines as a setext H2; pulldown-cmark reads them as two empty list items, which
  is what CommonMark specifies. The ported regression test `no156` is ignored for
  this reason. Where pulldown does report a setext heading the port matches
  Markdig exactly, including the underline appearing in both the element's text
  and its title.
- **Multi-line shortcut-reference labels.** Markdig's `LabelSpan` for a label
  spanning several lines starts on the wrong line; `Parser.fs` carries a `TODO`
  about it. The port reports the label's real span instead, so regression test
  `no235` is ignored.
- **`#` in a file name.** The original unescapes the whole URI before handing it
  to `System.Uri`, so a `%23` becomes a fragment separator and everything after
  it is dropped from the path. The port reproduces this, which is why the
  original's `pathFromRoot_SpecialChars` test is skipped there and ignored here,
  and why a wiki link to `blah#blah.md` resolves to nothing in both.
- **Duplicate link-reference labels.** Only the first binds, which is what
  CommonMark requires and what pulldown's definition map reflects.
- **`Refs.simplifyDest` for link definitions.** The original formats the label
  through `MdLinkDef.label`, which returns a node record rather than its text, so
  F# prints a type name there. The port prints the label text. No test reaches
  that branch in either implementation.
- **Fatal errors.** The original catches exceptions; the port catches panics. A
  panic in a state mutation or in `initialize` prints the same crash report and
  exits with status 1, matching `Fatality.abort`.

## Tests

Every case in the original's `Tests/*.fs` is ported, one Rust file per F# test
module, with `tests/common/mod.rs` mirroring `Tests/Helpers.fs` so the ported
cases build their fixtures the same way. Where the original asserts against a
Snapper snapshot in `Tests/_snapshots/*.json`, the expected strings are copied
from those files rather than regenerated.

```
cargo test
```

653 tests: 276 unit tests beside the implementation and 377 integration tests
under `tests/`. Five are ignored — three because the original skips them too
(`footnote_1` and `ref_to_footnote_at_link`, "footnote parsing not implemented",
and `path_from_root_special_chars`, "`Uri` and `#` don't mix well"), and the two
parser cases listed under known deviations. Four further gitignore cases are
compiled only on Windows, as in the original.

`conn_dependency_tests` dominates the runtime at roughly 45 s in a debug build;
it replays randomized edit sequences of 1000 documents against a full graph
rebuild. The port reproduces `System.Random` exactly, so the seeds used by the
original produce the same sequences.

## Benchmarks

Measured with `bench/bench.py` against the original's release build
(.NET 9, `dotnet build -c Release`) on the same machine, using a synthetic vault
generated by `bench/gen_corpus.py`. Each server is driven over stdio by the same
client: 20 documents are opened after load and request figures are the median of
5 probes. Lower is better; the last column is the original's time divided by this
port's.

1500 documents, 1.18 MB of Markdown:

| metric | marksman-rs | marksman (F#) | F#/rs |
| --- | --- | --- | --- |
| cold start (`initialize`) | 891 ms | 4686 ms | 5.3x |
| first diagnostics settled | 693 ms | 726 ms | 1.0x |
| completion | 5.6 ms | 8.3 ms | 1.5x |
| definition | 1.0 ms | 1.8 ms | 1.8x |
| references | 0.8 ms | 2.1 ms | 2.6x |
| hover | 0.8 ms | 1.6 ms | 1.9x |
| documentSymbol | 0.7 ms | 2.1 ms | 2.8x |
| codeLens | 0.9 ms | 2.2 ms | 2.4x |
| semanticTokens/full | 0.8 ms | 1.3 ms | 1.7x |
| workspace/symbol | 9.2 ms | 47.8 ms | 5.2x |
| edit → diagnostics | 853 ms | 1867 ms | 2.2x |

5000 documents, 3.94 MB of Markdown, same procedure:

| metric | marksman-rs | marksman (F#) | F#/rs |
| --- | --- | --- | --- |
| cold start (`initialize`) | 3879 ms | 10347 ms | 2.7x |
| first diagnostics settled | 1828 ms | 1927 ms | 1.1x |
| completion | 11.1 ms | 12.3 ms | 1.1x |
| definition | 0.8 ms | 1.1 ms | 1.4x |
| references | 0.6 ms | 1.0 ms | 1.7x |
| hover | 0.6 ms | 0.8 ms | 1.3x |
| documentSymbol | 0.6 ms | 1.3 ms | 2.2x |
| codeLens | 0.7 ms | 1.2 ms | 1.7x |
| semanticTokens/full | 0.6 ms | 1.0 ms | 1.7x |
| workspace/symbol | 18.5 ms | 57.1 ms | 3.1x |
| edit → diagnostics | 3984 ms | not measured | — |

At 5000 documents the harness measures `edit → diagnostics` by waiting for a
quiet window, and it never observes the original's post-edit publication there —
not with a 2 s, 30 s or 90 s window. The direct comparison in the next section
shows the original does publish, and publishes the same thing.

Fixed process startup is not the explanation for the cold-start gap:
`marksman --version` takes 0.08 s for the original and under 0.01 s for this
port.

Internal phase timings for the 1500-document corpus
(`cargo run --release --example bench_load -- /tmp/bench-1500 5`, each phase its
own best of 5):

| phase | time |
| --- | --- |
| read + parse | 260 ms |
| lookup indexes + connection graph | 306 ms |
| total load | 610 ms |
| one-document edit (full graph rebuild) | 6.9 ms |

Throughput: 4.5 MB/s parsed, 2458 documents/s loaded.

Note that a full connection-graph rebuild per document edit is the original's
behaviour too: `core.incremental_references` defaults to `false`, so both
implementations rebuild on every change.

## Behavioural parity

The benchmark harness cross-checks observable output, not just timings. On the
5000-document corpus both servers published diagnostics for the same 100
documents with the same 100 diagnostics. Driving each server directly through the
same edit of a document that was never opened — appending a second H1 and a link
to a document that does not exist — both published the same 7 notifications, with
the same messages in the same order:

```
Note%2004104.md  Ambiguous link to document 'sub4/Note 00004'
Note%2004241.md  Ambiguous link to document 'Note 00004'
Note%2000107.md  Ambiguous link to document 'sub4/Note 00004'
Note%2000004.md  Link to non-existent document 'Bench Appended'
Note%2000572.md  Ambiguous link to document 'Note 00004'
Note%2002228.md  Ambiguous link to document 'sub4/Note 00004'
Note%2004669.md  Ambiguous link to document 'Note 00004'
```

The ambiguity cascade is the non-obvious part: adding a second H1 to a note makes
every path-style link to that note ambiguous, because `selectDefinitions` returns
all of a document's titles.

`bench/lsp_smoke.py` runs the same comparison on the small hand-written vault in
`bench/fixtures` and prints both servers' advertised capabilities and feature
responses side by side. Its output differs in two cosmetic places: `lsp-types`
serializes an absent file-operation scheme as `"scheme": null` where the original
omits the key, and a `Position` round-trips through JSON with its keys in the
other order. Every value is the same.

## Repository layout

```
src/            the port, one module per F# source file
tests/          ported test suites, one file per F# test module
tests/common/   shared fixtures, mirroring Tests/Helpers.fs
examples/       bench_load.rs — phase timings for loading a folder
bench/          gen_corpus.py, bench.py, lsp_smoke.py
```
