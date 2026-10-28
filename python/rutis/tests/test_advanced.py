"""The names that moved to `rutis.advanced` in 0.19 (#692) are only there:
the root has no old names, and importing one fails like any missing name."""

import subprocess
import sys
import unittest
from pathlib import Path

import rutis
import rutis.advanced

PACKAGE = str(Path(__file__).resolve().parent.parent)


def run(code: str, *flags: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, *flags, "-c", code], cwd=PACKAGE, capture_output=True, text=True)


class Advanced(unittest.TestCase):
    def test_a_moved_name_is_not_at_the_root(self):
        for name in rutis.advanced.__all__:
            with self.subTest(name=name):
                self.assertFalse(hasattr(rutis, name))
                self.assertNotIn(name, rutis.__all__)

    def test_importing_an_old_name_fails(self):
        result = run("from rutis import AggregateError")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ImportError: cannot import name 'AggregateError' from 'rutis'", result.stderr)

    def test_the_root_and_advanced_import_without_warnings(self):
        result = run("from rutis import define_plugin, Plugin, Context, RemoteError, BindingError; "
                     "from rutis.advanced import *", "-W", "error")
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
