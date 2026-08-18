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
import subprocess
import sys
import time
import traceback

class Delegation(Exception):
    """A child agent was asked something and could not answer."""


def rlm(prompt, name=None, model=None, timeout=None, runner=None, system_prompt=None):
    """Ask a child agent a question and get its answer back, here, as a string.

    This is the recursive half of prompt-as-a-variable. The namespace already
    lets a large *input* stay out of the conversation; `rlm` lets a large
    *sub-task* stay out of it too — the child reads what it needs, thinks, and
    only its conclusion is bound to a variable:

        verdicts = {p: rlm(f"Breaking change in {p}? yes/no + why") for p in paths}
        sum(1 for v in verdicts.values() if v.lower().startswith("yes"))

    It blocks, which is the trade. When the answer is not needed before the
    turn ends, `pa agent spawn` from the shell is the cheaper shape: it admits
    the child and returns immediately, and several can run at once.

    Raises `Delegation` when the child fails, rather than returning its error
    text — an error that reads like an answer is the one thing worse than no
    answer at all.
    """
    binary = os.environ.get("PA_BIN") or "pa"
    argv = [binary, "--json", "agent", "call", str(prompt)]
    for flag, value in (
        ("--name", name),
        ("--model", model),
        ("--with", runner),
        ("--system-prompt", system_prompt),
        ("--timeout", timeout),
    ):
        if value is not None:
            argv += [flag, str(value)]

    try:
        done = subprocess.run(argv, capture_output=True, text=True)
    except OSError as error:
        raise Delegation(f"could not run {binary}: {error}") from None

    try:
        payload = json.loads(done.stdout)
    except ValueError:
        detail = (done.stderr or done.stdout or "").strip()
        raise Delegation(detail or f"{binary} exited {done.returncode}") from None

    if not payload.get("ok"):
        raise Delegation(payload.get("error") or "the child agent failed")
    return payload.get("text", "")



# --- the reducing toolkit ---------------------------------------------------
#
# Why these ship in the namespace rather than being left to the caller:
#
# A kernel only saves anything if the reduction happens *here*. Loading a
# hundred files and then printing them back out costs exactly what reading
# them one at a time would have cost — the namespace added a step and saved
# nothing. That is the easy mistake to make, and it is easy precisely because
# `print(src[path])` is one short line while writing a grep by hand is five.
#
# So the short line is the reducing one. `load` never prints a file, `grep`
# returns matches rather than contents, and `outline` returns shape rather
# than text. Reaching for the whole file is still possible and still
# sometimes right — it is just no longer the path of least resistance.

FILES = {}


def load(pattern, root="."):
    """Read every file matching a glob into `FILES`, printing only the tally.

        load("src/**/*.rs")      # 176 files, 1.2 MB
        grep("fn spawn")         # the four lines that matter

    Binary and unreadable files are skipped and counted, not raised: one
    stray `.png` in a tree must not fail the load of the other 900 files.
    """
    import glob as _glob

    matches = sorted(_glob.glob(os.path.join(root, pattern), recursive=True))
    loaded, skipped, total = 0, 0, 0
    for path in matches:
        if not os.path.isfile(path):
            continue
        try:
            with open(path, "r", encoding="utf-8") as handle:
                text = handle.read()
        except (OSError, UnicodeDecodeError):
            skipped += 1
            continue
        # Keyed by the path as written, not as globbed: `load("crates/**")`
        # putting `./crates/...` in the dict makes every later lookup miss in
        # a way that looks like an empty file rather than a wrong key.
        FILES[os.path.normpath(path)] = text
        loaded += 1
        total += len(text)

    note = f"{loaded} files, {total / 1024:.1f} KB in FILES"
    if skipped:
        note += f" ({skipped} unreadable, skipped)"
    return note


def grep(pattern, where=None, context=0, limit=200, ignore_case=False):
    """Search loaded files (or a dict you pass) and return matching lines.

    Returns `path:line: text` strings — the answer, not the haystack. This is
    the call that replaces reading a file to find one function in it.

    # Why it scans rather than splits

    The obvious implementation splits every file into a list of lines and
    walks it. That allocates the entire corpus on *every call* — 338MB of
    line objects to find twenty-five matches — and measured 6.8x slower than
    this on a 14,000-file tree, slower even than re-reading the whole tree
    from disk with ripgrep.

    So the scan runs over the text as one string, and line numbers are
    computed only where a match actually landed. Files with no match cost one
    regex scan and no allocation at all.
    """
    import re as _re

    source = FILES if where is None else where
    flags = _re.IGNORECASE if ignore_case else 0
    needle = _re.compile(pattern, flags)

    hits = []
    for path, text in source.items():
        for match in needle.finditer(text):
            at = match.start()
            # Counting newlines behind the match beats tracking them forwards:
            # it is paid once per hit, not once per line of the file.
            number = text.count("\n", 0, at) + 1
            start = text.rfind("\n", 0, at) + 1
            end = text.find("\n", at)
            if end == -1:
                end = len(text)

            if context:
                low = number - context
                high = number + context
                lines = text.split("\n")
                block = "\n".join(
                    f"{path}:{n}: {lines[n - 1]}"
                    for n in range(max(1, low), min(len(lines), high) + 1)
                )
                hits.append(block)
            else:
                hits.append(f"{path}:{number}: {text[start:end]}")

            if len(hits) >= limit:
                return hits
    return hits


def _text_of(path_or_text):
    """A loaded path, an unloaded path, or literal text — in that order.

    Falling back to disk matters: being told "that file has no definitions"
    because the path was never loaded is a wrong answer wearing a right
    answer's clothes.
    """
    key = os.path.normpath(path_or_text) if path_or_text else path_or_text
    if key in FILES:
        return FILES[key]
    if "\n" not in path_or_text and os.path.isfile(key):
        with open(key, "r", encoding="utf-8") as handle:
            text = handle.read()
        FILES[key] = text
        return text
    return path_or_text


def outline(path_or_text, kinds=None):
    """The shape of a file — its definitions — rather than its contents.

    Language-agnostic on purpose: it matches the handful of keywords that
    start a definition in most languages, which is enough to decide *where*
    to look without reading everything on the way there.
    """
    import re as _re

    text = _text_of(path_or_text)
    keywords = kinds or [
        "class",
        "def",
        "fn",
        "func",
        "function",
        "impl",
        "trait",
        "struct",
        "enum",
        "interface",
        "type",
        "const",
    ]
    pattern = _re.compile(
        r"^\s*(?:pub\s+|export\s+|async\s+|static\s+)*(" + "|".join(keywords) + r")\s+[\w<>:~]+"
    )
    return [
        f"{number}: {line.strip()}"
        for number, line in enumerate(text.split("\n"), 1)
        if pattern.match(line)
    ]


def head(path, lines=40):
    """The first N lines of a loaded file. For when a peek really is enough."""
    return "\n".join(_text_of(path).split("\n")[:lines])


def slice_lines(path, start, end):
    """Lines `start`..`end` of a loaded file, 1-indexed and inclusive."""
    return "\n".join(_text_of(path).split("\n")[start - 1 : end])



def write(path, text, mkdirs=True):
    """Write a file and refresh its entry in `FILES`.

    The write half of the same bargain: a file written through the kernel is
    a file the namespace still knows about afterwards, so the next `grep`
    sees the new text rather than a stale copy read minutes ago.
    """
    if mkdirs:
        parent = os.path.dirname(os.path.abspath(path))
        if parent:
            os.makedirs(parent, exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    FILES[os.path.normpath(path)] = text
    return f"wrote {len(text)} bytes to {path}"


def edit(path, old, new, count=1):
    """Replace exact text in a file on disk, and in `FILES`.

    Refuses when `old` is absent or ambiguous rather than guessing. A silent
    no-op edit is the worst outcome available here: it reports success while
    leaving the file exactly as it was.
    """
    text = _text_of(path)

    found = text.count(old)
    if found == 0:
        raise ValueError(f"not found in {path}: {old[:60]!r}")
    if count == 1 and found > 1:
        raise ValueError(
            f"{found} matches in {path}; pass count={found} to replace all, "
            "or include more surrounding text to make it unique"
        )

    updated = text.replace(old, new) if count != 1 else text.replace(old, new, 1)
    return write(path, updated)


def sh(command, cwd=None):
    """Run a shell command, returning (exit_code, stdout, stderr).

    Here so that a pipeline of "search, run, read the result" never has to
    leave the namespace and lose what it has already loaded.
    """
    done = subprocess.run(
        command, shell=True, capture_output=True, text=True, cwd=cwd
    )
    return done.returncode, done.stdout, done.stderr


# The namespace. Everything the model binds lives here and nowhere else.
NAMESPACE = {
    "__name__": "__pa_kernel__",
    "__builtins__": __builtins__,
    # Delegation and the reducing toolkit, available without an import.
    # Reserved below so `vars` keeps showing the model's own bindings rather
    # than its plumbing.
    "rlm": rlm,
    "Delegation": Delegation,
    "FILES": FILES,
    "load": load,
    "grep": grep,
    "outline": outline,
    "head": head,
    "slice_lines": slice_lines,
    "_text_of": _text_of,
    "write": write,
    "edit": edit,
    "sh": sh,
    "os": os,
    "json": json,
}

# Names present before any user code runs, so `vars` can hide its own plumbing.
RESERVED = {
    "__name__",
    "__builtins__",
    "__pa_kernel__",
    "rlm",
    "Delegation",
    "FILES",
    "load",
    "grep",
    "outline",
    "head",
    "slice_lines",
    "_text_of",
    "write",
    "edit",
    "sh",
    "os",
    "json",
}


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
        # FILES is reserved so the toolkit survives, but its *contents* are
        # the caller's data and "empty the namespace" has to mean it.
        FILES.clear()
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
