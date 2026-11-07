#!/usr/bin/env python3
"""Fake ACP agent for the acp.rs tests.

Speaks newline-delimited JSON-RPC 2.0 on stdio. Behavior is selected by the
FAKE_ACP_CASE environment variable. When FAKE_ACP_LOG names a file, one JSON
event per line is appended to it (session_new, prompt with the prompt text,
session_close, session_cancel, agent_response); FAKE_ACP_PIDFILE receives the
process id at startup, and FAKE_ACP_CHILD_PIDFILE receives the grandchild's
pid for the with_grandchild case.
"""

import json
import os
import subprocess
import sys

PIDFILE = os.environ.get("FAKE_ACP_PIDFILE")
if PIDFILE:
    with open(PIDFILE, "w") as handle:
        handle.write(str(os.getpid()))

LOG = os.environ.get("FAKE_ACP_LOG")
CASE = os.environ.get("FAKE_ACP_CASE", "ok")

CHILD_PIDFILE = os.environ.get("FAKE_ACP_CHILD_PIDFILE")
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

_sessions = 0


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def log(event):
    if LOG:
        with open(LOG, "a") as handle:
            handle.write(json.dumps(event) + "\n")


def noise():
    if CASE == "noise":
        sys.stdout.write("this is not json\n{also not json\n")
        sys.stdout.flush()


def initialize(request_id):
    result = {
        "protocolVersion": 1,
        "agentCapabilities": {"sessionCapabilities": {"close": {}}},
        "authMethods": [],
    }
    if CASE == "auth_fail":
        result["authMethods"] = [
            {"id": "login", "name": "Login with fake", "description": "fake login"}
        ]
    send({"jsonrpc": "2.0", "id": request_id, "result": result})
    if CASE == "crash_after_init":
        sys.exit(0)


def new_session(request_id, params):
    global _sessions
    if CASE == "auth_fail":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32000, "message": "authentication required"},
            }
        )
        return
    _sessions += 1
    session_id = "sess-%d" % _sessions
    log({"event": "session_new", "sessionId": session_id, "cwd": params.get("cwd")})
    send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": session_id}})


def chunks_for(case):
    if case == "empty":
        return []
    if case == "fenced":
        return ["```\nfeat: ", "demo\n```"]
    if case == "refusal":
        return ["I cannot help with that."]
    return ["feat: ", "demo"]


def stop_for(case):
    return "refusal" if case == "refusal" else "end_turn"


def update(session_id, text):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": text},
                },
            },
        }
    )


def finish_prompt(request_id, session_id, case):
    for chunk in chunks_for(case):
        noise()
        update(session_id, chunk)
    noise()
    send({"jsonrpc": "2.0", "id": request_id, "result": {"stopReason": stop_for(case)}})


def ask_agent(session_id):
    send(
        {
            "jsonrpc": "2.0",
            "id": 9001,
            "method": "session/request_permission",
            "params": {"sessionId": session_id, "options": []},
        }
    )
    send(
        {
            "jsonrpc": "2.0",
            "id": 9002,
            "method": "fs/read_text_file",
            "params": {"sessionId": session_id, "path": "x"},
        }
    )


def prompt(request_id, params):
    session_id = params.get("sessionId")
    text = "".join(
        str(part.get("text", "")) for part in params.get("prompt") or []
    )
    log({"event": "prompt", "sessionId": session_id, "text": text})
    if CASE == "hang_prompt":
        return
    if CASE == "perm":
        ask_agent(session_id)
        # Wait for the client's answers, then finish the turn normally.
        seen = set()
        while len(seen) < 2:
            line = sys.stdin.readline()
            if not line:
                return
            try:
                incoming = json.loads(line)
            except ValueError:
                continue
            if "method" not in incoming and "id" in incoming:
                seen.add(incoming["id"])
                log({"event": "agent_response", "response": incoming})
        finish_prompt(request_id, session_id, "ok")
        return
    finish_prompt(request_id, session_id, CASE)


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except ValueError:
            continue
        method = message.get("method")
        request_id = message.get("id")
        if method is None and request_id is not None:
            # A response to one of the agent's own requests.
            log({"event": "agent_response", "response": message})
            continue
        params = message.get("params") or {}
        if method == "initialize":
            if CASE != "no_init":
                initialize(request_id)
        elif method == "session/new":
            new_session(request_id, params)
        elif method == "session/prompt":
            prompt(request_id, params)
        elif method == "session/close":
            log({"event": "session_close", "sessionId": params.get("sessionId")})
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        elif method == "session/cancel":
            log({"event": "session_cancel", "sessionId": params.get("sessionId")})
        noise()


if __name__ == "__main__":
    main()
