#!/usr/bin/env python3
"""Generate a synthetic Markdown vault for benchmarking.

Produces N documents that cross-reference each other by title, by relative path
and by heading anchor, plus tags, reference links and a couple of deliberately
broken links so that diagnostics have work to do.

Usage: gen_corpus.py <output-dir> [num-docs]
"""

import os
import random
import sys


def doc_body(i, n, rng):
    title = f"Note {i:05d}"
    lines = [
        "---",
        f"title: {title}",
        "tags: [bench, generated]",
        "---",
        "",
        f"# {title}",
        "",
        f"Document {i} of {n}. It links to a few of its neighbours below.",
        "",
        "## First Section",
        "",
    ]

    for _ in range(6):
        target = rng.randrange(n)
        style = rng.randrange(4)
        if style == 0:
            lines.append(f"A wiki link to [[Note {target:05d}]] by title.")
        elif style == 1:
            sub = target % 8
            lines.append(f"A wiki link to [[sub{sub}/Note {target:05d}]] by path.")
        elif style == 2:
            lines.append(f"A wiki link with a heading [[Note {target:05d}#Second Section]].")
        else:
            lines.append(f"An inline link to [note](sub{target % 8}/Note%20{target:05d}.md#second-section).")

    lines += [
        "",
        "## Second Section",
        "",
        "Some #bench tags and #generated tags, plus an intra-document link",
        "to [[#First Section]].",
        "",
        "See also [the reference][ref] and [ref] again.",
        "",
        "[ref]: https://example.com \"An external reference\"",
        "",
        "```",
        "code blocks must not contribute links: [[Note 00000]] #notatag",
        "```",
        "",
        "## Third Section",
        "",
        "| a | b |",
        "| - | - |",
        "| 1 | 2 |",
        "",
    ]

    # A small number of documents carry a broken link, for diagnostics.
    if i % 50 == 0:
        lines.append("A link to [[Note That Does Not Exist]] is broken.")

    return "\n".join(lines)


def main():
    out = sys.argv[1]
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 1000
    rng = random.Random(20260930)

    os.makedirs(out, exist_ok=True)
    # A project marker so the server treats the directory as a real workspace.
    open(os.path.join(out, ".marksman.toml"), "w").close()

    for sub in range(8):
        os.makedirs(os.path.join(out, f"sub{sub}"), exist_ok=True)

    total_bytes = 0
    for i in range(n):
        sub = i % 8
        path = os.path.join(out, f"sub{sub}", f"Note {i:05d}.md")
        body = doc_body(i, n, rng)
        with open(path, "w", encoding="utf-8") as f:
            f.write(body)
        total_bytes += len(body.encode("utf-8"))

    print(f"wrote {n} documents ({total_bytes / 1e6:.1f} MB) to {out}")


if __name__ == "__main__":
    main()
