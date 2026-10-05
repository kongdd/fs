#!/usr/bin/env python3
"""Live progress stays on one terminal row, including narrow terminals and Unicode."""
import fcntl
import json
import os
import pathlib
import pty
import struct
import subprocess
import sys
import tempfile
import termios
import unicodedata

binary = str(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else 'target/release/fs').resolve())
# /tmp may be mounted noexec on NAS systems; mock tools need an executable filesystem.
cache = pathlib.Path.home() / '.cache'
cache.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix='fs-progress-row-', dir=cache) as temporary:
    base = pathlib.Path(temporary)
    locate = base / 'plocate'
    locate.write_text('#!/usr/bin/env python3\nprint(100)\n')
    locate.chmod(0o755)
    scanner = base / 'updatedb'
    scanner.write_text('#!/usr/bin/env python3\nimport time\n'
                       'for i in range(50): print("/data/file" + str(i), flush=True)\n'
                       'time.sleep(1.1)\n')
    scanner.chmod(0o755)
    database = base / 'index.db'
    database.touch()
    config = base / 'config.toml'
    config.write_text('engine="plocate"\n[tools]\nplocate=' + json.dumps(str(locate))
                      + '\nupdatedb=' + json.dumps(str(scanner))
                      + '\n[[index]]\nname="目录进度\\n测试"\nroot=' + json.dumps(str(base))
                      + '\ndatabase=' + json.dumps(str(database)) + '\n')
    for columns in (32, 80):
        master, slave = pty.openpty()
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, columns, 0, 0))
            result = subprocess.run([binary, '-c', str(config), 'updatedb'],
                                    stdout=subprocess.PIPE, stderr=slave, timeout=10,
                                    env={**os.environ, 'NO_COLOR': '1', 'COLUMNS': '999'})
            os.close(slave)
            slave = None
            chunks = []
            while True:
                try:
                    chunk = os.read(master, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                chunks.append(chunk)
            display = b''.join(chunks)
            assert result.returncode == 0, display
            frames = display.split(b'\r\x1b[2K')[1:-1]
            assert len(frames) >= 2, display
            for frame in frames:
                text = frame.decode()
                assert not any(unicodedata.category(c) == 'Cc' for c in text), repr(text)
                width = sum(2 if unicodedata.east_asian_width(c) in ('W', 'F') else 1 for c in text)
                assert width < columns, (columns, width, text)
            if columns == 32:
                assert any(frame.endswith('…'.encode()) for frame in frames), frames
        finally:
            os.close(master)
            if slave is not None:
                os.close(slave)
print('single-row progress: OK')
