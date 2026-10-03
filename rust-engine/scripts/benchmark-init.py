#!/usr/bin/env python3
"""Compare fresh native index builds on one isolated corpus, alternating binaries."""
import argparse
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

spec = importlib.util.spec_from_file_location('native_bench', pathlib.Path(__file__).with_name('benchmark-native.py'))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', required=True, help='Saved baseline executable')
    parser.add_argument('--after', default='rust-engine/target/release/nasfind')
    parser.add_argument('--files', type=int, default=100000)
    parser.add_argument('--per-directory', type=int, default=100)
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--parent', default=str(pathlib.Path.home() / '.cache'))
    parser.add_argument('--output', default='native-init-benchmark.json')
    args = parser.parse_args()
    if min(args.files, args.per_directory, args.runs) < 1:
        parser.error('files, per-directory, runs must be positive')
    time_tool = shutil.which('time')
    if time_tool and b'GNU Time' in subprocess.run([time_tool, '--version'], capture_output=True).stdout:
        bench.GNU_TIME = time_tool
    binaries = {label: str(pathlib.Path(path).resolve()) for label, path in [('before', args.before), ('after', args.after)]}
    report = {'platform': platform.platform(), 'files': args.files, 'per_directory': args.per_directory,
              'runs': args.runs, 'load_start': os.getloadavg(), 'rss_tool': bench.GNU_TIME,
              'note': 'Same generated corpus. New DB for every trial. Alternate before/after order, no caches cleared. CLI timings include stats refresh and optional GNU time wrapper. Phase timings from native diagnostics. Full search results checked after every build.',
              'binaries': {}, 'results': {label: [] for label in binaries}}
    for label, path in binaries.items():
        report['binaries'][label] = {'sha256': hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest(),
                                      'version': subprocess.check_output([path, '--version'], text=True).strip()}
    parent = pathlib.Path(args.parent).resolve()
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='nasfind-init-bench-', dir=str(parent)) as temporary:
        base = pathlib.Path(temporary)
        root = base / 'files'
        root.mkdir()
        expected = {os.fsencode(root)}
        for number in range(args.files):
            directory = root / ('group_%05d' % (number // args.per_directory))
            if number % args.per_directory == 0:
                directory.mkdir()
                expected.add(os.fsencode(directory))
            family = ['soil', 'rain', 'ERA5', 'report'][number % 4]
            extension = ['nc', 'tif', 'csv', 'txt'][number % 4]
            path = directory / ('%s_station_%07d_2024.%s' % (family, number, extension))
            path.touch()
            expected.add(os.fsencode(path))
        for trial in range(args.runs):
            for label in binaries if trial % 2 == 0 else reversed(list(binaries)):
                storage = base / ('%s_%d' % (label, trial))
                storage.mkdir()
                database = storage / 'index.db'
                config = storage / 'config.toml'
                config.write_text('[[index]]\nname="bench"\nroot=' + json.dumps(str(root)) + '\ndatabase=' + json.dumps(str(database)) + '\n')
                command = [binaries[label], '-c', str(config)]
                sample, _ = bench.measure(command + ['index', 'update', '--engine', 'rust', '--no-progress'])
                output = subprocess.check_output(command + ['search', '-0', '*'])
                found = output.split(b'\0')[:-1]
                assert len(found) == len(expected) and set(found) == expected, (label, trial, 'incorrect full result set')
                # Compare selective candidate results, not just the scan fallback.
                for pattern in ['0000123', 'soil', 'does_not_exist_xyz']:
                    actual = subprocess.check_output(command + ['search', '-b', '-0', pattern]).split(b'\0')[:-1]
                    oracle = {path for path in expected if pattern.encode() in path.rsplit(b'/', 1)[-1]}
                    assert len(actual) == len(oracle) and set(actual) == oracle, (label, trial, pattern)
                sample.update({'trial': trial, 'index_bytes': database.stat().st_size})
                report['results'][label].append(sample)
                pathlib.Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
                print(label, trial, sample, flush=True)
                # Previous trials should not occupy disk or induce page-cache
                # pressure. Never remove the baseline/optimized executables.
                shutil.rmtree(storage)
    report['load_end'] = os.getloadavg()
    report['summary'] = {}
    for label, samples in report['results'].items():
        report['summary'][label] = {'median_seconds': statistics.median(s['seconds'] for s in samples),
                                     'max_peak_rss_kib': max(s['peak_rss_kib'] or 0 for s in samples) or None}
    report['summary']['speedup'] = report['summary']['before']['median_seconds'] / report['summary']['after']['median_seconds']
    pathlib.Path(args.output).write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['summary'], indent=2))
    print('Report:', args.output)


if __name__ == '__main__':
    main()
