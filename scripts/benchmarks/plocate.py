#!/usr/bin/env python3
"""Measure real fs workloads without modifying NAS files or clearing caches."""
import argparse
import json
import os
import signal
import platform
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def measure(command, timeout, count_paths=False):
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        start = time.perf_counter()
        process = subprocess.Popen(command, stdout=output, stderr=errors, start_new_session=True)
        def overdue(_signum, _frame):
            raise subprocess.TimeoutExpired(command, timeout)
        previous = signal.signal(signal.SIGALRM, overdue)
        signal.setitimer(signal.ITIMER_REAL, timeout)
        try:
            process.wait()
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            raise
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, previous)
        elapsed = time.perf_counter() - start
        if process.returncode:
            errors.seek(0)
            raise RuntimeError(errors.read(4096).decode(errors="replace"))
        matches = None
        if count_paths:
            output.seek(0)
            matches = sum(chunk.count(b"\0") for chunk in iter(lambda: output.read(65536), b""))
        return {"seconds": elapsed, "matches": matches}


def percentile(samples, fraction):
    values = sorted(samples)
    position = (len(values) - 1) * fraction
    lower = int(position)
    upper = min(lower + 1, len(values) - 1)
    return values[lower] + (values[upper] - values[lower]) * (position - lower)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fs", default="fs")
    parser.add_argument("--engine", choices=["plocate", "rust"], default="plocate")
    parser.add_argument("--config", required=True)
    parser.add_argument("--index", action="append", default=[], help="Select a named DB; repeat as needed")
    parser.add_argument("--query", action="append", required=True, help="A single search pattern; repeat for different workloads")
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--limit", type=int, default=50, help="Result limit; 0 measures all matches")
    parser.add_argument("--update", action="store_true", help="Time two normal updates (writes configured DBs)")
    parser.add_argument("--folder", action="append", default=[], help="Also time a partial update of this folder (writes its DB)")
    parser.add_argument("--timeout", type=float, default=3600)
    parser.add_argument("--label", default="NAS", help="Describe device, DB storage and execution location")
    parser.add_argument("--output", default="fs-benchmark.json")
    args = parser.parse_args()
    if args.runs < 2 or args.limit < 0 or args.timeout <= 0:
        parser.error("runs must be >= 2, limit >= 0, timeout > 0")
    command = [args.fs, "--config", str(Path(args.config).resolve())]
    report = {
        "label": args.label,
        "platform": platform.platform(),
        "machine": platform.machine(),
        "version": subprocess.check_output([args.fs, "--version"], text=True).strip(),
        "config": str(Path(args.config).resolve()),
        "indexes": args.index,
        "limit": args.limit,
        "cache_note": "No caches cleared; first query is not necessarily cold. Output is written to a temporary file.",
        "updates": [],
        "queries": [],
    }
    try:
        if args.update:
            for number in (1, 2):
                print("Timing normal update {}...".format(number), flush=True)
                sample = measure(command + ["updatedb", "--engine", args.engine, "--no-progress"] + args.index, args.timeout)
                report["updates"].append({"kind": "normal", "run": number, **sample})
                print("  {:.3f} s".format(sample["seconds"]), flush=True)
        for folder in args.folder:
            print("Timing partial update: {}...".format(folder), flush=True)
            sample = measure(command + ["updatedb", "--engine", args.engine, "--folder", folder, "--no-progress"], args.timeout)
            report["updates"].append({"kind": "partial", "folder": folder, **sample})
            print("  {:.3f} s (includes statistics refresh; legacy plocate also rebuilds the main DB)".format(sample["seconds"]), flush=True)
        for query in args.query:
            search = command + ["locate", "-0"]
            for index in args.index:
                search += ["-d", index]
            if args.limit:
                search += ["-n", str(args.limit)]
            search += ["--", query]
            samples = [measure(search, args.timeout, True) for _ in range(args.runs)]
            repeated = [sample["seconds"] for sample in samples[1:]]
            row = {"pattern": query, "samples": samples, "first_ms": samples[0]["seconds"] * 1000,
                   "repeated_p50_ms": statistics.median(repeated) * 1000,
                   "repeated_p95_ms": percentile(repeated, 0.95) * 1000}
            report["queries"].append(row)
            print("{!r}: first {:.2f} ms; repeated P50 {:.2f} ms, P95 {:.2f} ms; {} matches".format(
                query, row["first_ms"], row["repeated_p50_ms"], row["repeated_p95_ms"], samples[-1]["matches"]), flush=True)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        raise
    finally:
        Path(args.output).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print("Report: {}".format(Path(args.output).resolve()), flush=True)


if __name__ == "__main__":
    main()
