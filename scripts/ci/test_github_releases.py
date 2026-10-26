# SPDX-License-Identifier: GPL-3.0-or-later
"""Release discovery must include drafts and fail closed on ambiguous listings."""
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from github_releases import find_release


class ReleaseDiscoveryTests(unittest.TestCase):
    def test_finds_drafts_and_published_releases_after_the_first_page(self):
        for draft in (True, False):
            expected = {"id": 42, "tag_name": "v1.0.0", "draft": draft}
            first = [{"id": n + 100, "tag_name": "v2.0.0", "draft": False} for n in range(100)]
            fetch = Mock(side_effect=[first, [expected]])
            self.assertEqual(find_release("v1.0.0", fetch), expected)
            self.assertEqual(fetch.call_args_list[1].args, ("releases?per_page=100&page=2",))

    def test_duplicate_drafts_or_published_releases_are_rejected(self):
        for draft in (True, False):
            matching = {"id": 42, "tag_name": "v1.0.0", "draft": draft}
            first = [matching] + [{"id": n + 100, "tag_name": "v2.0.0", "draft": False} for n in range(99)]
            with self.assertRaisesRegex(ValueError, "Multiple releases"):
                find_release("v1.0.0", Mock(side_effect=[first, [{**matching, "id": 43, "draft": True}]]))

    def test_absence_is_exact_and_errors_cannot_masquerade_as_absence(self):
        self.assertIsNone(find_release("v1.0.0", Mock(return_value=[{"tag_name": "v1.0.01"}])))
        with self.assertRaises(RuntimeError):
            find_release("v1.0.0", Mock(side_effect=RuntimeError("API failed")))
        for response in ({"message": "Not Found"}, [None], [{"id": True, "tag_name": "v1.0.0", "draft": True}]):
            with self.subTest(response=response), self.assertRaises(ValueError):
                find_release("v1.0.0", Mock(return_value=response))
        with self.assertRaisesRegex(ValueError, "verification limit"):
            find_release("v1.0.0", Mock(return_value=[{}] * 100))


if __name__ == '__main__':
    unittest.main()
