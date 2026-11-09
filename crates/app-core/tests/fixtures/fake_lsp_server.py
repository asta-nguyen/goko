#!/usr/bin/env python3
"""Fake LSP server for the lsp.rs tests.

Speaks Content-Length-framed JSON-RPC 2.0 on stdio (LSP 3.17), unlike the
newline-delimited fake_acp_agent.py. Behavior is selected by the
FAKE_LSP_CASE environment variable; FAKE_LSP_CAPS is a JSON object merged
over the default server capabilities; FAKE_LSP_LOG names a file that gets
one JSON event per line (every incoming method/response plus the canned
server->client exchanges); FAKE_LSP_PIDFILE receives the process id at
startup and FAKE_LSP_CHILD_PIDFILE the grandchild's pid for with_grandchild.
FAKE_LSP_COUNT names a file used by flaky_once to crash only the first
connection.
"""

import json
import os
import subprocess
import sys
import time

PIDFILE = os.environ.get("FAKE_LSP_PIDFILE")
if PIDFILE:
    with open(PIDFILE, "w") as handle:
        handle.write(str(os.getpid()))

LOG = os.environ.get("FAKE_LSP_LOG")
CASE = os.environ.get("FAKE_LSP_CASE", "ok")
COUNT_FILE = os.environ.get("FAKE_LSP_COUNT")
# A request method that is logged but never answered (cancel/timeout tests).
HANG_METHOD = os.environ.get("FAKE_LSP_HANG")

CHILD_PIDFILE = os.environ.get("FAKE_LSP_CHILD_PIDFILE")
if CASE == "with_grandchild" and CHILD_PIDFILE:
    # Like a wrapper such as `npx`: a long-lived child in our process group.
    grandchild = subprocess.Popen(
        ["sleep", "300"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    with open(CHILD_PIDFILE, "w") as handle:
        handle.write(str(grandchild.pid))

CAPS = {
    "textDocumentSync": 2,
    "completionProvider": {"triggerCharacters": [".", ">"]},
    "hoverProvider": True,
    "definitionProvider": True,
}
overrides = os.environ.get("FAKE_LSP_CAPS")
if overrides:
    CAPS.update(json.loads(overrides))

_next_server_id = 0


def send(message):
    body = json.dumps(message).encode("utf-8")
    sys.stdout.buffer.write(b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body)
    sys.stdout.buffer.flush()


def log(event):
    if LOG:
        text = json.dumps(event)
        if len(text) > 4000:
            text = text[:4000] + "...<truncated>"
        with open(LOG, "a") as handle:
            handle.write(text + "\n")


def read_message():
    """One Content-Length framed message, or None on EOF."""
    headers = {}
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.partition(b":")
        headers[name.strip().lower()] = value.strip()
    try:
        length = int(headers.get(b"content-length", b"0"))
    except ValueError:
        return None
    if length <= 0:
        return None
    body = sys.stdin.buffer.read(length)
    if len(body) < length:
        return None
    try:
        return json.loads(body)
    except ValueError:
        return None


def respond(request_id, result=None, error=None):
    message = {"jsonrpc": "2.0", "id": request_id}
    if error is not None:
        message["error"] = error
    else:
        message["result"] = result
    send(message)


def method_result(method, params):
    if method == "shutdown":
        return None, None
    if method == "textDocument/completion":
        return {
            "isIncomplete": False,
            "items": [
                {
                    "label": "fakeItem",
                    "kind": 1,
                    "insertText": "fakeInserted",
                    "detail": "fake detail",
                }
            ],
        }, None
    if method == "textDocument/hover":
        return {"contents": {"kind": "plaintext", "value": "fake hover"}}, None
    if method == "textDocument/definition":
        return [
            {
                "uri": "file:///fake/checkout/def.rs",
                "range": {
                    "start": {"line": 3, "character": 0},
                    "end": {"line": 3, "character": 4},
                },
            }
        ], None
    return None, {"code": -32601, "message": "method not found"}


def initialize(request_id):
    respond(
        request_id,
        {
            "capabilities": CAPS,
            "serverInfo": {"name": "fake-lsp", "version": "0.0.1"},
        },
    )


def server_requests():
    """The canned server->client exchanges the 'ok' case performs once."""
    global _next_server_id
    _next_server_id += 1
    send(
        {
            "jsonrpc": "2.0",
            "id": f"srv-cfg-{_next_server_id}",
            "method": "workspace/configuration",
            "params": {"items": [{"section": "rust"}, {"section": "editor"}]},
        }
    )
    _next_server_id += 1
    send(
        {
            "jsonrpc": "2.0",
            "id": f"srv-reg-{_next_server_id}",
            "method": "client/registerCapability",
            "params": {"registrations": []},
        }
    )
    _next_server_id += 1
    send(
        {
            "jsonrpc": "2.0",
            "id": f"srv-exec-{_next_server_id}",
            "method": "workspace/executeCommand",
            "params": {"command": "fake.command"},
        }
    )
    send(
        {
            "jsonrpc": "2.0",
            "method": "window/showMessage",
            "params": {"type": 3, "message": "fake notice"},
        }
    )
    send(
        {
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": "file:///fake/checkout/main.rs",
                "diagnostics": [
                    {
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 3},
                        },
                        "severity": 1,
                        "source": "fake-lsp",
                        "message": "fake diagnostic",
                    }
                ],
            },
        }
    )


def flaky_should_crash():
    if not COUNT_FILE:
        return False
    try:
        with open(COUNT_FILE) as handle:
            count = int(handle.read().strip() or "0")
    except (OSError, ValueError):
        count = 0
    count += 1
    with open(COUNT_FILE, "w") as handle:
        handle.write(str(count))
    return count == 1


def main():
    if CASE == "exit_now":
        sys.exit(0)
    if CASE == "bad_framing":
        sys.stdout.buffer.write(b"Content-Length: abc\r\n\r\nnot json at all\r\n")
        sys.stdout.buffer.flush()
        time.sleep(30)
        return
    if CASE == "never_init":
        time.sleep(30)
        return
    if CASE == "stderr_noise":
        for index in range(200):
            sys.stderr.write(f"fake stderr line {index} " + "x" * 100 + "\n")
        sys.stderr.flush()

    initialized = False
    while True:
        message = read_message()
        if message is None:
            return
        method = message.get("method")
        request_id = message.get("id")

        if CASE == "exit_on_initialize" and method == "initialize":
            sys.exit(0)
        if CASE == "flaky_once" and method == "initialize" and flaky_should_crash():
            sys.exit(0)

        if method is not None:
            log({"event": "notify" if request_id is None else "request", "method": method, "params": message.get("params")})
            if method == "initialize":
                initialize(request_id)
                if CASE == "crash_after_init":
                    sys.exit(0)
                if CASE == "oversize_after_init":
                    # One header declaring a body far beyond the client's cap.
                    sys.stdout.buffer.write(b"Content-Length: 20000000\r\n\r\n{")
                    sys.stdout.buffer.flush()
                    time.sleep(30)
                    return
                if CASE in ("ok", "flaky_once"):
                    initialized = True
                    server_requests()
                continue
            if method == "exit":
                sys.exit(0)
            if CASE == "crash_on_notify" and method == "textDocument/didOpen":
                sys.exit(0)
            if request_id is not None:
                if method == HANG_METHOD:
                    continue
                result, error = method_result(method, message.get("params"))
                respond(request_id, result=result, error=error)
        else:
            # A response to one of our server->client requests.
            log({"event": "response", "id": request_id, "result": message.get("result"), "error": message.get("error")})


if __name__ == "__main__":
    main()
