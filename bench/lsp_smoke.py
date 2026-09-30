#!/usr/bin/env python3
"""Drive an LSP server over stdio and report what it answers.

Used both as a functional smoke test and as the request-latency half of the
marksman vs marksman-rs comparison, so the same script runs against either
binary.

Usage: lsp_smoke.py <server-command...> [--repeat N]
"""

import json
import os
import queue
import subprocess
import sys
import threading
import time
from urllib.parse import quote


def encode(msg):
    body = json.dumps(msg).encode("utf-8")
    return b"Content-Length: %d\r\n\r\n%s" % (len(body), body)


class Client:
    """A minimal LSP client. A reader thread owns stdout so that waiting for
    notifications can time out instead of blocking forever."""

    def __init__(self, argv):
        self.proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        self.next_id = 0
        self.notifications = []
        self.incoming = queue.Queue()
        self.reader = threading.Thread(target=self._read_loop, daemon=True)
        self.reader.start()

    def _read_loop(self):
        try:
            while True:
                headers = {}
                while True:
                    line = self.proc.stdout.readline()
                    if not line:
                        return
                    line = line.strip()
                    if not line:
                        break
                    key, _, value = line.decode("ascii").partition(":")
                    headers[key.strip().lower()] = value.strip()
                if "content-length" not in headers:
                    continue
                body = self.proc.stdout.read(int(headers["content-length"]))
                if not body:
                    return
                self.incoming.put(json.loads(body.decode("utf-8")))
        except (OSError, ValueError):
            return

    def send(self, msg):
        self.proc.stdin.write(encode(msg))
        self.proc.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        rid = self.next_id
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        return self.wait_for_response(rid)

    def notify(self, method, params):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def wait_for_response(self, rid, timeout=120):
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"no response to request {rid}")
            msg = self.incoming.get(timeout=remaining)
            if msg.get("id") == rid and ("result" in msg or "error" in msg):
                return msg
            if "method" in msg:
                self.notifications.append(msg)

    def drain(self, seconds):
        """Collect notifications that arrive within the given window."""
        end = time.monotonic() + seconds
        while True:
            remaining = end - time.monotonic()
            if remaining <= 0:
                return
            try:
                msg = self.incoming.get(timeout=remaining)
            except queue.Empty:
                return
            if "method" in msg:
                self.notifications.append(msg)

    def close(self):
        try:
            self.request("shutdown", None, )
        except Exception:
            pass
        try:
            self.notify("exit", None)
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def uri(path):
    return "file://" + quote(path, safe="/:")


def main():
    sys.stdout.reconfigure(line_buffering=True)
    argv = []
    repeat = 1
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--repeat":
            repeat = int(args[i + 1])
            i += 2
        else:
            argv.append(args[i])
            i += 1

    default_root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")
    root = os.path.abspath(os.environ.get("LSP_ROOT", default_root))
    docs = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if not d.startswith(".")]
        for name in sorted(filenames):
            if name.endswith(".md"):
                full = os.path.join(dirpath, name)
                with open(full, encoding="utf-8") as f:
                    docs[full] = f.read()

    if not docs:
        sys.exit(f"no markdown documents found under {root}")

    timings = {}

    def timed(label, fn):
        best = None
        for _ in range(repeat):
            start = time.perf_counter()
            result = fn()
            elapsed = time.perf_counter() - start
            best = elapsed if best is None else min(best, elapsed)
        timings[label] = best
        return result

    t_boot = time.perf_counter()
    client = Client(argv)

    init = timed("initialize", lambda: client.request(
        "initialize",
        {
            "processId": os.getpid(),
            "rootUri": uri(root),
            "capabilities": {
                "textDocument": {
                    "documentSymbol": {"hierarchicalDocumentSymbolSupport": True},
                    "rename": {"prepareSupport": True},
                },
                "workspace": {"workspaceEdit": {"documentChanges": True}},
            },
            "workspaceFolders": [{"uri": uri(root), "name": os.path.basename(root)}],
            "initializationOptions": {"preferredTextSyncKind": 1},
        },
    ))
    timings["startup_to_initialize_response"] = time.perf_counter() - t_boot

    caps = init["result"]["capabilities"]
    print("== capabilities ==")
    for key in sorted(caps):
        print(f"  {key}: {json.dumps(caps[key], sort_keys=True)[:150]}")

    client.notify("initialized", {})

    # Open every document, as an editor would.
    t_open = time.perf_counter()
    for version, (path, text) in enumerate(docs.items()):
        client.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": uri(path),
                    "languageId": "markdown",
                    "version": version + 1,
                    "text": text,
                }
            },
        )
    timings["didOpen_all"] = time.perf_counter() - t_open

    client.drain(2.5)

    print("\n== notifications ==")
    counts = {}
    for n in client.notifications:
        counts[n["method"]] = counts.get(n["method"], 0) + 1
    for method, count in sorted(counts.items()):
        print(f"  {method}: {count}")

    diag_total = 0
    for n in client.notifications:
        if n["method"] == "textDocument/publishDiagnostics":
            diag_total += len(n["params"]["diagnostics"])
            for d in n["params"]["diagnostics"]:
                print(
                    "  diag [%s] %s:%d %s"
                    % (
                        d.get("code"),
                        os.path.basename(n["params"]["uri"].replace("file://", "")),
                        d["range"]["start"]["line"],
                        d["message"],
                    )
                )
    print(f"  total diagnostics: {diag_total}")

    # Pick a document that contains a wiki link for the feature checks.
    target = None
    for path, text in docs.items():
        if "[[" in text:
            target = path
            break
    target = target or next(iter(docs))
    print(f"\n== features on {os.path.relpath(target, root)} ==")

    def find_offset(text, needle, occurrence=0):
        idx = -1
        for _ in range(occurrence + 1):
            idx = text.index(needle, idx + 1)
        line = text.count("\n", 0, idx)
        col = idx - (text.rfind("\n", 0, idx) + 1)
        return {"line": line, "character": col + len(needle) // 2}

    doc_text = docs[target]
    probe = find_offset(doc_text, "[[") if "[[" in doc_text else {"line": 0, "character": 0}

    def pos_params(position):
        return {
            "textDocument": {"uri": uri(target)},
            "position": position,
        }

    r = timed("completion", lambda: client.request("textDocument/completion", pos_params(probe)))
    items = r.get("result") or []
    if isinstance(items, dict):
        items = items.get("items", [])
    print(f"  completion: {len(items)} items -> {[i['label'] for i in items[:6]]}")

    r = timed("definition", lambda: client.request("textDocument/definition", pos_params(probe)))
    result = r.get("result")
    locs = result if isinstance(result, list) else ([result] if result else [])
    print(f"  definition: {len(locs)} locations")
    for l in locs[:3]:
        print(f"    {os.path.basename(l['uri'].replace('file://', ''))} {l['range']['start']}")

    r = timed("references", lambda: client.request(
        "textDocument/references",
        {**pos_params(probe), "context": {"includeDeclaration": True}},
    ))
    print(f"  references: {len(r.get('result') or [])} locations")

    r = timed("documentSymbol", lambda: client.request(
        "textDocument/documentSymbol", {"textDocument": {"uri": uri(target)}}
    ))
    syms = r.get("result") or []
    print(f"  documentSymbol: {len(syms)} top-level -> {[s.get('name') for s in syms[:5]]}")

    r = timed("hover", lambda: client.request("textDocument/hover", pos_params(probe)))
    hover = r.get("result")
    if hover:
        contents = hover["contents"]
        text = contents.get("value") if isinstance(contents, dict) else str(contents)
        print(f"  hover: {len(text)} chars, first line: {text.splitlines()[0][:70]!r}")
    else:
        print("  hover: none")

    r = timed("codeLens", lambda: client.request(
        "textDocument/codeLens", {"textDocument": {"uri": uri(target)}}
    ))
    lenses = r.get("result") or []
    print(f"  codeLens: {len(lenses)} -> {[l['command']['title'] for l in lenses[:5] if l.get('command')]}")

    r = timed("semanticTokens/full", lambda: client.request(
        "textDocument/semanticTokens/full", {"textDocument": {"uri": uri(target)}}
    ))
    tokens = (r.get("result") or {}).get("data") or []
    print(f"  semanticTokens/full: {len(tokens)} numbers ({len(tokens) // 5} tokens)")

    r = timed("workspaceSymbol", lambda: client.request("workspace/symbol", {"query": "a"}))
    print(f"  workspace/symbol('a'): {len(r.get('result') or [])} symbols")

    r = timed("codeAction", lambda: client.request(
        "textDocument/codeAction",
        {
            "textDocument": {"uri": uri(target)},
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
            "context": {"diagnostics": []},
        },
    ))
    actions = r.get("result") or []
    print(f"  codeAction: {len(actions)} -> {[a.get('title') for a in actions[:5]]}")

    # An incremental edit, to exercise didChange and the resulting diagnostics.
    t_edit = time.perf_counter()
    client.notify(
        "textDocument/didChange",
        {
            "textDocument": {"uri": uri(target), "version": 99},
            "contentChanges": [{"text": doc_text + "\n# Appended Heading\n\n[[Appended Heading]]\n"}],
        },
    )
    timings["didChange_full"] = time.perf_counter() - t_edit
    client.drain(2.0)

    after = [n for n in client.notifications if n["method"] == "textDocument/publishDiagnostics"]
    print(f"\n== after edit: {len(after)} publishDiagnostics so far ==")

    r = timed("documentSymbol_after_edit", lambda: client.request(
        "textDocument/documentSymbol", {"textDocument": {"uri": uri(target)}}
    ))
    print(f"  documentSymbol: {len(r.get('result') or [])} top-level")

    client.close()

    print("\n== timings (seconds, best of %d) ==" % repeat)
    for label, value in timings.items():
        print(f"  {label:38s} {value * 1000:9.2f} ms")


if __name__ == "__main__":
    main()
