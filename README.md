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
| `src/markdown.rs` | *(Markdig)* | block and inline parser: headings, links, definitions, exclusions |
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
parsers for wiki links and tags. This port has no third-party Markdown
dependency: `src/markdown.rs` is a CommonMark-subset block and inline parser
written for what marksman actually needs. It builds no document tree; it emits
the facts the CST is made of — heading blocks, link occurrences, link reference
definitions, front matter, and the source regions that scanning must skip — all
as byte offsets into the original content. Wiki links and tags are then scanned
directly from the source, restricted to the regions the parser left open, so
code, HTML, math and link destinations cannot contribute elements.

Span conventions follow Markdig rather than CommonMark, because that is what the
original reports: an ATX heading's span excludes its closing `##` sequence but
keeps trailing whitespace; a setext heading's span includes its underline; a link
title's span includes its quotes while its text does not; a `<...>` destination's
span includes the angle brackets; and a label's span is whitespace-trimmed.
Reference links are recognised without consulting the definitions, because the
original patches Markdig's link parser to emit a link even when nothing matches —
so `[foo]` is always a shortcut link.

`bench/oracle/compare.py` is the differential test that pins all of this down: it
runs 140 tricky inputs through both the original's Markdig-based parser and this
one and diffs the elements. 139 match; the one that does not is the multi-line
link definition described under known deviations.

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
- **Multi-line link reference definitions.** When a definition spans several
  lines, Markdig reports label/url/title offsets into the buffer it builds by
  joining those lines, not into the source; `Parser.fs` carries a `TODO` about
  the resulting bad spans. The port reports true source offsets, so
  `[a]:\n  /url\n  "title"` gives `url=/url @ (1,2)-(1,6)` here and
  `@ (1,0)-(1,4)` there. Single-line definitions, the common case, agree.
- **`#` in a file name.** The original unescapes the whole URI before handing it
  to `System.Uri`, so a `%23` becomes a fragment separator and everything after
  it is dropped from the path. The port reproduces this, which is why the
  original's `pathFromRoot_SpecialChars` test is skipped there and ignored here,
  and why a wiki link to `blah#blah.md` resolves to nothing in both.
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
under `tests/`. 650 pass and the only three ignored are the ones the original
skips too — `footnote_1` and `ref_to_footnote_at_link` ("footnote parsing not
implemented") and `path_from_root_special_chars` ("`Uri` and `#` don't mix
well"). Four further gitignore cases are compiled only on Windows, as in the
original.

The parser has a second, stronger check: `bench/oracle/compare.py` runs 140
tricky Markdown inputs through the original's Markdig-based parser and this one
and diffs the resulting elements. See the design notes.

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

1500 documents, 1.18 MB of Markdown, with a 10 s quiet window:

| metric | marksman-rs | marksman (F#) | F#/rs |
| --- | --- | --- | --- |
| cold start (`initialize`) | 1027 ms | 4396 ms | 4.3x |
| first diagnostics settled | 559 ms | 810 ms | 1.4x |
| completion | 4.1 ms | 9.9 ms | 2.4x |
| definition | 0.5 ms | 1.3 ms | 2.6x |
| references | 0.5 ms | 1.6 ms | 3.2x |
| hover | 0.6 ms | 1.0 ms | 1.7x |
| documentSymbol | 0.6 ms | 1.4 ms | 2.3x |
| codeLens | 0.6 ms | 1.3 ms | 2.2x |
| semanticTokens/full | 0.5 ms | 1.0 ms | 2.0x |
| workspace/symbol | 6.8 ms | 31.3 ms | 4.6x |
| edit → diagnostics | 777 ms | 1554 ms | 2.0x |

5000 documents, 3.94 MB of Markdown, with a 30 s quiet window:

| metric | marksman-rs | marksman (F#) | F#/rs |
| --- | --- | --- | --- |
| cold start (`initialize`) | 4807 ms | 11126 ms | 2.3x |
| first diagnostics settled | 1885 ms | 1920 ms | 1.0x |
| completion | 10.9 ms | 13.2 ms | 1.2x |
| definition | 0.7 ms | 1.3 ms | 1.9x |
| references | 0.5 ms | 1.1 ms | 2.2x |
| hover | 0.5 ms | 1.1 ms | 2.2x |
| documentSymbol | 0.4 ms | 1.1 ms | 2.8x |
| codeLens | 0.5 ms | 1.1 ms | 2.2x |
| semanticTokens/full | 0.5 ms | 0.9 ms | 1.8x |
| workspace/symbol | 18.5 ms | 49.1 ms | 2.7x |
| edit → diagnostics | 3096 ms | not measured | — |

At 5000 documents the harness measures `edit → diagnostics` by waiting for a
quiet window, and it never observes the original's post-edit publication there —
not with a 2 s, 30 s or 90 s window. The direct comparison in the next section
shows the original does publish, and publishes the same seven diagnostics. The
quiet window also has to be wide enough for this port: at 2 s a loaded machine
reports nothing, which reads as a functional difference but is not one. Set
`BENCH_STDERR_DIR` to capture each server's stderr, since a server that dies
mid-run otherwise looks like one that simply published nothing.

Fixed process startup is not the explanation for the cold-start gap:
`marksman --version` takes 0.08 s for the original and under 0.01 s for this
port.

Internal phase timings for the 1500-document corpus
(`cargo run --release --example bench_load -- /tmp/bench-1500 5`, each phase its
own best of 5):

| phase | time |
| --- | --- |
| read + parse | 242 ms |
| lookup indexes + connection graph | 271 ms |
| total load | 558 ms |
| one-document edit (full graph rebuild) | 8.5 ms |

Throughput: 4.9 MB/s parsed, 2688 documents/s loaded.

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
src/               the port, one module per F# source file
tests/             ported test suites, one file per F# test module
tests/common/      shared fixtures, mirroring Tests/Helpers.fs
examples/          bench_load.rs (phase timings), dump_elements.rs (oracle)
bench/             gen_corpus.py, bench.py, lsp_smoke.py, fixtures/
bench/oracle/      compare.py and the F# dumper: differential parser test
```
