"""Which call ids a reference's origin may hold, in either format."""

import unittest

from rutis.peer import _COMPAT_ORIGIN, _ENDPOINT_ORIGIN


class OriginIdTests(unittest.TestCase):
    def test_compat_takes_this_sessions_ids_and_other_sessions_tagged_ones(self):
        # Another session's ids come tagged with it, whatever its format:
        # refusing them ended the session of a nested call (#225).
        for id in ["node:1", "rust:12", "s1/rust:7", "s1/node:2", "s12/mac-2:3"]:
            self.assertTrue(_COMPAT_ORIGIN.match(id), id)
        for id in ["py:1", "node:0", "s1/s2/node:1", "s1/node", "S1/node:1", "node:01"]:
            self.assertFalse(_COMPAT_ORIGIN.match(id), id)

    def test_endpoint_takes_any_endpoint_tagged_or_not(self):
        for id in ["mac:1", "s3/mac:4"]:
            self.assertTrue(_ENDPOINT_ORIGIN.match(id), id)
        self.assertFalse(_ENDPOINT_ORIGIN.match("s1/s2/mac:1"))


if __name__ == "__main__":
    unittest.main()
