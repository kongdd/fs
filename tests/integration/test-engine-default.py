#!/usr/bin/env python3
"""Default-engine environment/CLI precedence; only isolated temporary databases."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/fs').resolve())
with tempfile.TemporaryDirectory(prefix='fs-engine-default-') as temporary:
    base = pathlib.Path(temporary)
    root = base / 'files'
    root.mkdir()
    (root / 'file.nc').touch()
    database = base / 'index.db'
    config = base / 'config.toml'
    config.write_text('[tools]\nplocate="/does/not/exist"\nupdatedb="/does/not/exist"\n'
                      '[[index]]\nname="test"\nroot=' + json.dumps(str(root)) +
                      '\ndatabase=' + json.dumps(str(database)) + '\n')

    def run(engine, *args, ok=True):
        env = dict(os.environ)
        env.pop('FS_ENGINE', None)
        if engine is not None:
            env['FS_ENGINE'] = engine
        result = subprocess.run([binary, '-c', str(config), *args], env=env,
                                capture_output=True, timeout=30)
        assert (result.returncode == 0) == ok, (engine, args, result.stderr)
        return result

    assert b'fs config engine' in run(None, 'updatedb', '--help').stdout
    shown = run(None, 'config', 'engine')
    assert b'engine: rust (built-in default)' in shown.stdout
    # The built-in default is rust, so missing external tools do not block a new DB.
    run(None, 'updatedb')
    assert database.read_bytes().startswith(b'SQLite format 3\0')
    database.unlink()
    run(None, 'config', 'engine', 'rust')
    assert b'engine: rust (config ' in run(None, 'config', 'engine').stdout
    # Environment still overrides the saved config, and an explicit flag overrides both.
    run('plocate', 'updatedb', ok=False)
    assert not database.exists()
    run('plocate', 'updatedb', '--engine', 'rust')
    assert database.read_bytes().startswith(b'SQLite format 3\0')
    database.unlink()
    assert not database.exists()
    run('plocate', 'updatedb', ok=False)
    assert not database.exists()
    # Invalid defaults fail early, without initializing a DB.
    run('invalid', 'updatedb', ok=False)
    assert not database.exists()
    # Doctor resolves the same environment default for missing databases.
    run('rust', 'doctor')
    run('rust', 'updatedb')
    assert database.read_bytes().startswith(b'SQLite format 3\0')
    before = database.read_bytes()
    # Explicit CLI selection wins over either a different or invalid env value.
    run('plocate', 'updatedb', '--engine', 'rust')
    run('invalid', 'updatedb', '--engine', 'rust')
    assert database.read_bytes() == before
    run('rust', 'updatedb', '--engine', 'plocate', ok=False)
    run('invalid', 'updatedb', ok=False)
    assert database.read_bytes() == before
    # Search does not inspect the engine default: only the existing DB format.
    expected = run(None, 'locate', '-0', 'file.nc').stdout
    assert run('plocate', 'locate', '-0', 'file.nc').stdout == expected
    assert run('invalid', 'locate', '-0', 'file.nc').stdout == expected
    # Build from outdir + name, then query through an explicit database path.
    output = base / 'output'
    config.write_text('outdir=' + json.dumps(str(output)) +
                      '\n[[index]]\nname="test"\nroot=' + json.dumps(str(root)) + '\n')
    run(None, 'updatedb')
    generated = output / 'test.db'
    assert generated.read_bytes().startswith(b'SQLite format 3\0')
    config.write_text('[[index]]\nname="test"\nroot=' + json.dumps(str(root)) +
                      '\ndatabase=' + json.dumps(str(generated)) + '\n')
    assert run(None, 'locate', '-0', 'file.nc').stdout == expected
print('Default engine CLI integration tests passed')
