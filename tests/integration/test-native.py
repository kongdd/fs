#!/usr/bin/env python3
"""Native CLI integration tests; no plocate or filesystem access during queries."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time


def encoded(path):
    raw = os.fsencode(path)
    return raw.replace(b'\\', b'/') if sys.platform == 'win32' else raw


binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/fs').resolve())
with tempfile.TemporaryDirectory(prefix='fs-native-e2e-') as temporary:
    base = pathlib.Path(temporary)
    root = base / 'files'
    root.mkdir()
    (root / 'nested').mkdir()
    (root / 'excluded').mkdir()
    names = ['soil_ERA5.nc', 'SOIL.csv', 'nested/rain.nc', 'nested/line_file.txt' if sys.platform == 'win32' else 'nested/line\nfile.txt', '中文.nc', 'excluded/hidden.nc', 'ignore.tmp']
    for name in names:
        (root / name).touch()
    # Raw bytes are Unix-only; Windows/APFS exercise Unicode filenames.
    invalid = os.fsencode(root) + (b'/bad_native.nc' if sys.platform in ('darwin', 'win32') else b'/bad_\xff.nc')
    with open(invalid, 'wb'):
        pass
    invalid = encoded(invalid)
    if sys.platform != 'win32':
        os.symlink(root, root / 'loop')
    config = base / 'config.toml'
    config.write_text('[tools]\nplocate="/does/not/exist"\nupdatedb="/does/not/exist"\n[filters]\nexclude_dirs=["excluded"]\nexclude_extensions=["tmp"]\n[[index]]\nname="native"\nroot=' + json.dumps(str(root)) + '\ndatabase=' + json.dumps(str(base / 'index.db')) + '\n')

    def run(*args, ok=True):
        if args[0] in ('index', 'updatedb') and '--engine' not in args:
            args = (*args, '--engine', 'rust')
        if args[0] == 'search':
            args = ('search', '--locate', *args[1:])
        result = subprocess.run([binary, '-c', str(config), *args], capture_output=True, timeout=30)
        assert (result.returncode == 0) == ok, (args, result.stderr)
        return result

    # Missing DBs default to plocate; this suite explicitly builds Rust.
    run('updatedb', 'init', '--no-progress')
    run('doctor') # Existing Rust indexes need no external tools.
    assert b'already exist' in run('index', 'init').stderr
    assert run('locate', '--files', '--ext', 'nc', 'soil').stdout
    run('ignore', 'add', 'nested')
    assert run('search', 'rain').stdout == b''
    run('ignore', 'rm', 'nested')

    # New config locations take priority, but old names still work.
    home = base / 'home'
    old_config = home / '.config/nasfind/config.toml'
    new_config = home / '.config/fs/config.toml'
    old_config.parent.mkdir(parents=True)
    old_config.write_text(config.read_text())
    env = dict(os.environ, HOME=str(home), USERPROFILE=str(home), APPDATA=str(home / '.config'))
    env.pop('FS_CONFIG', None)
    env.pop('NASFIND_CONFIG', None)

    def default_doctor(expected):
        result = subprocess.run([binary, 'doctor'], env=env, capture_output=True, timeout=30)
        assert result.returncode == 0 and str(expected).encode() in result.stdout, result.stderr

    default_doctor(old_config)
    new_config.parent.mkdir(parents=True)
    new_config.write_text(config.read_text())
    default_doctor(new_config)
    env['NASFIND_CONFIG'] = str(old_config)
    default_doctor(old_config)
    env['FS_CONFIG'] = str(config)
    default_doctor(config)
    run('index', 'update', '--no-progress')
    run('doctor')
    assert b'0 dirs scanned' in run('index', 'update', '--no-progress').stderr
    assert b'already exist' in run('index', 'init').stderr
    assert run('search', '-b', 'soil', '-0').stdout == encoded(root / 'soil_ERA5.nc') + b'\0'
    assert len(run('search', '-b', '-i', 'soil', '-0').stdout.split(b'\0')[:-1]) == 2
    assert run('search', '-r', 'soil_.*[.]nc$', '-0').stdout == encoded(root / 'soil_ERA5.nc') + b'\0'
    assert run('search', '--ext', '.NC', '--path', str(root / 'nested'), '-0', '*').stdout == encoded(root / 'nested/rain.nc') + b'\0'
    assert run('search', '-0', 'bad').stdout == invalid + b'\0'
    assert run('search', '-0', 'ignore').stdout == b''
    assert run('search', '-0', 'hidden').stdout == b''
    assert run('search', '--json', 'nothing_matches').stdout == b'[]\n'
    assert json.loads(run('search', '--json', 'soil').stdout) == [{'path': os.fsdecode(encoded(root / 'soil_ERA5.nc'))}]
    assert run('search', '-0', 'soil', 'nc').stdout == encoded(root / 'soil_ERA5.nc') + b'\0'
    # Default Everything syntax shares exact matching with the native planner.
    assert set(run('<soil | rain> ext:nc', '-0').stdout.split(b'\0')[:-1]) == {
        encoded(root / 'soil_ERA5.nc'), encoded(root / 'nested/rain.nc'),
    }
    soil = run('soil', '-0').stdout.split(b'\0')[:-1]
    assert run('soil | soil_ERA5', '-0', '--offset', '1', '-n', '1').stdout == soil[1] + b'\0'
    assert run('path:nested ext:nc !renamed', '-0').stdout == encoded(root / 'nested/rain.nc') + b'\0'
    assert run('search', '--path', str(root) + '-other', '*').stdout == b''
    all_paths = run('search', '-0', '*').stdout.split(b'\0')[:-1]
    assert run('search', '-0', '--offset', '1', '-n', '2', '*').stdout.split(b'\0')[:-1] == all_paths[1:3]
    run('search', '-r', '(', ok=False)
    before = (base / 'index.db').read_bytes()
    run('index', 'update', '--engine', 'plocate', ok=False)
    assert (base / 'index.db').read_bytes() == before
    initial_stats = run('stats', '-n10').stdout
    assert b'building stats cache' not in run('stats').stderr
    # Directory stamps intentionally have whole-second resolution.
    time.sleep(1.1)
    (root / 'nested/rain.nc').rename(root / 'nested/renamed.nc')
    (root / 'fresh.txt').touch()
    run('index', '--folder', str(root / 'nested'), '--no-progress')
    assert run('search', 'rain.nc').stdout == b''
    assert run('search', 'renamed.nc').stdout
    assert run('search', 'fresh.txt').stdout == b''
    run('index', 'update', '--no-progress')
    assert run('search', 'fresh.txt').stdout
    time.sleep(1.1)
    (root / 'fresh.txt').unlink()
    assert run('search', '-e', 'fresh.txt').stdout == b''
    assert run('search', 'fresh.txt').stdout
    run('index', 'update', '--no-progress')
    assert run('search', 'fresh.txt').stdout == b''
    # Cached searches/statistics work while the source root is offline.
    expected_stats = run('stats', '-n10').stdout
    root.rename(base / 'offline')
    assert run('search', '-0', 'bad').stdout == invalid + b'\0'
    assert run('stats', '-n10').stdout == expected_stats
    before = (base / 'index.db').read_bytes()
    run('index', 'update', '--no-progress', ok=False)
    assert (base / 'index.db').read_bytes() == before
print('Native CLI integration tests passed')
