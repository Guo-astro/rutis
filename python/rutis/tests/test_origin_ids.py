"""Which call ids a reference's origin may hold."""

import unittest

from rutis.peer import _ENDPOINT_ORIGIN


class OriginIdTests(unittest.TestCase):
    def test_takes_any_endpoint_tagged_or_not(self):
        # Another session's ids come tagged with it: refusing them ended the
        # session of a nested call (#225).
        for id in ["mac:1", "s3/mac:4", "main:12", "s12/mac-2:3"]:
            self.assertTrue(_ENDPOINT_ORIGIN.match(id), id)
        for id in ["mac:0", "s1/s2/mac:1", "s1/mac", "S1/mac:1", "mac:01"]:
            self.assertFalse(_ENDPOINT_ORIGIN.match(id), id)


if __name__ == "__main__":
    unittest.main()
