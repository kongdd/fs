#!/usr/bin/env python3
"""Everything syntax over real updatedb/plocate, and parity with native indexes."""
import json
import os
import pathlib
import subprocess
import shutil
import sys
import tempfile

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/fs').resolve())
engines = ['rust']
if shutil.which('plocate') and shutil.which('updatedb') and sys.platform != 'win32':
    engines.append('plocate')


def encoded(path):
    raw = os.fsencode(path)
    return raw.replace(b'\\', b'/') if sys.platform == 'win32' else raw


for engine in engines:
    with tempfile.TemporaryDirectory(prefix='fs-everything-') as temporary:
        base = pathlib.Path(temporary)
        root = base / 'soil_parent/data'
        root.mkdir(parents=True)
        (root / 'rain_folder').mkdir()
        names = ['soil.nc', 'SOIL.csv', 'rain.nc', 'soil_rain.nc', 'backup_soil.nc', 'my report.nc', 'a ! b.txt', 'rain_folder/other.nc', '中文.nc']
        for name in names:
            (root / name).touch()
        invalid = os.fsencode(root) + (b'/bad_native.nc' if sys.platform in ('darwin', 'win32') else b'/bad_\xff.nc')
        with open(invalid, 'wb'):
            pass
        invalid = encoded(invalid)
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
            wanted = {encoded(root / name) for name in expected}
            assert set(actual) == wanted and len(actual) == len(wanted), (engine, expression, actual, wanted)
        # Native indexing is the default; plocate is explicitly selected.
        arguments = [] if engine == 'rust' else ['--engine', 'plocate']
        run('index', 'update', '--no-progress', *arguments)
        assert (base / 'index.db').read_bytes().startswith(b'SQLite format 3\0') == (engine == 'rust')
        soil = ['soil.nc', 'SOIL.csv', 'soil_rain.nc', 'backup_soil.nc']
        check('soil', soil)
        check('soil', [name for name in soil if name != 'SOIL.csv'], '--case-sensitive')
        check('<soil | rain> ext:nc !backup', ['soil.nc', 'rain.nc', 'soil_rain.nc'])
        check('soil ext:nc;csv !backup', ['soil.nc', 'SOIL.csv', 'soil_rain.nc'])
        check('soil | soil_rain', soil)
        check('"my report"', ['my report.nc'])
        check('"a ! b"', ['a ! b.txt'])
        check('path:rain_folder ext:nc', ['rain_folder/other.nc'])
        check('rain_folder/other', ['rain_folder/other.nc'])
        check('regex:"^soil[.]nc$"', ['soil.nc'])
        assert len(paths('-p', 'soil')) > len(soil)
        assert len(paths('--locate', 'soil')) > len(soil)
        assert paths('bad') == [invalid]
        assert run('search', '--json', 'nothing_matches').stdout == b'[]\n'
        whole = paths('soil | rain')
        assert paths('--offset', '1', '-l', '2', 'soil | rain') == whole[1:3]
        assert paths('--ext', 'nc', '--path', str(root / 'rain_folder'), '*') == [encoded(root / 'rain_folder/other.nc')]
        for expression in ['soil |', '<>', '<soil', 'soil >', '!', 'ext:', 'folder:']:
            run('search', expression, ok=False)
        run('search', '-l', '0', '*', ok=False)
        # Retired/corrupt SQLite indexes must not be queried or overwritten.
        retired = base / 'retired.db'
        payload = b'SQLite format 3\0' + b'old index must remain intact'
        retired.write_bytes(payload)
        retired_config = base / 'retired.toml'
        retired_config.write_text(config.read_text().replace(
            json.dumps(str(base / 'index.db')), json.dumps(str(retired))))
        for arguments in [
            ['search', 'soil'], ['index', 'update', '--no-progress'],
            ['index', '--folder', str(root / 'rain_folder'), '--no-progress'],
            ['stats'], ['doctor'],
        ]:
            result = subprocess.run([binary, '-c', str(retired_config), *arguments],
                                    capture_output=True, timeout=30)
            assert result.returncode != 0, (arguments, result.stderr)
            assert retired.read_bytes() == payload
        # No implicit NAS access, including negative-only/full-scan queries.
        root.rename(base / 'offline')
        check('soil', soil)
        assert invalid in paths('!soil')
        assert paths('-e', 'soil') == []
print('Everything CLI integration tests passed: ' + ', '.join(engines))
