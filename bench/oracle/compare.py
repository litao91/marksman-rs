#!/usr/bin/env python3
"""Differential test for the Markdown parser.

Writes a corpus of tricky Markdown cases, runs both the original Markdig-based
parser (`dump_fs.fsx`) and this port (`examples/dump_elements.rs`) over them, and
reports per-case differences in the CST elements each produces.

Usage: compare.py [--case NAME] [--keep]

Exit status is 0 only if every case matches.
"""

import argparse
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
DOTNET_ROOT = os.environ.get("DOTNET_ROOT", os.path.expanduser("~/.dotnet"))

# Each value is written verbatim to `<name>.md`.
CASES = {
    # --- ATX headings ---
    "heading_atx_levels": "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n",
    "heading_atx_seven_hashes": "####### not a heading\n",
    "heading_atx_no_space": "#nospace\n# space\n",
    "heading_atx_closing_hashes": "## Title ##\n## Title ##  \n## ##\n",
    "heading_atx_indented": "   # three spaces\n    # four is code\n",
    "heading_atx_trailing_space": "# Title \n## 45\n",
    "heading_atx_empty": "#\n#\n# \n",
    "heading_duplicate_slugs": "# A\n# A\n# A\n",
    # --- setext headings ---
    "heading_setext_equals": "Foo\n===\n",
    "heading_setext_dash": "Foo\n-\n",
    "heading_setext_indented": "Foo\n   ===\n    ==== too far\n",
    "heading_setext_multiword": "Foo bar baz\n---\n",
    "heading_setext_then_text": "Foo\n-\nbar\n",
    # The case the pulldown-based parser could not reproduce: Markdig reads the
    # second `-` as a setext underline rather than a second list item.
    "regression_156": "A\n\n-\n-\n",
    # --- code ---
    "code_fenced_backtick": "```\n[[not a wiki]] #notatag\n```\n",
    "code_fenced_tilde": "~~~\n[[not a wiki]]\n~~~\n",
    "code_fenced_info": "```rust fn main() {}\nlet x = 1;\n```\n",
    "code_fenced_unclosed": "```\nnever closed\n",
    "code_fenced_nested": "````\n```\ninner\n```\n````\n",
    "code_fenced_empty": "```\n```\n",
    "code_indented": "text\n\n    [[not a wiki]] #notatag\n\nmore\n",
    "code_indented_in_list": "- item\n\n      code in list\n",
    "code_span_one": "a `[[not wiki]]` b\n",
    "code_span_two": "a ``b ` c`` d\n",
    "code_span_unmatched": "a ` b\n",
    "code_span_empty": "a `` b\n",
    "code_span_multiline": "a `b\nc` d\n",
    # --- HTML ---
    "html_block_div": "<div>\n[[not a wiki]]\n</div>\n",
    "html_block_comment": "<!-- comment [[x]] -->\ntext\n",
    "html_block_pre": "<pre>\n[[not a wiki]]\n</pre>\n",
    "html_block_script": "<script>\nvar x = 1;\n</script>\n",
    "html_block_decl": "<!DOCTYPE html>\ntext\n",
    "html_block_cdata": "<![CDATA[ [[x]] ]]>\ntext\n",
    "html_inline": "a <b>[[wiki]]</b> c\n",
    "html_inline_comment": "a <!-- c --> b [[wiki]]\n",
    "html_tag_alone_in_paragraph": "text\n<blank>\nmore [[wiki]] #tag\n",
    "html_known_tag_interrupts": "text\n<div>\n[[x]]\n</div>\n",
    "semato_document": (
        "# Title\n"
        "Start with [[a-wiki-link]]. Then a [ref-link].\n"
        "[[wiki-at-sol]]\n"
        "<blank>\n"
        "End with [[wiki-link-no-eol]] and #tag."
    ),
    "html_inline_unclosed": "a < b and 1 < 2 > 0\n",
    # --- math ---
    "math_inline": "a $x^2$ b [[wiki]]\n",
    "math_display": "$$\nx^2\n$$\n",
    "math_block_wikilink": "$$\n\\begin{verbatim}\n[[nodiscard]]\n\\end{verbatim}\n$$",
    "math_block_then_text": "$$\n[[in-math]]\n$$\n\nRegular [[valid-link]]",
    "math_block_unclosed": "$$\n[[x]] and #tag\n",
    "math_inline_wikilink": "Inline math: $[[x]]$ in text",
    "math_unmatched": "price is $5 and $6\n",
    "math_empty": "a $$ b\n",
    # --- front matter ---
    "yaml_basic": "---\ntitle: X\n---\n\n# X\n",
    "yaml_dots": "---\ntitle: X\n...\n\n# X\n",
    "yaml_unclosed": "---\ntitle: X\n",
    "yaml_not_at_start": "\n---\ntitle: X\n---\n",
    "yaml_empty": "---\n---\n\n# X\n",
    "yaml_thematic_lookalike": "---\n\n# X\n",
    # --- links ---
    "link_inline": "[text](dest.md)\n",
    "link_inline_title_double": '[la bel](url "title")\n',
    "link_inline_title_single": "[a](b 'c')\n",
    "link_inline_title_paren": "[a](b (c))\n",
    "link_inline_angle_dest": "[a](<b c>)\n",
    "link_inline_nested_parens": "[a](b(c)d)\n",
    "link_inline_empty_label": "[]()\n[](url)\n",
    "link_inline_empty_dest": "[a]()\n",
    "link_inline_escaped_bracket": r"[a\]b](c)" + "\n",
    "link_full_reference": "[a][b]\n\n[b]: /url\n",
    "link_full_reference_undefined": "[a][b]\n",
    "link_collapsed": "[short cut][]\n",
    "link_collapsed_undefined": "[never defined][]\n",
    "link_shortcut": "[shortcut]\n",
    "link_shortcut_undefined": "[never defined]\n",
    "link_shortcut_multiline": "[\nmulti\nline\nlabel\n]\n",
    "link_image": "![alt](img.png)\n",
    "link_image_reference": "![alt][ref]\n\n[ref]: /i.png\n",
    "link_autolink": "<https://example.com/a?b=1&c=2>\n",
    "link_email": "<someone@example.com>\n",
    "link_nested": "[outer [inner](i)](o)\n",
    "link_in_link": "[a](b) [c](d)\n",
    "link_unbalanced": "[a(b\n",
    "link_empty_collapsed": "[][]\n",
    "link_empty_label_ref": "[][x]\n",
    "link_unquoted_title": "[la bel](url title)\n",
    "link_junk_after_title": '[a](b "t" x)\n',
    "link_heading_hashes_only": "# #\n",
    "link_def_then_use": "[foo]: /foo 'foo title'\n\n[foo]\n",
    # --- link reference definitions ---
    "linkdef_basic": "[My Label]: /some/url \"A Title\"\n",
    "linkdef_no_title": "[a]: /b\n",
    "linkdef_angle": "[a]: </b c> \"t\"\n",
    "linkdef_multiline": "[a]:\n  /url\n  \"title\"\n",
    "linkdef_duplicate": "[a]: /1\n[a]: /2\n",
    "linkdef_in_blockquote": "> [a]: /b\n",
    "linkdef_indented": "    [a]: /b\n",
    "linkdef_followed_by_text": "[a]: /b\ntext\n",
    "linkdef_empty_label": "[]: /b\n",
    # --- wiki links ---
    "wiki_plain": "[[note]]\n",
    "wiki_heading": "[[#heading]]\n",
    "wiki_doc_heading": "[[doc#heading]]\n",
    "wiki_title": "[[doc|title]]\n",
    "wiki_doc_heading_title": "[[doc#heading|title]]\n",
    "wiki_empty": "[[]]\n",
    "wiki_pipe_only": "[[|]]\n",
    "wiki_escaped_hash": r"[[doc\#heading]]" + "\n",
    "wiki_spaces": "[[My Note]]\n",
    "wiki_path": "[[notes/sub/My Note.md]]\n",
    "wiki_adjacent": "[[a]][[b]]\n",
    "wiki_unclosed": "[[unclosed\n",
    "wiki_in_code": "`[[x]]`\n\n```\n[[y]]\n```\n",
    "wiki_in_link": "[text [[wiki]]](url)\n",
    "wiki_in_blockquote": "> [[wiki]] in a quote\n",
    "wiki_in_list": "- [[wiki]] in a list\n",
    "wiki_across_lines": "[[a\nb]]\n",
    "wiki_hash_in_heading": "# Heading [[wiki]]\n",
    # --- tags ---
    "tag_basic": "#tag and #another\n",
    "tag_slash": "#a/b/c\n",
    "tag_dashes": "#with-dash_and_underscore\n",
    "tag_glued": "word#notatag\n",
    "tag_at_line_start": "#tag\n",
    "tag_after_wiki": "[[doc#heading]] #realtag\n",
    "tag_in_code": "`#notatag`\n",
    "tag_trailing_punct": "#tag. and #tag, and #tag!\n",
    "tag_heading_lookalike": "# Heading\n\n#tag\n",
    # --- containers ---
    "quote_simple": "> text\n",
    "quote_nested": "> > deep\n",
    "quote_lazy": "> text\nlazy continuation\n",
    "quote_paragraphs": "> p1\n>\n> p2\n",
    "list_bullet": "- a\n- b\n",
    "list_ordered": "1. a\n2. b\n",
    "list_nested": "- a\n  - b\n    - c\n",
    "list_loose": "- a\n\n- b\n",
    "list_paragraph": "- a\n\n  continued\n",
    "list_star_plus": "* a\n+ b\n",
    "list_no_space": "-not a list\n",
    "list_ordered_long": "123456789. ok\n1234567890. too long\n",
    "thematic_break": "***\n\n- - -\n\n___\n",
    "thematic_vs_setext": "text\n---\n",
    # --- misc / regression ---
    "empty": "",
    "only_newlines": "\n\n\n",
    "crlf": "# Title\r\n\r\ntext [[wiki]]\r\n",
    "unicode": "# 日本語の見出し\n\n[[日本語]] #タグ\n",
    "emoji": "# 🎉 Party\n\n[[🎉]] #🎉\n",
    "nbsp": "a\u00a0b\n",
    "long_paragraph": "word " * 200 + "\n",
    "mixed_everything": (
        "---\ntitle: T\n---\n\n# Title\n\nIntro [[wiki]] and #tag.\n\n"
        "## Section\n\n- item with [[link]]\n- `code [[x]]`\n\n"
        "> quoted [[q]]\n\n"
        "[ref]: /url \"t\"\n\n"
        "[text](dest.md#anchor) and [ref] and [a][ref]\n\n"
        "```js\n[[not a link]]\n```\n\n"
        "$math$ and <!-- html [[x]] -->\n\n"
        "Ending\n---\n"
    ),
}


def run(argv, env=None):
    full_env = dict(os.environ)
    full_env["DOTNET_ROOT"] = DOTNET_ROOT
    full_env["PATH"] = full_env["PATH"] + ":" + DOTNET_ROOT
    if env:
        full_env.update(env)
    return subprocess.run(argv, capture_output=True, text=True, env=full_env, cwd=REPO)


def write_cases(directory, only=None):
    for name, content in CASES.items():
        if only and name != only:
            continue
        with open(os.path.join(directory, name + ".md"), "w", encoding="utf-8") as f:
            f.write(content)


def split_output(text):
    """Split a dumper's output into {case name: [lines]}."""
    cases = {}
    current = None
    for line in text.splitlines():
        if line.startswith("===== "):
            current = line[len("===== ") :]
            cases[current] = []
        elif current is not None:
            cases[current].append(line)
    return cases


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--case", help="run only this case")
    ap.add_argument("--keep", action="store_true", help="keep the temp directory")
    ap.add_argument("--list", action="store_true", help="list case names and exit")
    args = ap.parse_args()

    if args.list:
        for name in sorted(CASES):
            print(name)
        return 0

    binary = os.path.join(REPO, "target/release/examples/dump_elements")
    build = subprocess.run(
        ["cargo", "build", "--release", "--example", "dump_elements"],
        capture_output=True,
        text=True,
        cwd=REPO,
    )
    if build.returncode != 0:
        print(build.stdout[-4000:])
        print(build.stderr[-4000:], file=sys.stderr)
        return 2

    tmp = tempfile.mkdtemp(prefix="marksman-oracle-")
    try:
        write_cases(tmp, args.case)

        fs = run(["dotnet", "fsi", os.path.join(HERE, "dump_fs.fsx"), tmp])
        if fs.returncode != 0:
            print("F# dumper failed:", file=sys.stderr)
            print(fs.stdout[-3000:], fs.stderr[-3000:], file=sys.stderr)
            return 2

        rs = run([binary, tmp])
        if rs.returncode != 0:
            print("Rust dumper failed:", file=sys.stderr)
            print(rs.stdout[-3000:], rs.stderr[-3000:], file=sys.stderr)
            return 2

        expected = split_output(fs.stdout)
        actual = split_output(rs.stdout)

        names = sorted(set(expected) | set(actual))
        failures = []
        for name in names:
            want = expected.get(name)
            got = actual.get(name)
            if want == got:
                continue
            failures.append(name)
            print(f"--- {name} ---")
            if want is None:
                print("  missing from the F# output")
                continue
            if got is None:
                print("  missing from the Rust output")
                continue
            for i in range(max(len(want), len(got))):
                w = want[i] if i < len(want) else "<absent>"
                g = got[i] if i < len(got) else "<absent>"
                if w != g:
                    print(f"  line {i}")
                    print(f"    markdig: {w}")
                    print(f"    port   : {g}")

        print()
        print(f"{len(names) - len(failures)}/{len(names)} cases match")
        if failures:
            print("differing: " + ", ".join(failures))
        return 1 if failures else 0
    finally:
        if args.keep:
            print(f"cases kept in {tmp}")
        else:
            for name in os.listdir(tmp):
                os.unlink(os.path.join(tmp, name))
            os.rmdir(tmp)


if __name__ == "__main__":
    sys.exit(main())
