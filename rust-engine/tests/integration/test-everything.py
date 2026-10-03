#!/usr/bin/env python3
"""Everything syntax over real updatedb/plocate, and parity with native indexes."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/nasfind').resolve())
for engine in ['plocate', 'rust']:
    with tempfile.TemporaryDirectory(prefix='nasfind-everything-') as temporary:
        base = pathlib.Path(temporary)
        root = base / 'soil_parent/data'
        root.mkdir(parents=True)
        (root / 'rain_folder').mkdir()
        names = ['soil.nc', 'SOIL.csv', 'rain.nc', 'soil_rain.nc', 'backup_soil.nc', 'my report.nc', 'a | b.txt', 'rain_folder/other.nc', '中文.nc']
        for name in names:
            (root / name).touch()
        invalid = os.fsencode(root) + b'/bad_\xff.nc'
        with open(invalid, 'wb'):
            pass
        config = base / 'config.toml'
        config.write_text('[[index]]\nname="test"\nroot=' + json.dumps(str(root)) + '\ndatabase=' + json.dumps(str(base / 'index.db')) + '\n')
        def run(*args, ok=True):
            result = subprocess.run([binary, '-c', str(config), *args], capture_output=True, timeout=30)
            assert (result.returncode == 0) == ok, (engine, args, result.stderr)
            return result
        def paths(*args):
            return run('search', '-0', *args).stdout.split(b'\0')[:-1]
        def check(expression, expected, *flags):
            actual = paths(*flags, expression)
            wanted = {os.fsencode(root / name) for name in expected}
            assert set(actual) == wanted and len(actual) == len(wanted), (engine, expression, actual, wanted)
        # Verify the new default really is updatedb, not just explicit engine use.
        arguments = [] if engine == 'plocate' else ['--engine', 'rust']
        run('index', 'update', '--no-progress', *arguments)
        assert (base / 'index.db').read_bytes().startswith(b'SQLite format 3\0') == (engine == 'rust')
        soil = ['soil.nc', 'SOIL.csv', 'soil_rain.nc', 'backup_soil.nc']
        check('soil', soil)
        check('soil', [name for name in soil if name != 'SOIL.csv'], '--case-sensitive')
        check('<soil | rain> ext:nc !backup', ['soil.nc', 'rain.nc', 'soil_rain.nc'])
        check('soil ext:nc;csv !backup', ['soil.nc', 'SOIL.csv', 'soil_rain.nc'])
        check('soil | soil_rain', soil)
        check('"my report"', ['my report.nc'])
        check('"a | b"', ['a | b.txt'])
        check('path:rain_folder ext:nc', ['rain_folder/other.nc'])
        check('rain_folder/other', ['rain_folder/other.nc'])
        check('regex:"^soil[.]nc$"', ['soil.nc'])
        assert len(paths('-p', 'soil')) > len(soil)
        assert len(paths('--locate', 'soil')) > len(soil)
        assert paths('bad') == [invalid]
        assert run('search', '--json', 'nothing_matches').stdout == b'[]\n'
        whole = paths('soil | rain')
        assert paths('--offset', '1', '-l', '2', 'soil | rain') == whole[1:3]
        assert paths('--ext', 'nc', '--path', str(root / 'rain_folder'), '*') == [os.fsencode(root / 'rain_folder/other.nc')]
        for expression in ['soil |', '<>', '<soil', 'soil >', '!', 'ext:', 'folder:']:
            run('search', expression, ok=False)
        run('search', '-l', '0', '*', ok=False)
        # No implicit NAS access, including negative-only/full-scan queries.
        root.rename(base / 'offline')
        check('soil', soil)
        assert invalid in paths('!soil')
        assert paths('-e', 'soil') == []
print('Everything CLI integration tests passed (plocate + Rust)')
