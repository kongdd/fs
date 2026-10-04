#!/usr/bin/env python3
"""Interleave fresh Rust/plocate builds for one index in a real configuration.

The small config reader accepts JSON-style quoted strings/arrays used by the
example config, not arbitrary TOML. The original config is passed through to the
CLI with only the selected database path replaced. Unsupported arrays fail.
"""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import sqlite3
import statistics
import subprocess
import tempfile
import time


def array(text, key, default):
    match = re.search(r'^\s*' + key + r'\s*=\s*(\[.*?\])', text, re.M | re.S)
    if not match:
        return default
    # Strip comments without stripping '#' inside a quoted directory name.
    value = match[1]
    result, quoted, escape, comment = [], False, False, False
    for char in value:
        if comment:
            if char == '\n':
                comment = False
                result.append(char)
            continue
        if char == '#' and not quoted:
            comment = True
            continue
        result.append(char)
        if escape:
            escape = False
        elif char == '\\' and quoted:
            escape = True
        elif char == '"':
            quoted = not quoted
    parsed = json.loads(re.sub(r',\s*\]', ']', ''.join(result)))
    if not isinstance(parsed, list) or not all(isinstance(item, str) for item in parsed):
        raise ValueError('expected string array for ' + key)
    return parsed


def field(text, key):
    match = re.search(r'^\s*' + key + r'\s*=\s*("(?:[^"\\]|\\.)*")\s*(?:#.*)?$', text, re.M)
    if not match:
        raise ValueError('missing quoted field: ' + key)
    return json.loads(match[1])


def snapshot(root, exclusions, paths):
    root = os.fsencode(root)
    names = {os.fsencode(name) for name in exclusions}
    blocked_paths = [os.fsencode(path) for path in paths]
    included, pruned = {root}, set()
    directories = 0

    def blocked(path):
        return any(path == item or path.startswith(item.rstrip(b'/') + b'/') for item in blocked_paths)

    def fail(error):
        raise error

    for parent, dirs, files in os.walk(root, followlinks=False, onerror=fail):
        directories += 1
        retained = []
        for name in dirs:
            path = os.path.join(parent, name)
            if (name in names or blocked(path)) and not os.path.islink(path):
                pruned.add(path)
            elif not blocked(path):
                retained.append(name)
                included.add(path)
        dirs[:] = retained
        included.update(os.path.join(parent, name) for name in files if not blocked(os.path.join(parent, name)))
    return included, pruned, directories


def filtered(paths, extensions, files, excluded_paths=()):
    extensions = {os.fsencode(ext.lstrip('.')).lower() for ext in extensions}
    files = {os.fsencode(name).lower() for name in files}
    blocked = [os.fsencode(path).rstrip(b'/') for path in excluded_paths]
    result = set()
    for path in paths:
        if any(path == item or path.startswith(item + b'/') for item in blocked):
            continue
        name = path.rstrip(b'/').rsplit(b'/', 1)[-1].lower()
        dot = name.rfind(b'.')
        if name not in files and not (dot > 0 and name[dot + 1:] in extensions):
            result.add(path)
    return result


def measure(command, profiling):
    env = os.environ.copy()
    if profiling:
        env['FS_SCAN_PROFILE'] = '1'
    else:
        env.pop('FS_SCAN_PROFILE', None)
    with tempfile.TemporaryFile() as stdout, tempfile.NamedTemporaryFile() as usage:
        start = time.perf_counter()
        process = subprocess.run(['/usr/bin/time', '-f', '%M %U %S', '-o', usage.name, '--'] + command,
                                 stdout=stdout, stderr=subprocess.PIPE, env=env, close_fds=False)
        elapsed = time.perf_counter() - start
        diagnostics = process.stderr.decode(errors='replace')
        if process.returncode:
            raise RuntimeError(diagnostics)
        usage.seek(0)
        rss, user, system = usage.read().split()
    sample = {'seconds': elapsed, 'peak_rss_kib': int(rss), 'user_seconds': float(user),
              'system_seconds': float(system), 'diagnostics': diagnostics}
    phases = re.search(r'timings: scan ([\d.]+)s .*? index ([\d.]+)s .*? finalize ([\d.]+)s', diagnostics)
    if phases:
        sample['native_phase_seconds'] = dict(zip(['scan', 'index', 'finalize'], map(float, phases.groups())))
    sample['stats_build_seconds'] = sum(map(float, re.findall(r'stats cached: .*? in ([\d.]+)s', diagnostics)))
    sample['scan_detail_seconds'] = {}
    for component, values in re.findall(r'scan-detail (worker|consumer|writer): ([^\n]+)', diagnostics):
        sample['scan_detail_seconds'][component] = {
            key: float(seconds) for key, seconds in re.findall(r'(\w+) ([\d.]+)s', values)}
    return sample


def check(command, expected):
    data = subprocess.check_output(command + ['search', '-0', '*'])
    if data and not data.endswith(b'\0'):
        raise AssertionError('incomplete NUL results')
    paths = data.split(b'\0')[:-1]
    actual = set(paths)
    if len(actual) != len(paths) or actual != expected:
        raise AssertionError(('result mismatch', len(paths), len(expected),
                              'missing', sorted(expected - actual)[:3],
                              'extra', sorted(actual - expected)[:3]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', default='examples/config_nas.toml')
    parser.add_argument('--index', default='cmip6')
    parser.add_argument('--fs', default='target/release/fs')
    parser.add_argument('--runs', type=int, default=6)
    parser.add_argument('--update-runs', type=int, default=3)
    parser.add_argument('--scan-profile', action='store_true', help='Opt-in Rust scan diagnostics, adds timing overhead')
    parser.add_argument('--output', default='docs/updatedb/cmip6-interleaved.json.txt')
    args = parser.parse_args()
    if min(args.runs, args.update_runs) < 1:
        parser.error('run counts must be positive')
    source = pathlib.Path(args.config).resolve(strict=True)
    binary = str(pathlib.Path(args.fs).resolve(strict=True))
    parts = source.read_text().split('[[index]]')
    selected = next(part for part in parts[1:] if field(part, 'name') == args.index)
    root = pathlib.Path(field(selected, 'root')).resolve(strict=True)
    original_db = pathlib.Path(field(selected, 'database')).resolve()
    parent = original_db.parent
    output = pathlib.Path(args.output).resolve()
    if not root.is_dir() or not parent.is_dir() or parent == root or root in parent.parents or root == output or root in output.parents:
        parser.error('root must be a directory; database storage/output must be outside root')
    if not shutil.which('plocate') or not shutil.which('updatedb'):
        parser.error('plocate and updatedb are required')
    excludes = array(selected, 'exclude_dirs', array(parts[0], 'exclude_dirs', []))
    extensions = array(selected, 'exclude_extensions', array(parts[0], 'exclude_extensions', []))
    files = array(selected, 'exclude_files', array(parts[0], 'exclude_files', []))
    paths = [pathlib.Path(path) if pathlib.Path(path).is_absolute() else root / path
             for path in array(selected, 'exclude_paths', array(parts[0], 'exclude_paths', []))]
    report = {'started_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'status': 'inventory', 'config': str(source), 'config_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
              'binary': binary, 'binary_sha256': hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),
              'index': args.index, 'root': str(root), 'storage_parent': str(parent),
              'effective_filters': {'exclude_dirs': excludes, 'exclude_paths': list(map(str, paths)),
                                    'exclude_extensions': extensions, 'exclude_files': files},
              'runs': args.runs, 'update_runs': args.update_runs, 'scan_profile': args.scan_profile,
              'platform': platform.platform(), 'load_start': os.getloadavg(),
              'tools': {tool: subprocess.check_output([tool, '--version'], text=True).splitlines()[0]
                        for tool in ['plocate', 'updatedb']},
              'note': 'Inventory prewarms directory metadata for BOTH engines; no caches cleared, not a cold-disk benchmark. Fresh DB per trial, alternating first engine, same storage filesystem. Single-index isolated config preserves filters but excludes other-index union refresh. CLI includes startup, GNU time and stats. Per-engine result sets and stats totals verified; plocate retains pruned markers and omits root. Profiling is opt-in and background worker/consumer times overlap; enumerate includes file_type/allocation/pruning, send includes queue backpressure. Source path inventories rechecked at end, not content/mtime fingerprints.',
              'engines': {engine: {'init': [], 'updates': []} for engine in ['rust', 'plocate']}}

    def save():
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + '\n')

    save()
    start = time.perf_counter()
    inventory, pruned, directories = snapshot(root, excludes, paths)
    report['inventory'] = {'entries': len(inventory), 'directories': directories, 'pruned_markers': len(pruned),
                           'seconds': time.perf_counter() - start,
                           'sha256': hashlib.sha256(b'\0'.join(sorted(inventory))).hexdigest()}
    expected = {'rust': filtered(inventory, extensions, files, paths),
                'plocate': filtered((inventory - {os.fsencode(root)}) | pruned, extensions, files, paths)}
    report['expected_filtered_totals'] = {engine: len(values) for engine, values in expected.items()}
    report['status'] = 'building'
    print('inventory', report['inventory'], 'filtered totals', report['expected_filtered_totals'], flush=True)
    save()
    try:
        with tempfile.TemporaryDirectory(prefix='fs-config-bench-', dir=str(parent)) as temporary:
            base = pathlib.Path(temporary)
            commands, databases = {}, {}
            for trial in range(args.runs):
                engines = ['rust', 'plocate'] if trial % 2 == 0 else ['plocate', 'rust']
                for engine in engines:
                    storage = base / engine
                    if storage.exists():
                        shutil.rmtree(storage)
                    storage.mkdir()
                    database = storage / 'index.db'
                    config = storage / 'config.toml'
                    text = re.sub(r'^\s*database\s*=.*$', 'database=' + json.dumps(str(database)), selected, flags=re.M)
                    config.write_text(parts[0] + '[[index]]' + text)
                    command = [binary, '-c', str(config)]
                    commands[engine], databases[engine] = command, database
                    sample = measure(command + ['index', 'init', args.index, '--engine', engine, '--no-progress'], args.scan_profile and engine == 'rust')
                    sample.update({'trial': trial, 'index_bytes': database.stat().st_size,
                                   'stats_bytes': (storage / 'stats.db').stat().st_size})
                    check(command, expected[engine])
                    with sqlite3.connect('file:' + str(storage / 'stats.db') + '?mode=ro', uri=True) as connection:
                        totals = connection.execute('SELECT total FROM selections').fetchall()
                        assert totals and all(total == len(expected[engine]) for (total,) in totals)
                    report['engines'][engine]['init'].append(sample)
                    save()
                    print(engine, trial, round(sample['seconds'], 3), 's', sample.get('scan_detail_seconds'), flush=True)
            report['status'] = 'updating'
            save()
            for trial in range(args.update_runs):
                for engine in (['rust', 'plocate'] if trial % 2 == 0 else ['plocate', 'rust']):
                    database = databases[engine]
                    before = database.stat()
                    sample = measure(commands[engine] + ['index', 'update', args.index, '--engine', engine, '--no-progress'], False)
                    after = database.stat()
                    sample.update({'trial': trial, 'database_fingerprint_unchanged':
                                   (before.st_size, before.st_mtime_ns, before.st_ino) == (after.st_size, after.st_mtime_ns, after.st_ino)})
                    check(commands[engine], expected[engine])
                    report['engines'][engine]['updates'].append(sample)
                    print(engine, 'update', trial, round(sample['seconds'], 3), flush=True)
                    save()
        final, final_pruned, _ = snapshot(root, excludes, paths)
        if final != inventory or final_pruned != pruned:
            raise RuntimeError('source path inventory changed; results invalid')
        report['source_inventory_unchanged'] = True
        report['temporary_indexes_removed'] = True
        for row in report['engines'].values():
            row['summary'] = {}
            for kind in ['init', 'updates']:
                samples = row[kind]
                row['summary'][kind] = {
                    'median_seconds': statistics.median(sample['seconds'] for sample in samples),
                    'min_seconds': min(sample['seconds'] for sample in samples),
                    'max_seconds': max(sample['seconds'] for sample in samples),
                    'max_peak_rss_kib': max(sample['peak_rss_kib'] for sample in samples)}
        report['status'] = 'complete'
        report['load_end'] = os.getloadavg()
        report['finished_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        save()
        print('summary', json.dumps({engine: row['summary'] for engine, row in report['engines'].items()}, indent=2), flush=True)
    except BaseException as error:
        report.update({'status': 'failed', 'error': str(error)})
        save()
        raise


if __name__ == '__main__':
    main()
