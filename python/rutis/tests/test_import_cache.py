"""A plugin module is imported though it was written after the import system
last listed its directory (#1139)."""

import importlib
import os
import sys
import tempfile
import unittest

import background

from rutis.runner import Runtime

# The directory's mtime when the listing is taken: set to it again once the
# plugin file is written, as Windows' coarse timestamps leave it (#1139).
LISTED = 1_000_000_000


class FirstImport(background.TestCase):
    """The finders cache each directory's listing, and take it again only
    when the directory's mtime changed since. A directory whose mtime is the
    one the listing was taken at, though a file was written since, is what
    Windows gives when both writes fall in one timestamp tick: the same state,
    made here on every platform."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.dir = os.path.realpath(directory.name)
        sys.path.insert(0, self.dir)
        self.addCleanup(sys.path.remove, self.dir)
        for name in ("early", "late"):
            self.addCleanup(sys.modules.pop, name, None)
        self.write("early.py", "marker = 1\n")
        os.utime(self.dir, (LISTED, LISTED))

    def write(self, name, text):
        with open(os.path.join(self.dir, name), "w") as file:
            file.write(text)

    def test_a_module_written_after_the_listing_is_imported(self):
        importlib.import_module("early")  # takes the listing
        # The plugin file: the directory's mtime is the one the listing saw,
        # so nothing says the listing is old, and importlib alone does not
        # find it.
        self.write("late.py", "inject = []\ndef apply(ctx, config):\n    pass\n")
        os.utime(self.dir, (LISTED, LISTED))
        with self.assertRaises(ModuleNotFoundError):
            importlib.import_module("late")
        module = Runtime().module("late")
        self.assertEqual(module.__name__, "late")
        self.assertIn("apply", module.__dict__)


class SameSecondEdit(background.TestCase):
    """A `.pyc` is current when its source's modification time, in whole
    seconds, and its size are the ones it recorded. An edit that keeps both
    — the same text length, saved in the second the last compile ran in —
    must still run (#1080)."""

    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.dir = os.path.realpath(directory.name)
        sys.path.insert(0, self.dir)
        self.addCleanup(sys.path.remove, self.dir)
        self.addCleanup(sys.modules.pop, "edited", None)

    def write(self, text, at):
        path = os.path.join(self.dir, "edited.py")
        with open(path, "w") as file:
            file.write(text)
        os.utime(path, (at, at))

    def test_an_edit_of_the_same_size_in_the_same_second_runs(self):
        self.write("VALUE = 'v1'\n", 1_000_000_000.1)
        runtime = Runtime()
        self.assertEqual(runtime.module("edited").VALUE, "v1")
        # The same size, and a modification time in the second the first
        # import's `.pyc` records: the bytecode Python cached is not current.
        self.write("VALUE = 'v2'\n", 1_000_000_000.7)
        self.assertEqual(runtime.module("edited").VALUE, "v2")


if __name__ == "__main__":
    unittest.main()
