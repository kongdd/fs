#!/usr/bin/env python3
"""Rank folders using the nasfind databases selected by a TOML config."""
import argparse
from collections import Counter
import heapq
import os
from pathlib import Path
import shutil
import subprocess
import sys


def positive_int(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return number


def count_directories(paths):
    """Count each unique indexed path under all its ancestor directories."""
    counts = Counter()
    parents = {}
    seen = set()
    accepted = 0
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        accepted += 1
        parent = os.path.dirname(path)
        if not parent or parent == os.path.dirname(parent):
            continue
        counts[parent] += 1
        # Register each directory's ancestors once, not once per indexed path.
        while parent not in parents:
            ancestor = os.path.dirname(parent)
            if not ancestor or ancestor == os.path.dirname(ancestor):
                parents[parent] = None
                break
            parents[parent] = ancestor
            parent = ancestor
    # Aggregate children before parents; directory records already count as an
    # entry in their parent, so do not add an extra entry during propagation.
    for directory in sorted(parents, key=lambda p: p.count(os.sep), reverse=True):
        parent = parents[directory]
        if parent is not None:
            counts[parent] += counts[directory]
    return counts, accepted


def nul_paths(stream):
    """Decode raw NUL-delimited paths without buffering the entire query."""
    pending = b""
    while True:
        chunk = stream.read(65536)
        if not chunk:
            break
        records = (pending + chunk).split(b"\0")
        pending = records.pop()
        for record in records:
            if record:
                yield os.fsdecode(record)
    if pending:
        yield os.fsdecode(pending)


def subtree_pattern(root):
    # plocate treats patterns containing glob characters as whole-path globs.
    escaped = "".join("\\" + c if c in "*?[\\" else c for c in root)
    return escaped.rstrip(os.sep) + os.sep + "*"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", help="restrict ranking to folders inside this path")
    parser.add_argument("-c", "--config", help="nasfind config TOML (default: nasfind's normal config lookup)")
    parser.add_argument("-n", "--top", type=positive_int, default=10, help="number of results (default: 10; accepts -n10)")
    parser.add_argument("-d", "--index", action="append", default=[], help="select a configured index; repeatable")
    parser.add_argument("--nasfind", default=shutil.which("nasfind") or str(Path.home() / ".local/bin/nasfind"), help="nasfind executable")
    args = parser.parse_args()
    command = [args.nasfind]
    if args.config:
        command.extend(["--config", args.config])
    command.extend(["search", "-0"])
    for index in args.index:
        command.extend(["-d", index])
    root = None
    if args.path:
        root = os.path.abspath(os.path.expanduser(args.path)).rstrip(os.sep) or os.sep
    command.extend(["--", subtree_pattern(root) if root else "/"])
    env = os.environ.copy()
    env["PATH"] = str(Path.home() / ".local/bin") + os.pathsep + env.get("PATH", "")
    try:
        # nasfind applies query-time filters; consume records while it runs.
        with subprocess.Popen(command, stdout=subprocess.PIPE, env=env) as process:
            counts, total = count_directories(nul_paths(process.stdout))
            status = process.wait()
            if status:
                raise subprocess.CalledProcessError(status, command)
    except (OSError, subprocess.CalledProcessError) as error:
        print("error: {}".format(error), file=sys.stderr)
        return 1
    if root is not None:
        prefix = root.rstrip(os.sep) + os.sep
        counts = {p: n for p, n in counts.items() if p == root or p.startswith(prefix)}
    else:
        # Omit common ancestors above all returned records, which dominate every
        # ranking but convey no distribution information. Use PATH to include one.
        if counts:
            largest = max(counts.values())
            counts = {p: n for p, n in counts.items() if n < largest}
    rows = heapq.nsmallest(args.top, counts.items(), key=lambda item: (-item[1], item[0]))
    print("{:>12}  {}".format("ENTRIES", "DIRECTORY"))
    for path, count in rows:
        print("{:>12,}  {}".format(count, path))
    print("Total: {:,} unique indexed paths (including directories; config filters applied). Recursive counts overlap between parent and child folders.".format(total), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
