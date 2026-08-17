#!/usr/bin/env python3
"""The persistent Python namespace.

One process, one dict, one Unix socket. The Rust host spawns this, then every
`pa kernel exec` connects, sends a single JSON request and reads a single JSON
response. The dict outlives all of them, which is the entire point: a 200MB log
is loaded once and queried a hundred times, and only the answers ever reach the
conversation.

Deliberately dependency-free — standard library only. A kernel that needs a
package manager to start is a kernel that fails on a fresh machine.
"""

import argparse
import ast
import contextlib
import io
import json
import os
import pickle
import signal
import socket
import sys
import time
import traceback

# The namespace. Everything the model binds lives here and nowhere else.
NAMESPACE = {"__name__": "__pa_kernel__", "__builtins__": __builtins__}

# Names present before any user code runs, so `vars` can hide its own plumbing.
RESERVED = {"__name__", "__builtins__", "__pa_kernel__"}


class Timeout(Exception):
    pass


def _on_alarm(signum, frame):
    raise Timeout()


def summarise(name, value):
    """Shape without contents.

    `vars` exists so the model can decide what to slice next. Printing the value
    would defeat the feature it is part of, so this returns a length, a size and
    a deliberately short preview.
    """
    info = {
        "name": name,
        "type_name": type(value).__name__,
        "length": None,
        "size_bytes": None,
        "preview": None,
    }

    try:
        info["length"] = len(value)
    except (TypeError, AttributeError):
        pass

    try:
        info["size_bytes"] = sys.getsizeof(value)
    except (TypeError, AttributeError):
        pass

    try:
        text = repr(value)
        info["preview"] = text if len(text) <= 96 else text[:93] + "..."
    except Exception:
        info["preview"] = "<unreprable>"

    return info


def execute(code, timeout):
    """Runs code in the namespace, returning stdout, stderr and a last value.

    The last statement is evaluated separately when it is an expression, so
    `rows[0]` prints its value the way a REPL would rather than silently
    returning nothing.
    """
    stdout, stderr = io.StringIO(), io.StringIO()
    started = time.time()
    result = None
    error = None
    timed_out = False

    previous = signal.signal(signal.SIGALRM, _on_alarm)
    signal.alarm(max(1, int(timeout)))

    try:
        tree = ast.parse(code)
        tail = None
        if tree.body and isinstance(tree.body[-1], ast.Expr):
            tail = ast.Expression(tree.body.pop().value)

        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            if tree.body:
                exec(compile(tree, "<pa>", "exec"), NAMESPACE)
            if tail is not None:
                value = eval(compile(tail, "<pa>", "eval"), NAMESPACE)
                if value is not None:
                    result = repr(value)
    except Timeout:
        timed_out = True
        error = "timed out after %ss" % timeout
    except SystemExit:
        # A stray sys.exit() in model-generated code must not take the kernel
        # down with it; the namespace is worth more than the call.
        error = "sys.exit() ignored inside the kernel"
    except BaseException:
        error = traceback.format_exc(limit=12).strip()
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)

    return {
        "ok": error is None,
        "stdout": stdout.getvalue(),
        "stderr": stderr.getvalue(),
        "result": result,
        "error": error,
        "duration_ms": int((time.time() - started) * 1000),
        "timed_out": timed_out,
    }


def snapshot(path):
    """Pickles what can be pickled, and names what cannot.

    A partial snapshot that says what it dropped is far more useful than a
    refusal: most namespaces hold one open file handle and a hundred perfectly
    serialisable results.
    """
    saved, skipped = {}, []
    for name, value in NAMESPACE.items():
        if name in RESERVED:
            continue
        try:
            pickle.dumps(value)
            saved[name] = value
        except Exception:
            skipped.append(name)

    with open(path, "wb") as handle:
        pickle.dump(saved, handle, protocol=pickle.HIGHEST_PROTOCOL)

    return {"ok": True, "saved": sorted(saved), "skipped": sorted(skipped), "path": path}


def restore(path):
    if not os.path.exists(path):
        return {"ok": False, "error": "no snapshot at %s" % path, "restored": []}
    with open(path, "rb") as handle:
        NAMESPACE.update(pickle.load(handle))
    return {"ok": True, "restored": sorted(k for k in NAMESPACE if k not in RESERVED)}


def handle(request):
    op = request.get("op", "")

    if op == "ping":
        return {"ok": True, "pid": os.getpid()}

    if op == "exec":
        return execute(request.get("code", ""), request.get("timeout", 120))

    if op == "vars":
        return {
            "ok": True,
            "vars": [
                summarise(name, value)
                for name, value in NAMESPACE.items()
                if name not in RESERVED and not name.startswith("__")
            ],
        }

    if op == "reset":
        for name in [k for k in NAMESPACE if k not in RESERVED]:
            del NAMESPACE[name]
        return {"ok": True}

    if op == "snapshot":
        return snapshot(request["path"])

    if op == "restore":
        return restore(request["path"])

    if op == "shutdown":
        return {"ok": True}

    return {"ok": False, "error": "unknown op '%s'" % op}


def serve(sock_path, idle_seconds):
    if os.path.exists(sock_path):
        os.unlink(sock_path)

    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(sock_path)
    server.listen(16)
    server.settimeout(idle_seconds)
    os.chmod(sock_path, 0o600)

    try:
        while True:
            try:
                conn, _ = server.accept()
            except socket.timeout:
                # Nobody has used this namespace in a long time. Exiting frees
                # the memory; the next exec starts a fresh kernel.
                break

            with conn, conn.makefile("rwb") as stream:
                line = stream.readline()
                if not line:
                    continue
                try:
                    request = json.loads(line.decode("utf-8"))
                    response = handle(request)
                except Exception:
                    request = {}
                    response = {"ok": False, "error": traceback.format_exc(limit=6)}

                stream.write(json.dumps(response).encode("utf-8") + b"\n")
                stream.flush()

            if request.get("op") == "shutdown":
                break
    finally:
        server.close()
        with contextlib.suppress(OSError):
            os.unlink(sock_path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--idle", type=int, default=3600)
    args = parser.parse_args()
    serve(args.socket, args.idle)


if __name__ == "__main__":
    main()
