"""Subprocess timing/RSS helpers for isolated benchmarks."""
import os
import re
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
            sample['native_phase_seconds'] = dict(zip(
                ['scan', 'index', 'finalize'], map(float, phases.groups())))
        stats = re.findall(r'stats cached: .*? in ([0-9.]+)s', diagnostics)
        if stats:
            sample['stats_build_seconds'] = sum(map(float, stats))
        return sample, data
