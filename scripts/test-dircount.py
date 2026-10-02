#!/usr/bin/env python3
"""Unit tests for dircount; no NAS databases required."""
from collections import Counter
import io
import os
import random
import unittest
from unittest.mock import patch

from dircount import count_directories, nul_paths, subtree_pattern


def reference(paths):
    counts = Counter()
    seen = set()
    total = 0
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        total += 1
        parent = os.path.dirname(path)
        while parent and parent != os.path.dirname(parent):
            counts[parent] += 1
            parent = os.path.dirname(parent)
    return counts, total


class DirectoryCountsTests(unittest.TestCase):
    def test_matches_reference(self):
        rng = random.Random(42)
        paths = ['/', '/a', '//a', '/a/file\nname', '/a/invalid\udcff', 'relative/file']
        for _ in range(5000):
            paths.append('/' + '/'.join(str(rng.randrange(20)) for _ in range(rng.randrange(1, 10))))
        paths += paths[:500]
        rng.shuffle(paths)
        self.assertEqual(count_directories(iter(paths)), reference(paths))

    def test_database_records_require_no_filesystem_checks(self):
        paths = ['/missing/tree', '/missing/tree/file', '/missing/tree/file']
        with patch('os.stat', side_effect=AssertionError('must not inspect disk')), \
             patch('os.scandir', side_effect=AssertionError('must not traverse disk')):
            self.assertEqual(count_directories(paths), reference(paths))

    def test_streaming_byte_safe_records(self):
        paths = ['/a/new\nline', '/a/invalid\udcff', '/a/' + 'x' * 70000, '/end']
        raw = b'\0'.join(os.fsencode(path) for path in paths) + b'\0'
        self.assertEqual(list(nul_paths(io.BytesIO(raw))), paths)
        self.assertEqual(list(nul_paths(io.BytesIO(b'/tail'))), ['/tail'])
        self.assertEqual(list(nul_paths(io.BytesIO(b''))), [])

    def test_subtree_pattern(self):
        self.assertEqual(subtree_pattern('/data'), '/data/*')
        self.assertEqual(subtree_pattern('/'), '/*')
        self.assertEqual(subtree_pattern('/data/a[*?]'), r'/data/a\[\*\?]/*')
        self.assertEqual(subtree_pattern('/data/back\\slash'), r'/data/back\\slash/*')


if __name__ == '__main__':
    unittest.main()
