#!/usr/bin/env python3
"""Compare unchanged full Rust updates using isolated copies of outdir databases."""
import argparse
import hashlib
import json
import pathlib
import re
import sqlite3
import statistics
import subprocess
import tempfile
import time


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return (path.stat().st_mtime_ns, digest.hexdigest())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True)
    parser.add_argument('--before', required=True)
    parser.add_argument('--after', default='target/release/fs')
    parser.add_argument('--runs', type=int, default=7)
    parser.add_argument('--warmup', type=int, default=2)
    parser.add_argument('--jobs', type=int, nargs='+', default=[1, 4])
    parser.add_argument('--parent', default=str(pathlib.Path.home() / '.cache'))
    parser.add_argument('--output', default='build/update-benchmark/unchanged.json')
    args = parser.parse_args()
    if args.runs < 1 or args.warmup < 1 or any(not 1 <= jobs <= 256 for jobs in args.jobs):
        parser.error('runs/warmup must be positive; jobs must be 1..256')
    text = pathlib.Path(args.config).read_text()
    # Keep arbitrary database overrides from pointing at production files.
    if re.search(r'(?m)^\s*(?:database|update_database|search_database)\s*=', text):
        parser.error('this benchmark requires outdir-only database paths (no per-index overrides)')
    match = re.search(r'(?m)^outdir\s*=\s*("(?:[^"\\]|\\.)*")\s*$', text.partition('\n[')[0])
    if not match:
        parser.error('config must have a top-level double-quoted outdir')
    source_dir = pathlib.Path(json.loads(match.group(1))).expanduser().resolve()
    paths = sorted(path for path in source_dir.glob('*.db') if path.name != 'stats.db')
    if not paths:
        parser.error('no databases found in outdir')
    names = [path.stem for path in paths]
    originals = {str(path): fingerprint(path) for path in paths}
    binaries = {'before': str(pathlib.Path(args.before).resolve(strict=True)),
                'after': str(pathlib.Path(args.after).resolve(strict=True))}
    parent = pathlib.Path(args.parent).resolve()
    parent.mkdir(parents=True, exist_ok=True)
    results = {'indexes': names, 'directory_counts': {}, 'jobs': {},
               'note': 'Warm-cache full updates; source databases are never updated.'}
    with tempfile.TemporaryDirectory(prefix='fs-unchanged-', dir=parent) as temporary:
        base = pathlib.Path(temporary)
        configs = {}
        for kind in binaries:
            outdir = base / kind
            outdir.mkdir()
            for path in paths:
                source = sqlite3.connect(path.as_uri() + '?mode=ro', uri=True)
                try:
                    assert source.execute('PRAGMA application_id').fetchone()[0] == 0x4e465231, path
                    root = pathlib.Path(source.execute("SELECT value FROM meta WHERE key='root'").fetchone()[0].decode())
                    # The benchmark workspace must not create changes under a scanned root.
                    assert root != parent and root not in parent.parents, (root, parent)
                    results['directory_counts'][path.stem] = source.execute('SELECT COUNT(*) FROM directories').fetchone()[0]
                    destination = sqlite3.connect(str(outdir / path.name))
                    try:
                        source.backup(destination)
                    finally:
                        destination.close()
                finally:
                    source.close()
            config = base / (kind + '.toml')
            config.write_text(text[:match.start(1)] + json.dumps(str(outdir)) + text[match.end(1):])
            configs[kind] = config

        def run(kind, jobs, measured):
            start = time.perf_counter()
            completed = subprocess.run(
                [binaries[kind], '-c', str(configs[kind]), 'updatedb', '--engine', 'rust',
                 '-j' + str(jobs), '--no-progress', *names],
                capture_output=True, timeout=300)
            elapsed = time.perf_counter() - start
            output = completed.stderr.decode(errors='replace')
            assert completed.returncode == 0, output
            if measured:
                assert len(re.findall(r'0 dirs scanned,', output)) == len(names), output
                assert 'building stats cache' not in output, output
                for path, before in snapshots[kind].items():
                    assert fingerprint(path) == before, ('no-op wrote a database', path)
            timings = re.findall(r'(\S+) timings: scan ([\d.]+)s · index ([\d.]+)s · finalize ([\d.]+)s', output)
            return {'seconds': elapsed, 'indexes': {
                name: {'scan': float(scan), 'index': float(index), 'finalize': float(finalize)}
                for name, scan, index, finalize in timings}}

        # The first pass can legitimately catch changes since the source DBs
        # were built and initialize the isolated stats caches. Never time it.
        for kind in binaries:
            run(kind, max(args.jobs), False)
        snapshots = {kind: {path: fingerprint(path) for path in (base / kind).glob('*.db')}
                     for kind in binaries}
        for jobs in args.jobs:
            for _ in range(args.warmup):
                for kind in binaries:
                    run(kind, jobs, True)
            samples = {kind: [] for kind in binaries}
            for iteration in range(args.runs):
                order = ('before', 'after') if iteration % 2 == 0 else ('after', 'before')
                for kind in order:
                    sample = run(kind, jobs, True)
                    samples[kind].append(sample)
                    print('j{} {} {}: {:.3f}s'.format(jobs, iteration, kind, sample['seconds']), flush=True)
            medians = {kind: statistics.median(sample['seconds'] for sample in values)
                       for kind, values in samples.items()}
            speedup = medians['before'] / medians['after']
            results['jobs'][str(jobs)] = {'medians_seconds': medians, 'speedup': speedup, 'samples': samples}
            print('j{} median: {:.3f}s -> {:.3f}s ({:.2f}x)'.format(
                jobs, medians['before'], medians['after'], speedup), flush=True)
    assert originals == {str(path): fingerprint(path) for path in paths}, 'source databases changed during benchmark'
    output = pathlib.Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(results, ensure_ascii=False, indent=2) + '\n')
    print('Report:', output)


if __name__ == '__main__':
    main()
