"""The map-review page builder's pure parts."""

import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from tools import map_review
from tools.map_review import far_scrap_share, page, review_card


class FarScrapShare(unittest.TestCase):
    def test_far_share_counts_value_beyond_the_threshold(self) -> None:
        rows = [
            "1" + "." * 40,
            "s" + "." * 40,  # adjacent: near
            "." * 30 + "S" + "." * 10,  # 30 tiles out: far, worth 3
        ]
        share, total = far_scrap_share(rows)
        self.assertEqual(total, 4)
        self.assertAlmostEqual(share, 0.75)

    def test_a_map_with_no_scrap_reports_zero(self) -> None:
        share, total = far_scrap_share(["1..", "..."])
        self.assertEqual((share, total), (0.0, 0))


class Rendering(unittest.TestCase):
    def test_the_card_escapes_and_inlines(self) -> None:
        card = review_card(
            "test<map>",
            {"hook": "a & b", "pace": "grand"},
            {"size": "10 x 5"},
            b"pngbytes",
        )
        self.assertIn("test&lt;map&gt;", card)
        self.assertIn("a &amp; b", card)
        self.assertIn("data:image/png;base64,", card)
        self.assertNotIn("<map>", card)

    def test_the_page_is_a_complete_document(self) -> None:
        doc = page(["<section>one</section>"])
        self.assertTrue(doc.startswith("<!doctype html>"))
        self.assertIn("<section>one</section>", doc)
        self.assertIn("</html>", doc)


class ReviewBuild(unittest.TestCase):
    def test_a_failed_audit_without_json_has_no_structural_stats(self) -> None:
        completed = subprocess.CompletedProcess(
            ["fake-driver", "map-audit"],
            1,
            "map could not be audited\n",
            "details on stderr",
        )
        with patch.object(map_review.subprocess, "run", return_value=completed):
            self.assertIsNone(map_review.run_json(completed.args))

    def test_main_renders_sorted_drafts_and_includes_structural_audit(self) -> None:
        calls: list[list[str]] = []

        def driver(
            cmd: list[str], **kwargs: object
        ) -> subprocess.CompletedProcess[str]:
            calls.append(cmd)
            if cmd[1] == "render":
                Path(cmd[cmd.index("--out") + 1]).write_bytes(b"rendered png")
                return subprocess.CompletedProcess(cmd, 0, "", "")
            if cmd[1] == "map-audit":
                return subprocess.CompletedProcess(
                    cmd,
                    0,
                    '{"free_tiles": 37, "nodes": 2}\n',
                    "",
                )
            raise AssertionError(f"unexpected driver command: {cmd}")

        with tempfile.TemporaryDirectory(prefix="oxide-map-review-") as temp:
            root = Path(temp)
            drafts = root / "drafts"
            drafts.mkdir()
            for name, hook in (("zeta", "second"), ("alpha", "first & best")):
                (drafts / f"{name}.json").write_text(
                    "{\n"
                    f'  "meta": {{"hook": "{hook}", "pace": "quick"}},\n'
                    '  "players": [{}, {}],\n'
                    '  "map": ["1..................S", "....................", '
                    '"...................2"]\n'
                    "}\n",
                    encoding="utf-8",
                )
            output = root / "nested" / "review.html"
            argv = [
                "map_review.py",
                "--drafts",
                str(drafts),
                "--out",
                str(output),
                "--driver",
                "fake-driver",
            ]

            with (
                patch.object(sys, "argv", argv),
                patch.object(map_review.subprocess, "run", side_effect=driver),
                redirect_stdout(StringIO()),
            ):
                map_review.main()

            document = output.read_text(encoding="utf-8")
            self.assertLess(document.index("alpha"), document.index("zeta"))
            self.assertIn("first &amp; best", document)
            self.assertIn("<th>free tiles</th><td>37</td>", document)
            self.assertIn("<th>nodes</th><td>2</td>", document)
            self.assertIn("data:image/png;base64,cmVuZGVyZWQgcG5n", document)
            self.assertEqual(len(calls), 4)
            for draft in (drafts / "alpha.json", drafts / "zeta.json"):
                self.assertIn(["fake-driver", "map-audit", str(draft), "--json"], calls)

    def test_main_refuses_an_empty_draft_directory_before_running_driver(self) -> None:
        with tempfile.TemporaryDirectory(prefix="oxide-map-review-empty-") as temp:
            argv = ["map_review.py", "--drafts", temp]
            with (
                patch.object(sys, "argv", argv),
                patch.object(map_review.subprocess, "run") as driver,
                self.assertRaisesRegex(SystemExit, "no drafts"),
            ):
                map_review.main()
            driver.assert_not_called()


if __name__ == "__main__":
    unittest.main()
