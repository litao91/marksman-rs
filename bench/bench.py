#!/usr/bin/env python3
"""Compare two LSP servers on the same Markdown corpus.

Measures the phases that dominate real use:
  * cold start          - process spawn to the initialize response, which covers
                          the workspace scan, parse, index and connection graph
  * first diagnostics   - until the server finishes publishing diagnostics
  * request latency     - repeated language requests over the loaded workspace
  * edit turnaround     - didChange to the diagnostics that follow it

Usage: bench.py <corpus-dir> <label>=<cmd...> [<label>=<cmd...> ...]
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
    def __init__(self, argv):
        self.proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        self.next_id = 0
        self.incoming = queue.Queue()
        self.notifications = []
        self.last_notification_at = None
        # (timestamp, method) for every notification, so that "time until the
        # server finished publishing" can be measured without counting the
        # silence window the harness waits through.
        self.notification_log = []
        threading.Thread(target=self._read_loop, daemon=True).start()

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
                msg = json.loads(body.decode("utf-8"))
                if "method" in msg:
                    now = time.perf_counter()
                    self.notifications.append(msg)
                    self.notification_log.append((now, msg["method"]))
                    self.last_notification_at = now
                self.incoming.put(msg)
        except (OSError, ValueError):
            return

    def request(self, method, params, timeout=600):
        self.next_id += 1
        rid = self.next_id
        self.proc.stdin.write(encode({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
        self.proc.stdin.flush()

        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(method)
            msg = self.incoming.get(timeout=remaining)
            if msg.get("id") == rid and ("result" in msg or "error" in msg):
                return msg

    def notify(self, method, params):
        self.proc.stdin.write(encode({"jsonrpc": "2.0", "method": method, "params": params}))
        self.proc.stdin.flush()

    def quiet_for(self, seconds, hard_limit, since=None):
        """Wait until no *new* notification has arrived for `seconds`.

        `since` is a perf_counter baseline; notifications that predate it do not
        count as activity.
        """
        start = time.monotonic()
        baseline = since if since is not None else start
        while time.monotonic() - start < hard_limit:
            last = self.last_notification_at
            if last is not None and last >= baseline and time.perf_counter() - last > seconds:
                return
            if last is None and time.monotonic() - start > seconds:
                return
            try:
                self.incoming.get(timeout=0.2)
            except queue.Empty:
                if time.monotonic() - start > seconds:
                    return
        return

    def last_notification_elapsed(self, since, method=None):
        """Seconds from `since` until the last matching notification arrived."""
        matches = [
            t
            for t, name in self.notification_log
            if t >= since and (method is None or name == method)
        ]
        if not matches:
            return None
        return (max(matches) - since) * 1000

    def close(self):
        try:
            self.request("shutdown", None, timeout=10)
            self.notify("exit", None)
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


def file_uri(path):
    return "file://" + quote(path, safe="/:")


def run_one(label, argv, root, docs, repeats, open_count, quiet):
    result = {"label": label}

    t0 = time.perf_counter()
    client = Client(argv)
    caps = client.request(
        "initialize",
        {
            "processId": os.getpid(),
            "rootUri": file_uri(root),
            "capabilities": {
                "textDocument": {
                    "documentSymbol": {"hierarchicalDocumentSymbolSupport": True},
                    "rename": {"prepareSupport": True},
                },
                "workspace": {"workspaceEdit": {"documentChanges": True}},
            },
            "workspaceFolders": [{"uri": file_uri(root), "name": "bench"}],
            "initializationOptions": {"preferredTextSyncKind": 1},
        },
    )
    result["cold_start_ms"] = (time.perf_counter() - t0) * 1000

    client.notify("initialized", {})

    # Open a subset, as an editor does. Every didOpen rebuilds the connection
    # graph under the default configuration, so opening a whole corpus would
    # measure a pathological case no editor produces.
    opened = docs[:open_count] if open_count else docs
    t_open = time.perf_counter()
    for version, (path, text) in enumerate(opened):
        client.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": file_uri(path),
                    "languageId": "markdown",
                    "version": version + 1,
                    "text": text,
                }
            },
        )
    result["did_open_send_ms"] = (time.perf_counter() - t_open) * 1000
    result["docs_opened"] = len(opened)

    # Diagnostics are computed asynchronously; wait for the stream to settle.
    t_diag = time.perf_counter()
    client.quiet_for(quiet, 600.0, since=t_diag)
    settled = client.last_notification_elapsed(
        t_diag, "textDocument/publishDiagnostics"
    )
    result["first_diagnostics_ms"] = settled if settled is not None else 0.0

    diag_notifications = [n for n in client.notifications if n["method"] == "textDocument/publishDiagnostics"]
    result["docs_with_published_diagnostics"] = len(diag_notifications)
    result["total_diagnostics"] = sum(len(n["params"]["diagnostics"]) for n in diag_notifications)

    # A document in the middle of the corpus, with a wiki link to probe.
    probe_path, probe_text = docs[len(docs) // 2]
    link_at = probe_text.index("[[")
    probe_pos = {
        "line": probe_text.count("\n", 0, link_at),
        "character": link_at - (probe_text.rfind("\n", 0, link_at) + 1) + 3,
    }

    def pos_params(position):
        return {"textDocument": {"uri": file_uri(probe_path)}, "position": position}

    probes = {
        "completion": ("textDocument/completion", pos_params(probe_pos)),
        "definition": ("textDocument/definition", pos_params(probe_pos)),
        "references": (
            "textDocument/references",
            {**pos_params(probe_pos), "context": {"includeDeclaration": True}},
        ),
        "hover": ("textDocument/hover", pos_params(probe_pos)),
        "documentSymbol": (
            "textDocument/documentSymbol",
            {"textDocument": {"uri": file_uri(probe_path)}},
        ),
        "codeLens": ("textDocument/codeLens", {"textDocument": {"uri": file_uri(probe_path)}}),
        "semanticTokens": (
            "textDocument/semanticTokens/full",
            {"textDocument": {"uri": file_uri(probe_path)}},
        ),
        "workspaceSymbol": ("workspace/symbol", {"query": "note 007"}),
    }

    for name, (method, params) in probes.items():
        samples = []
        for _ in range(repeats):
            start = time.perf_counter()
            client.request(method, params)
            samples.append((time.perf_counter() - start) * 1000)
        samples.sort()
        result[f"{name}_ms"] = samples[len(samples) // 2]

    # An edit, and how long the resulting diagnostics take.
    t_edit = time.perf_counter()
    before_count = len(client.notifications)
    client.notify(
        "textDocument/didChange",
        {
            "textDocument": {"uri": file_uri(probe_path), "version": 10**6},
            "contentChanges": [{"text": probe_text + "\n# Bench Appended\n\n[[Bench Appended]]\n"}],
        },
    )
    client.quiet_for(quiet, 600.0, since=t_edit)
    settled = client.last_notification_elapsed(t_edit)
    result["edit_to_diagnostics_ms"] = settled if settled is not None else 0.0
    result["notifications_after_edit"] = len(client.notifications) - before_count

    client.close()
    return result


def load_docs(root, limit):
    docs = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if not d.startswith(".")]
        for name in sorted(filenames):
            if not name.endswith(".md"):
                continue
            full = os.path.join(dirpath, name)
            with open(full, encoding="utf-8") as f:
                docs.append((full, f.read()))
            if limit and len(docs) >= limit:
                return docs
    return docs


def main():
    sys.stdout.reconfigure(line_buffering=True)
    root = os.path.abspath(sys.argv[1])
    specs = sys.argv[2:]
    repeats = int(os.environ.get("BENCH_REPEATS", "5"))
    limit = int(os.environ.get("BENCH_DOCS", "0"))
    open_count = int(os.environ.get("BENCH_OPEN", "20"))
    # Diagnostics are published asynchronously; wait this long for the stream to
    # go quiet before deciding the server has finished.
    quiet = float(os.environ.get("BENCH_QUIET", "2.0"))

    docs = load_docs(root, limit)
    total_bytes = sum(len(t.encode("utf-8")) for _, t in docs)
    print(f"corpus: {len(docs)} documents, {total_bytes / 1e6:.2f} MB of markdown")
    print(f"documents opened after load: {open_count or 'all'}")
    print(f"repeats per request probe: {repeats}")
    print(f"quiet window for async work: {quiet}s\n")

    results = []
    for spec in specs:
        label, _, command = spec.partition("=")
        argv = command.split()
        print(f"--- running {label}: {' '.join(argv)}")
        results.append(run_one(label, argv, root, docs, repeats, open_count, quiet))

    metrics = [
        ("cold_start_ms", "cold start (initialize)"),
        ("did_open_send_ms", "didOpen batch (send)"),
        ("first_diagnostics_ms", "first diagnostics (settled)"),
        ("completion_ms", "completion"),
        ("definition_ms", "definition"),
        ("references_ms", "references"),
        ("hover_ms", "hover"),
        ("documentSymbol_ms", "documentSymbol"),
        ("codeLens_ms", "codeLens"),
        ("semanticTokens_ms", "semanticTokens/full"),
        ("workspaceSymbol_ms", "workspace/symbol"),
        ("edit_to_diagnostics_ms", "edit -> diagnostics"),
    ]

    width = max(max(len(r["label"]) for r in results) + 2, 11)
    header = f"{'metric':32s}" + "".join(f"{r['label']:>{width}s}" for r in results)
    if len(results) == 2:
        header += f"{'rs vs fs':>12s}"
    print("\n" + "=" * len(header))
    print(header)
    print("=" * len(header))

    for key, name in metrics:
        row = f"{name:32s}"
        values = [r.get(key) for r in results]
        for v in values:
            row += f"{('n/a' if v is None else f'{v:,.1f}'):>{width}s}"
        if len(results) == 2 and values[0] and values[1]:
            # Below 1.0 means the Rust server is faster; the factor is fs/rs.
            row += f"{values[0] / values[1]:>10.2f}x"
        print(row)

    print("=" * len(header))
    print("\nfunctional cross-check")
    for key in ("docs_with_published_diagnostics", "total_diagnostics", "notifications_after_edit"):
        print(f"  {key:36s}" + "".join(f"{str(r.get(key)):>{width}s}" for r in results))

    print("\nraw json")
    print(json.dumps(results, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
