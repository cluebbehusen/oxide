"""The line report's counting rules."""

import subprocess
import tempfile
import unittest
from pathlib import Path

from tools import line_report
from tools.line_report import collect, count, is_test_file, test_by_name

MIXED = """//! Crate docs.

use std::fmt;

/// A thing.
pub fn thing() -> &'static str {
    let brace = '{';
    let quote = '\\'';
    let text = "} not a close // nor a comment";
    let raw = r#"
    { "json": [1, 2] }
    "#;
    text
}

/* block
   comment */
#[cfg(not(test))]
fn production_only() {}

pub struct Holder {
    pub value: u8,
    #[cfg(test)]
    pub probe: Vec<(u8, u8)>,
    pub other: u8,
}

#[cfg(test)]
fn helper<A, B>(a: A, b: B) -> (A, B) {
    (a, b)
}

#[cfg(test)]
mod tests {
    #[test]
    fn works() {
        assert_eq!(super::thing(), "}");
    }
}

fn after() {}
"""


class Counting(unittest.TestCase):
    def test_cfg_test_items_are_test_and_the_rest_is_production(self) -> None:
        # Production: the use, the nine-line function with its raw string,
        # the cfg(not(test)) function, four Holder lines and `after`.
        # Test: the probe field, the four-line helper and the seven-line module.
        self.assertEqual(count(MIXED, whole_file_test=False), (17, 13))

    def test_a_test_file_counts_every_code_line_as_test(self) -> None:
        self.assertEqual(count(MIXED, whole_file_test=True), (0, 30))
        self.assertEqual(
            count("#![cfg(test)]\nfn a() {}\n", whole_file_test=False), (0, 2)
        )

    def test_blank_and_comment_only_lines_are_not_counted(self) -> None:
        self.assertEqual(
            count("// a\n\n/// b\n/* c\n d */\nfn e() {} // f\n", False), (1, 0)
        )

    def test_file_names_and_directories_mark_whole_test_files(self) -> None:
        for path in [
            "a/tests/x.rs",
            "a/src/tests.rs",
            "a/src/b/foo_tests.rs",
            "a/src/fixtures.rs",
        ]:
            self.assertTrue(test_by_name(path), path)
        for path in ["a/src/testsuite.rs", "a/src/contests.rs", "a/src/lib.rs"]:
            self.assertFalse(test_by_name(path), path)

    def test_cfg_test_module_declarations_mark_their_files_and_submodules(self) -> None:
        files = {
            "k/src/lib.rs": "#[cfg(test)]\nmod test_support;\npub mod real;\n",
            "k/src/test_support.rs": "pub mod nested;\n",
            "k/src/test_support/nested.rs": "fn n() {}\n",
            "k/src/real.rs": "#[cfg(test)] mod checks;\nfn r() {}\n",
            "k/src/real/checks.rs": "fn c() {}\n",
        }
        cache: dict[str, bool] = {}
        verdicts = {path: is_test_file(path, files.get, cache) for path in files}
        self.assertEqual(
            verdicts,
            {
                "k/src/lib.rs": False,
                "k/src/test_support.rs": True,
                "k/src/test_support/nested.rs": True,
                "k/src/real.rs": False,
                "k/src/real/checks.rs": True,
            },
        )


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        [
            "git",
            "-c",
            "user.name=Line Report",
            "-c",
            "user.email=line-report@example.com",
            "-c",
            "commit.gpgsign=false",
            *args,
        ],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def write(root: Path, path: str, text: str) -> None:
    file = root / path
    file.parent.mkdir(parents=True, exist_ok=True)
    file.write_text(text)


class Revisions(unittest.TestCase):
    def test_reports_net_lines_from_the_merge_base(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git(root, "init", "-q", "-b", "main")
            write(
                root,
                "k/src/lib.rs",
                "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n",
            )
            write(root, "k/src/old.rs", "fn gone() {}\nfn also_gone() {}\n")
            write(root, "notes.md", "not rust\n")
            git(root, "add", ".")
            git(root, "commit", "-q", "-m", "base")
            git(root, "switch", "-q", "-c", "feature")
            write(
                root,
                "k/src/lib.rs",
                "fn a() {}\nfn b() {}\n\n#[cfg(test)]\nmod test_support;\n"
                "#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n",
            )
            write(root, "k/src/test_support.rs", "pub fn s() {}\npub fn u() {}\n")
            write(root, "k/tests/it.rs", "#[test]\nfn it() {}\n")
            (root / "k/src/old.rs").unlink()
            write(root, "notes.md", "still not rust\n")
            git(root, "add", "-A")
            git(root, "commit", "-q", "-m", "feature")
            git(root, "switch", "-q", "main")
            write(root, "m/src/lib.rs", "fn later() {}\n")
            git(root, "add", ".")
            git(root, "commit", "-q", "-m", "main moves on")

            report = collect(root, "main", "feature")
            self.assertEqual(report.files, 4)
            self.assertEqual(report.areas, {"k": [-1, 6]})

            git(root, "switch", "-q", "feature")
            write(root, "k/src/extra.rs", "fn extra() {}\n")
            write(root, "k/src/test_support.rs", "pub fn s() {}\n")
            report = collect(root, "main", working_tree=True)
            self.assertEqual(report.areas, {"k": [0, 5]})
            self.assertIn("total", line_report.render(report))


if __name__ == "__main__":
    unittest.main()
