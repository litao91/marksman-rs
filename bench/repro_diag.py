#!/usr/bin/env python3
"""Minimal repro: open a document, edit it, and watch what the server publishes."""

import json
import os
import queue
import subprocess
import sys
import threading
import time


def encode(msg):
    body = json.dumps(msg).encode("utf-8")
    return b"Content-Length: %d\r\n\r\n" % len(body) + body


def main():
    argv = sys.argv[1:]
    root = os.path.abspath("/tmp/repro-ws")
    os.makedirs(root, exist_ok=True)
    open(os.path.join(root, ".marksman.toml"), "w").close()
    with open(os.path.join(root, "a.md"), "w") as f:
        f.write("# A\n\n[[Missing]]\n")

    proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr)
    q = queue.Queue()

    def reader():
        while True:
            headers = {}
            while True:
                line = proc.stdout.readline()
                if not line:
                    return
                line = line.strip()
                if not line:
                    break
                k, _, v = line.decode("ascii").partition(":")
                headers[k.strip().lower()] = v.strip()
            body = proc.stdout.read(int(headers["content-length"]))
            q.put(json.loads(body))

    threading.Thread(target=reader, daemon=True).start()

    def show(label, until):
        n = 0
        while time.monotonic() < until:
            try:
                msg = q.get(timeout=0.2)
            except queue.Empty:
                continue
            n += 1
            if msg.get("method") == "textDocument/publishDiagnostics":
                p = msg["params"]
                print(f"  [{label}] publishDiagnostics {os.path.basename(p['uri'])}: "
                      f"{[d['message'] for d in p['diagnostics']]}")
            elif "id" in msg:
                print(f"  [{label}] response id={msg['id']} keys={list(msg.keys())}")
            else:
                print(f"  [{label}] {msg.get('method')}")
        if n == 0:
            print(f"  [{label}] <nothing>")

    def send(msg):
        proc.stdin.write(encode(msg))
        proc.stdin.flush()

    uri = "file://" + os.path.join(root, "a.md")
    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "processId": os.getpid(), "rootUri": uri.rsplit("/", 1)[0],
        "capabilities": {}, "workspaceFolders": [{"uri": uri.rsplit("/", 1)[0], "name": "ws"}]}})
    show("init", time.monotonic() + 1.0)
    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
    show("initialized", time.monotonic() + 3.0)

    send({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
        "textDocument": {"uri": uri, "languageId": "markdown", "version": 1, "text": "# A\n\n[[Missing]]\n"}}})
    show("didOpen", time.monotonic() + 3.0)

    send({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
        "textDocument": {"uri": uri, "version": 2},
        "contentChanges": [{"text": "# A\n\n[[Missing]]\n\n[[Also Missing]]\n"}]}})
    show("didChange", time.monotonic() + 3.0)

    send({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol",
          "params": {"textDocument": {"uri": uri}}})
    show("symbol", time.monotonic() + 2.0)

    send({"jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": None})
    send({"jsonrpc": "2.0", "method": "exit", "params": None})
    time.sleep(0.5)
    proc.kill()


if __name__ == "__main__":
    main()
