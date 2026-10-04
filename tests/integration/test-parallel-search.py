#!/usr/bin/env python3
"""Exercise >512 native blocks through CLI predicates, output and pagination."""
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import tempfile

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/fs').resolve())
before = str(pathlib.Path(sys.argv[2]).resolve()) if len(sys.argv) > 2 else None
with tempfile.TemporaryDirectory(prefix='fs-parallel-e2e-') as temporary:
    base = pathlib.Path(temporary)
    root = base / 'files'
    root.mkdir()
    expected = {os.fsencode(root)}
    for number in range(640):
        directory = root / f'd{number:04}'
        directory.mkdir()
        expected.add(os.fsencode(directory))
        for name in [f'py_{number:04}.c', 'other.bin', 'line_file.txt' if sys.platform == 'win32' else 'line\nfile.txt']:
            path = directory / name
            path.touch()
            expected.add(os.fsencode(path))
    if sys.platform not in ('win32', 'darwin'):
        invalid = os.fsencode(root / 'd0600') + b'/bad_\xff.c'
        with open(invalid, 'wb'):
            pass
        expected.add(invalid)
    database = base / 'index.db'
    config = base / 'config.toml'
    config.write_text('[[index]]\nname="parallel"\nroot=' + json.dumps(str(root)) + '\ndatabase=' + json.dumps(str(database)) + '\n')

    def run(executable, *args):
        result = subprocess.run([executable, '-c', str(config), *args], capture_output=True, timeout=30)
        assert result.returncode == 0, (args, result.stderr)
        return result.stdout

    def paths(*args):
        data = run(binary, 'search', '-0', *args)
        if before:
            assert data == run(before, 'search', '-0', *args), args
        return data.rstrip(b'\0').split(b'\0') if data else []

    run(binary, 'index', 'update', '--engine', 'rust', '--no-progress')
    with sqlite3.connect(database) as connection:
        assert connection.execute('SELECT count(*) FROM blocks').fetchone()[0] > 512
    if sys.platform == 'win32':
        expected = {path.replace(b'\\', b'/') for path in expected}
    whole = paths('*')
    assert set(whole) == expected and len(whole) == len(expected)
    py = [p for p in whole if b'py' in p.rsplit(b'/', 1)[-1]]
    assert paths('py') == py
    assert paths('py !path:d00') == [p for p in py if b'd00' not in p]
    assert paths('!py') == [p for p in whole if p not in py]
    assert paths('ext:c') == [p for p in whole if p.endswith(b'.c')]
    assert paths('--regex', 'py|other') == [p for p in whole if any(word in p.rsplit(b'/', 1)[-1] for word in [b'py', b'other'])]
    assert paths('--offset', '530', '-l', '20', 'py') == py[530:550]
    assert paths('-l', '513', 'py') == py[:513]
    assert paths('-l', '1', 'py') == py[:1]
    assert paths('--path', str(root / 'd0600'), 'py') == [p for p in py if p.startswith(os.fsencode(root / 'd0600').replace(b'\\', b'/') + b'/')]
    output = run(binary, 'search', '--json', 'py')
    assert [row['path'] for row in json.loads(output)] == [p.decode() for p in py]
    assert paths('--ext', 'c', 'py') == py
    assert paths('--files', 'py') == py
    assert paths('--dirs', 'py') == []
print('Parallel CLI integration tests passed')
