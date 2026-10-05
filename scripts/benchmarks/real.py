#!/usr/bin/env python3
"""Read-only real-directory benchmark; all indexes use isolated NAS storage."""
import argparse
import collections
import fnmatch
import hashlib
import importlib.util
import json
import os
import pathlib
import platform
import shutil
import statistics
import subprocess
import tempfile
import time
from datetime import datetime, timezone

spec = importlib.util.spec_from_file_location('native_bench', pathlib.Path(__file__).with_name('native.py'))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def snapshot(root, exclusions):
    """Byte-safe inventory, without following directory symlinks."""
    paths = {os.fsencode(root)}
    directories = 0
    pruned = set()

    def fail(error):
        raise error

    for directory, dirs, files in os.walk(os.fsencode(root), followlinks=False, onerror=fail):
        blocked = {name for name in dirs if name in exclusions
                   and not os.path.islink(os.path.join(directory, name))}
        pruned.update(os.path.join(directory, name) for name in blocked)
        dirs[:] = [name for name in dirs if name not in blocked]
        directories += 1
        paths.update(os.path.join(directory, name) for name in dirs + files)
    return paths, directories, pruned


def check(data, expected, label, limit=None):
    if data and not data.endswith(b'\0'):
        raise AssertionError((label, 'incomplete NUL output'))
    found = data.split(b'\0')[:-1]
    unique = set(found)
    if len(found) != len(unique):
        raise AssertionError((label, 'duplicate results'))
    correct = unique == expected if limit is None else len(found) == min(limit, len(expected)) and unique <= expected
    if not correct:
        raise AssertionError((label, len(found), len(expected),
                              'missing', sorted(expected - unique)[:5],
                              'unexpected', sorted(unique - expected)[:5]))


def distribution(samples):
    milliseconds = [sample['seconds'] * 1000 for sample in samples]
    return {'p50_ms': statistics.median(milliseconds),
            'p95_ms': bench.percentile(milliseconds, .95),
            'max_peak_rss_kib': max(sample['peak_rss_kib'] or 0 for sample in samples) or None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True)
    parser.add_argument('--fs', default='target/release/fs')
    parser.add_argument('--before', help='Compare saved Rust executable against --fs on the same native DB')
    parser.add_argument('--with-plocate', action='store_true', help='Also query plocate in the same --before run (three-way comparison)')
    parser.add_argument('--parent', default=str(pathlib.Path.cwd()), help='Index storage, outside --root; prefer the same filesystem')
    parser.add_argument('--exclude-dir', action='append', default=None, help='Default: @eaDir only; .git/dependencies/builds remain included')
    parser.add_argument('--init-runs', type=int, default=3)
    parser.add_argument('--update-runs', type=int, default=5)
    parser.add_argument('--runs', type=int, default=21, help='Repeated query samples, excluding first and warmups')
    parser.add_argument('--warmup', type=int, default=3)
    parser.add_argument('--native-only', action='store_true')
    parser.add_argument('--query', action='append', choices=['rare', 'common', 'soil', 'and', 'or', 'missing', 'short', 'glob', 'extension', 'path', 'limited', 'all', 'negation', 'short_limited', 'regex_scan'], help='Run only these query labels (repeatable); default: all')
    parser.add_argument('--output', default='docs/updatedb/real-benchmark.json.txt')
    args = parser.parse_args()
    if min(args.init_runs, args.update_runs, args.runs) < 1 or args.warmup < 0:
        parser.error('run counts must be positive, warmup must be nonnegative')
    if args.with_plocate and (not args.before or args.native_only):
        parser.error('--with-plocate requires --before and cannot be combined with --native-only')
    root = pathlib.Path(args.root).resolve(strict=True)
    parent = pathlib.Path(args.parent).resolve(strict=True)
    output = pathlib.Path(args.output).resolve()
    if not root.is_dir() or not parent.is_dir():
        parser.error('root and parent must be directories')
    if parent == root or root in parent.parents or output == root or root in output.parents:
        parser.error('storage and report must be outside the scanned root')
    exclusions = args.exclude_dir if args.exclude_dir is not None else ['@eaDir']
    if any('/' in name or not name for name in exclusions):
        parser.error('exclude-dir must be a nonempty basename')
    binary = str(pathlib.Path(args.fs).resolve(strict=True))
    if pathlib.Path('/usr/bin/time').exists() and b'GNU Time' in subprocess.run(
            ['/usr/bin/time', '--version'], capture_output=True).stdout:
        bench.GNU_TIME = '/usr/bin/time'
    before = str(pathlib.Path(args.before).resolve(strict=True)) if args.before else None
    engines = ['before', 'after'] if before else (['rust'] if args.native_only else ['rust', 'plocate'])
    if args.with_plocate:
        engines.append('plocate')
    def engine_order(trial):
        if len(engines) == 2:
            return engines if trial % 2 == 0 else list(reversed(engines))
        start = trial % len(engines)
        order = engines[start:] + engines[:start]
        return order if (trial // len(engines)) % 2 == 0 else list(reversed(order))

    binaries = {engine: before if engine == 'before' else binary for engine in engines}
    backends = {engine: 'plocate' if engine == 'plocate' else 'rust' for engine in engines}
    tools = {name: shutil.which(name) for name in ['plocate', 'updatedb']}
    if ((not before and not args.native_only) or args.with_plocate) and not all(tools.values()):
        parser.error('plocate and updatedb required for comparison; otherwise use --native-only')
    report = {'started_utc': datetime.now(timezone.utc).isoformat(),
              'root': str(root), 'storage_parent': str(parent), 'exclude_dirs': exclusions,
              'platform': platform.platform(), 'cpu_count': os.cpu_count(),
              'load_start': os.getloadavg(), 'binary': binary,
              'binary_sha256': hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),
              'version': subprocess.check_output([binary, '--version'], text=True).strip(),
              'tools': tools, 'rss_tool': bench.GNU_TIME,
              'init_runs': args.init_runs, 'update_runs': args.update_runs,
              'query_runs': args.runs, 'warmups': args.warmup,
              'query_order': 'Two engines alternate; three engines rotate through all six permutations.',
              'note': 'Read-only source. No caches cleared; inventory warms directory metadata before builds. Fresh DB each init, alternating engine order. CLI timings include process startup, output to a temporary file, GNU time and stats refresh for index commands. Queries use default Everything ASCII-insensitive basename semantics (path: explicitly uses full path), except regex_scan which uses --regex with default basename scope. P50/P95 exclude first query and warmups. Source inventory verified again at end. No changed-directory update test: source is never modified. RSS is GNU time peak, not summed process-tree RSS.',
              'engines': {engine: {'init': [], 'unchanged_update': [], 'queries': {}} for engine in engines}}
    if before:
        report['before_binary_sha256'] = hashlib.sha256(pathlib.Path(before).read_bytes()).hexdigest()
        report['note'] += ' Before/after queries share the final after native DB; both versions verify its full inventory.'
    if pathlib.Path('/proc/cpuinfo').exists():
        report['cpu_model'] = next((line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), None)

    def save():
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2, ensure_ascii=True) + '\n')

    start = time.perf_counter()
    expected, directories, pruned = snapshot(root, {os.fsencode(name) for name in exclusions})
    universes = {engine: expected if backends[engine] == 'rust'
                 else (expected - {os.fsencode(root)}) | pruned for engine in engines}
    report['index_semantics'] = 'Rust includes root and omits pruned directory markers; updatedb omits root but retains pruned directory markers. Exact per-engine inventories are validated, not silently normalized.'
    report['inventory'] = {'entries': len(expected), 'directories': directories,
                           'non_directory_entries': len(expected) - directories,
                           'pruned_directory_markers': len(pruned),
                           'engine_entries': {engine: len(universes[engine]) for engine in engines},
                           'seconds': time.perf_counter() - start,
                           'sha256': hashlib.sha256(b'\0'.join(sorted(expected))).hexdigest()}
    print('inventory', report['inventory'], flush=True)
    save()
    with tempfile.TemporaryDirectory(prefix='fs-real-bench-', dir=str(parent)) as temporary:
        base = pathlib.Path(temporary)
        commands = {}
        databases = {}
        for trial in range(args.init_runs):
            for engine in engine_order(trial):
                storage = base / engine
                if storage.exists():
                    shutil.rmtree(storage)
                storage.mkdir()
                database = storage / 'index.db'
                config = storage / 'config.toml'
                config.write_text('[filters]\nexclude_dirs=' + json.dumps(exclusions) +
                                  '\n[[index]]\nname="bench"\nroot=' + json.dumps(str(root)) +
                                  '\ndatabase=' + json.dumps(str(database)) + '\n')
                command = [binaries[engine], '-c', str(config)]
                commands[engine], databases[engine] = command, database
                sample, _ = bench.measure(command + ['index', 'update', '--engine', backends[engine], '--no-progress'])
                sample.update({'trial': trial, 'index_bytes': database.stat().st_size,
                               'stats_bytes': database.with_name('stats.db').stat().st_size})
                data = subprocess.check_output(command + ['search', '-0', '*'])
                check(data, universes[engine], (engine, 'init', trial))
                report['engines'][engine]['init'].append(sample)
                print(engine, 'init', sample, flush=True)
                save()
        for trial in range(args.update_runs):
            for engine in engine_order(trial):
                sample, _ = bench.measure(commands[engine] + ['index', 'update', '--engine', backends[engine], '--no-progress'])
                report['engines'][engine]['unchanged_update'].append(sample)
                print(engine, 'unchanged_update', trial, sample, flush=True)
            save()
        if before:
            # Query both executables against exactly the same persisted index.
            commands['before'] = [before, '-c', commands['after'][2]]
            for engine in ['before', 'after']:
                check(subprocess.check_output(commands[engine] + ['search', '-0', '*']),
                      universes[engine], (engine, 'shared native DB'))
        names = {path: path.rsplit(b'/', 1)[-1].lower() for path in expected}
        frequencies = collections.Counter(names.values())
        rare = next((name for name in sorted(frequencies) if frequencies[name] == 1 and len(name) >= 12
                     and all(c in b'abcdefghijklmnopqrstuvwxyz0123456789_.-' for c in name)), None)
        queries = [
            ('rare', rare.decode() if rare else '', lambda p, n: rare in n, None),
            ('common', 'readme', lambda p, n: b'readme' in n, None),
            ('soil', 'soil', lambda p, n: b'soil' in n, None),
            ('and', 'readme md', lambda p, n: b'readme' in n and b'md' in n, None),
            ('or', 'soil | rain', lambda p, n: b'soil' in n or b'rain' in n, None),
            ('missing', 'fs_missing_7e39af3b', lambda p, n: b'fs_missing_7e39af3b' in n, None),
            ('short', 'py', lambda p, n: b'py' in n, None),
            ('glob', '*.csv', lambda p, n: fnmatch.fnmatchcase(n, b'*.csv'), None),
            ('extension', 'ext:py', lambda p, n: n.endswith(b'.py'), None),
            ('path', 'path:USGS', lambda p, n: b'usgs' in p.lower(), None),
            ('limited', 'readme', lambda p, n: b'readme' in n, 50),
            ('all', '*', lambda p, n: True, None),
            ('negation', '!py', lambda p, n: b'py' not in n, None),
            ('short_limited', 'py', lambda p, n: b'py' in n, 50),
            ('regex_scan', '.*py.*', lambda p, n: b'py' in n, None),
        ]
        if rare is None:
            queries = [query for query in queries if query[0] != 'rare']
        if args.query:
            queries = [query for query in queries if query[0] in args.query]
        report['query_labels'] = [query[0] for query in queries]
        time.sleep(3)
        for label, expression, predicate, limit in queries:
            oracles = {engine: {path for path in universes[engine]
                                if predicate(path, path.rsplit(b'/', 1)[-1].lower())}
                       for engine in engines}

            def query(engine):
                extra = ['-n', str(limit)] if limit is not None else []
                if label == 'regex_scan':
                    extra.append('--regex')
                sample, data = bench.measure(commands[engine] + ['search', '-0'] + extra + [expression])
                check(data, oracles[engine], (engine, label), limit)
                return sample

            first = {engine: query(engine) for engine in engines}
            warmups = {engine: [] for engine in engines}
            for trial in range(args.warmup):
                for engine in engine_order(trial):
                    warmups[engine].append(query(engine))
            samples = {engine: [] for engine in engines}
            for trial in range(args.runs):
                for engine in engine_order(trial):
                    samples[engine].append(query(engine))
            for engine in engines:
                oracle = oracles[engine]
                row = {'expression': expression, 'regex': label == 'regex_scan', 'limit': limit,
                       'matches': min(limit, len(oracle)) if limit else len(oracle),
                       'first_sample': first[engine], 'samples': samples[engine],
                       'warmup_samples': warmups[engine], **distribution(samples[engine])}
                report['engines'][engine]['queries'][label] = row
                print(engine, label, 'P50=%.2fms P95=%.2fms matches=%d' % (row['p50_ms'], row['p95_ms'], row['matches']), flush=True)
            save()
        for engine in engines:
            check(subprocess.check_output(commands[engine] + ['search', '-0', '*']), universes[engine], (engine, 'final'))
            row = report['engines'][engine]
            row['init_summary'] = distribution(row['init'])
            row['update_summary'] = distribution(row['unchanged_update'])
            row['index_bytes'] = databases[engine].stat().st_size
            row['stats_bytes'] = databases[engine].with_name('stats.db').stat().st_size
    final_paths, _, final_pruned = snapshot(root, {os.fsencode(name) for name in exclusions})
    if final_paths != expected or final_pruned != pruned:
        raise RuntimeError('source path inventory changed during benchmark; results are not valid')
    report['source_inventory_unchanged'] = True
    report['load_end'] = os.getloadavg()
    report['finished_utc'] = datetime.now(timezone.utc).isoformat()
    save()
    print('Report:', output, flush=True)


if __name__ == '__main__':
    main()
