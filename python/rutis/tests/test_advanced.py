"""The names that moved to `rutis.advanced` in 0.19 (#692): the old root
names still work, warn with the new place, and fail under `-W error`."""

import subprocess
import sys
import unittest
import warnings
from pathlib import Path

import rutis
import rutis.advanced

PACKAGE = str(Path(__file__).resolve().parent.parent)
MESSAGE = ("AggregateError moved in 0.19: import it from rutis.advanced. "
           "The old name is removed when the plugin API version changes.")


def run(code: str, *flags: str) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, *flags, "-c", code], cwd=PACKAGE, capture_output=True, text=True)


class OldNames(unittest.TestCase):
    def test_an_old_name_works_and_warns_once_with_the_new_place(self):
        result = run("from rutis import AggregateError\n"
                     "import rutis.advanced\n"
                     "assert AggregateError is rutis.advanced.AggregateError\n"
                     "import rutis; rutis.AggregateError", "-W", "always::DeprecationWarning")
        self.assertEqual(result.returncode, 0, result.stderr)
        # Once, at the user's line, not in the import machinery.
        self.assertEqual(result.stderr, f"<string>:1: DeprecationWarning: {MESSAGE}\n")

    def test_every_moved_name_is_the_advanced_one(self):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            for name in rutis.advanced.__all__:
                with self.subTest(name=name):
                    self.assertIs(getattr(rutis, name), getattr(rutis.advanced, name))
                    self.assertIn(name, rutis.__all__)

    def test_star_import_still_gives_every_name_and_warns(self):
        result = run("scope = {}\n"
                     "exec('from rutis import *', scope)\n"
                     "import rutis\n"
                     "assert set(rutis.__all__) <= set(scope), set(rutis.__all__) - set(scope)",
                     "-W", "always::DeprecationWarning")
        self.assertEqual(result.returncode, 0, result.stderr)
        warned = sorted(line.split("DeprecationWarning: ")[1].split()[0] for line in result.stderr.splitlines())
        self.assertEqual(warned, sorted(rutis.advanced.__all__))

    def test_the_root_names_do_not_warn(self):
        result = run("from rutis import define_plugin, Plugin, Context, RemoteError, BindingError; "
                     "from rutis.advanced import *", "-W", "error")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_under_w_error_an_old_name_fails(self):
        for code in ("from rutis import AggregateError", "from rutis import *"):
            with self.subTest(code=code):
                result = run(code, "-W", "error")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("DeprecationWarning: ", result.stderr)
                self.assertIn("moved in 0.19: import it from rutis.advanced", result.stderr)

    def test_an_unknown_name_is_an_attribute_error(self):
        with self.assertRaises(AttributeError):
            rutis.NoSuchName


if __name__ == "__main__":
    unittest.main()
