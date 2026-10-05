#!/usr/bin/env python3
"""Measure Everything filtering against direct plocate on an isolated corpus."""
import argparse
import json
import os
import pathlib
import platform
import shutil
import statistics
import subprocess
import tempfile

import common as bench


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fs', default='target/release/fs')
    parser.add_argument('--files', type=int, default=100000)
    parser.add_argument('--runs', type=int, default=7)
    parser.add_argument('--output', default='everything-benchmark.json')
    args = parser.parse_args()
    if min(args.files, args.runs) < 1:
        parser.error('files and runs must be positive')
    binary = str(pathlib.Path(args.fs).resolve())
    plocate = shutil.which('plocate')
    if not plocate:
        parser.error('plocate is required on PATH')
    time_tool = shutil.which('time')
    if time_tool and b'GNU Time' in subprocess.run([time_tool, '--version'], capture_output=True).stdout:
        bench.GNU_TIME = time_tool
    report = {'platform': platform.platform(), 'files': args.files, 'runs': args.runs,
              'load_start': os.getloadavg(), 'queries': {},
              'note': 'One shared plocate DB. Direct plocate vs fs Everything CLI, both NUL output captured. One warmup per command, alternating order of timed runs, no OS caches cleared. Complete results checked against byte-level oracle; limited results checked for count, uniqueness and membership. NOT comparison omitted because plocate has no NOT predicate; OR and extension-list comparisons use equivalent POSIX regexes since plocate has no OR flag. CLI timings include optional GNU time wrapper. No user config or indexes accessed.'}
    parent = (pathlib.Path.home() / '.cache').resolve()
    parent.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='fs-everything-bench-', dir=str(parent)) as temporary:
        base = pathlib.Path(temporary)
        root = base / 'files'
        root.mkdir()
        expected = {os.fsencode(root)}
        for number in range(args.files):
            directory = root / ('group_%05d' % (number // 100))
            if number % 100 == 0:
                directory.mkdir()
                expected.add(os.fsencode(directory))
            family = ['soil', 'rain', 'ERA5', 'report'][number % 4]
            extension = ['nc', 'tif', 'csv', 'txt'][number % 4]
            path = directory / ('%s_station_%07d_2024.%s' % (family, number, extension))
            path.touch()
            expected.add(os.fsencode(path))
        database = base / 'index.db'
        config = base / 'config.toml'
        config.write_text('[[index]]\nname="bench"\nroot=' + json.dumps(str(root)) + '\ndatabase=' + json.dumps(str(database)) + '\n')
        command = [binary, '-c', str(config)]
        report['build'], _ = bench.measure(command + ['index', 'update', '--no-progress'])
        direct = [plocate, '-d', str(database), '-i', '-b', '-0']
        basename = lambda path: path.rsplit(b'/', 1)[-1].lower()
        cases = [
            ('rare', ['0000123'], ['0000123'], lambda path: b'0000123' in basename(path), None),
            ('and', ['soil station'], ['soil', 'station'], lambda path: b'soil' in basename(path) and b'station' in basename(path), None),
            ('or', ['soil | rain'], ['--regex', 'soil|rain'], lambda path: b'soil' in basename(path) or b'rain' in basename(path), None),
            ('not', ['station !soil'], None, lambda path: b'station' in basename(path) and b'soil' not in basename(path), None),
            ('extension', ['ext:nc;tif'], ['--regex', '[.](nc|tif)$'], lambda path: basename(path).endswith((b'.nc', b'.tif')), None),
            ('limit50', ['-n', '50', 'soil'], ['-l', '50', 'soil'], lambda path: b'soil' in basename(path), 50),
        ]
        for name, everything, raw, predicate, limit in cases:
            oracle = {path for path in expected if predicate(path)}
            commands = {'everything': command + ['search', '-0'] + everything}
            if raw is not None:
                commands['plocate'] = direct + raw
            samples = {label: [] for label in commands}
            warmup = {}
            def query(label):
                sample, output = bench.measure(commands[label])
                assert not output or output.endswith(b'\0'), (name, label, 'truncated NUL output')
                found = output.split(b'\0')[:-1]
                assert len(found) == len(set(found)), (name, label, 'duplicates')
                if limit is None:
                    assert set(found) == oracle, (name, label, len(found), len(oracle))
                else:
                    assert len(found) == min(limit, len(oracle)) and set(found) <= oracle, (name, label)
                return sample
            for label in commands:
                warmup[label] = query(label)
            for trial in range(args.runs):
                for label in commands if trial % 2 == 0 else reversed(list(commands)):
                    samples[label].append(query(label))
            row = {'matches': min(limit, len(oracle)) if limit else len(oracle), 'commands': {}}
            for label in commands:
                row['commands'][label] = {'p50_ms': statistics.median(s['seconds'] for s in samples[label]) * 1000,
                                          'max_ms': max(s['seconds'] for s in samples[label]) * 1000,
                                          'samples': samples[label], 'warmup': warmup[label]}
            report['queries'][name] = row
            print(name, {label: round(stats['p50_ms'], 2) for label, stats in row['commands'].items()}, flush=True)
    report['load_end'] = os.getloadavg()
    pathlib.Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
    print('Report:', args.output)


if __name__ == '__main__':
    main()
