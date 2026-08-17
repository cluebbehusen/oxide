"""The map-review page builder's pure parts."""

import unittest

from tools.map_review import far_scrap_share, page, probe_summary, review_card


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


class Summaries(unittest.TestCase):
    def test_no_flips_reads_robust(self) -> None:
        row = {"helped": 0, "hurt": 0, "stalled": 0, "games": 24}
        self.assertIn("robust", probe_summary(row))

    def test_flips_read_fragile_with_the_split(self) -> None:
        row = {"helped": 12, "hurt": 0, "stalled": 12, "games": 24}
        summary = probe_summary(row)
        self.assertIn("fragile", summary)
        self.assertIn("helped 12", summary)
        self.assertIn("stalled 12", summary)


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


if __name__ == "__main__":
    unittest.main()
