#!/usr/bin/env python3
"""Isolated native-vs-plocate benchmark with result oracles and interleaved queries."""
import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import statistics
import subprocess
import tempfile
import time

GNU_TIME = None


def measure(command):
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors, tempfile.NamedTemporaryFile() as rss:
        wrapped = [GNU_TIME, '-f', '%M', '-o', rss.name, '--'] + command if GNU_TIME else command
        start = time.perf_counter()
        # GNU time's small launcher avoids the Python generator heap polluting
        # wait4's pre-exec peak RSS (even posix_spawn does that on some kernels).
        process = subprocess.Popen(wrapped, stdout=output, stderr=errors, close_fds=False)
        _, status, _ = os.wait4(process.pid, 0)
        process.returncode = -os.WTERMSIG(status) if os.WIFSIGNALED(status) else os.WEXITSTATUS(status)
        elapsed = time.perf_counter() - start
        if process.returncode:
            errors.seek(0)
            raise RuntimeError(errors.read().decode(errors='replace'))
        output.seek(0)
        data = output.read()
        rss.seek(0)
        peak = int(rss.read().strip()) if GNU_TIME else None
        sample = {'seconds': elapsed, 'peak_rss_kib': peak}
        errors.seek(0)
        diagnostics = errors.read().decode(errors='replace')
        phases = re.search(r'timings: scan ([0-9.]+)s .*? index ([0-9.]+)s .*? finalize ([0-9.]+)s', diagnostics)
        if phases:
            sample['native_phase_seconds'] = dict(zip(['scan', 'index', 'finalize'], map(float, phases.groups())))
        stats = re.findall(r'stats cached: .*? in ([0-9.]+)s', diagnostics)
        if stats:
            sample['stats_build_seconds'] = sum(map(float, stats))
        return sample, data


def percentile(values, p):
    values = sorted(values)
    index = (len(values) - 1) * p
    lo = int(index)
    hi = min(lo + 1, len(values) - 1)
    return values[lo] + (values[hi] - values[lo]) * (index - lo)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--nasfind', default='target/release/nasfind')
    parser.add_argument('--files', type=int, default=100000)
    parser.add_argument('--runs', type=int, default=21, help='One first sample + repeated timed samples')
    parser.add_argument('--warmup', type=int, default=3, help='Untimed-distribution warmups between first and repeated samples')
    parser.add_argument('--settle', type=float, default=3, help='Pause after indexing, seconds; does not clear caches')
    parser.add_argument('--parent', default=str(pathlib.Path.home() / '.cache'))
    parser.add_argument('--output', default='native-benchmark.json')
    parser.add_argument('--native-only', action='store_true')
    args = parser.parse_args()
    if args.files < 100 or args.runs < 3 or args.warmup < 0 or args.settle < 0:
        parser.error('files >= 100, runs >= 3, warmup and settle >= 0 required')
    global GNU_TIME
    candidate = shutil.which('time')
    if candidate and b'GNU Time' in subprocess.run([candidate, '--version'], capture_output=True).stdout:
        GNU_TIME = candidate
    binary = str(pathlib.Path(args.nasfind).resolve())
    pathlib.Path(args.parent).mkdir(parents=True, exist_ok=True)
    report = {'platform': platform.platform(), 'files': args.files, 'runs': args.runs,
              'version': subprocess.check_output([binary, '--version'], text=True).strip(),
              'binary_sha256': hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),
              'warmup_runs': args.warmup, 'settle_seconds': args.settle,
              'cpu_count': os.cpu_count(), 'load_average_start': os.getloadavg(),
              'python': platform.python_version(), 'rss_tool': GNU_TIME,
              'note': 'Synthetic zero-byte files; both indexes built before querying. Repeated queries interleaved, alternating engine order. No caches cleared. P50/P95 exclude first sample and recorded warmups. CLI timings include startup and optional GNU time wrapper; index timings include stats refresh. GNU time RSS (KiB on Linux) is not simultaneous process-tree sum. RSS null if GNU time unavailable.',
              'engines': {}}

    def save():
        pathlib.Path(args.output).write_text(json.dumps(report, indent=2) + '\n')

    with tempfile.TemporaryDirectory(prefix='nasfind-native-bench-', dir=str(pathlib.Path(args.parent).resolve())) as temporary:
        base = pathlib.Path(temporary)
        root = base / 'files'
        root.mkdir()
        generation_start = time.perf_counter()
        expected = []
        for number in range(args.files):
            directory = root / ('group_%05d' % (number // 100))
            if number % 100 == 0:
                directory.mkdir()
            family = ['soil', 'rain', 'ERA5', 'report'][number % 4]
            extension = ['nc', 'tif', 'csv', 'txt'][number % 4]
            path = directory / ('%s_station_%07d_2024.%s' % (family, number, extension))
            path.touch()
            expected.append(os.fsencode(path))
        report['generation_seconds'] = time.perf_counter() - generation_start
        engines = ['rust'] if args.native_only else ['rust', 'plocate']
        commands = {}
        databases = {}
        for engine in engines:
            storage = base / engine
            storage.mkdir()
            config = storage / 'config.toml'
            database = storage / 'index.db'
            config.write_text('[[index]]\nname="bench"\nroot=' + json.dumps(str(root)) + '\ndatabase=' + json.dumps(str(database)) + '\n')
            command = [binary, '-c', str(config)]
            commands[engine] = command
            databases[engine] = database
            row = {'index': [], 'queries': {}}
            report['engines'][engine] = row
            for label in ['init', 'unchanged_update']:
                sample, _ = measure(command + ['index', 'update', '--engine', engine, '--no-progress'])
                row['index'].append({'kind': label, **sample})
                print(engine, label, sample, flush=True)
            save()
        # Same single nested-directory rename applied to both existing indexes.
        first = root / 'group_00000/soil_station_0000000_2024.nc'
        renamed = first.with_name('soil_renamed_0000000_2024.nc')
        first.rename(renamed)
        current = [os.fsencode(renamed) if p == os.fsencode(first) else p for p in expected]
        for engine in engines:
            sample, _ = measure(commands[engine] + ['index', 'update', '--engine', engine, '--no-progress'])
            row = report['engines'][engine]
            row['index'].append({'kind': 'one_directory_changed', **sample})
            row['index_bytes'] = databases[engine].stat().st_size
            row['stats_bytes'] = databases[engine].with_name('stats.db').stat().st_size
            print(engine, 'one_directory_changed', sample, flush=True)
        time.sleep(args.settle)
        queries = [('rare', ['0000123']), ('common', ['soil']), ('and', ['soil', '2024']),
                   ('missing', ['does_not_exist_xyz']), ('short', ['nc']), ('glob', ['*rain*.tif']), ('limited', ['soil'])]
        for label, patterns in queries:
            if label == 'glob':
                oracle = {p for p in current if b'rain' in p.rsplit(b'/', 1)[-1] and p.endswith(b'.tif')}
            else:
                oracle = {p for p in current if all(term.encode() in p.rsplit(b'/', 1)[-1] for term in patterns)}
            samples = {engine: [] for engine in engines}
            warmups = {engine: [] for engine in engines}

            def query(engine):
                extra = ['-l', '50'] if label == 'limited' else []
                sample, data = measure(commands[engine] + ['search', '--locate', '-b', '-0'] + extra + patterns)
                result = data.split(b'\0')[:-1]
                found = set(result)
                assert len(found) == len(result), (engine, label, 'duplicate results')
                if label == 'limited':
                    assert len(found) == min(50, len(oracle)) and found <= oracle, (engine, label)
                else:
                    assert found == oracle, (engine, label, len(found), len(oracle))
                return sample

            for engine in engines:
                samples[engine].append(query(engine))
            for iteration in range(args.warmup):
                for engine in engines if iteration % 2 == 0 else reversed(engines):
                    warmups[engine].append(query(engine))
            for iteration in range(args.runs - 1):
                for engine in engines if iteration % 2 == 0 else reversed(engines):
                    samples[engine].append(query(engine))
            for engine in engines:
                timed = samples[engine]
                times = [sample['seconds'] * 1000 for sample in timed[1:]]
                row = {'patterns': patterns, 'matches': min(50, len(oracle)) if label == 'limited' else len(oracle),
                       'first_ms': timed[0]['seconds'] * 1000,
                       'p50_ms': statistics.median(times), 'p95_ms': percentile(times, .95),
                       'peak_rss_kib': max(sample['peak_rss_kib'] or 0 for sample in timed) or None,
                       'samples': timed, 'warmup_samples': warmups[engine]}
                report['engines'][engine]['queries'][label] = row
                print(engine, label, 'P50=%.2fms P95=%.2fms matches=%d' % (row['p50_ms'], row['p95_ms'], row['matches']), flush=True)
            save()
        report['load_average_end'] = os.getloadavg()
        save()
    print('Report:', args.output)


if __name__ == '__main__':
    main()
