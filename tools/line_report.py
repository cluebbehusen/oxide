# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Reports net production and test line changes in Rust code.

Usage (from the repository root):
    uv run tools/line_report.py BASE                  # to HEAD
    uv run tools/line_report.py BASE --head REF       # to REF
    uv run tools/line_report.py BASE --working-tree   # to uncommitted files

Counting method:
  - Revisions: the merge base of BASE and the head (HEAD, --head REF, or HEAD
    for --working-tree) against that head, like `git diff BASE...HEAD`. The
    working tree includes uncommitted changes to tracked files and untracked,
    non-ignored files.
  - Files: every .rs file that differs between the two, anywhere in the
    repository, including those added or deleted.
  - Lines: a counted line holds code. Blank lines and lines holding only
    comments (//, ///, //!, /* */) are not counted. Lines inside string
    literals are code.
  - Test code: whole files under a tests/ directory; files named tests.rs,
    *_tests.rs or fixtures.rs; files with #![cfg(test)]; and files declared by
    their parent module as `#[cfg(test)] mod name;`, together with their
    submodules. Inside any other file, each #[cfg(test)] item is test code,
    from the attribute to the item's end. Everything else is production code.
  - Net: counted lines at the head minus counted lines at the merge base, per
    category, grouped by top-level directory.
"""

from __future__ import annotations

import argparse
import functools
import re
import subprocess
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath

TEST_ATTRIBUTE = re.compile(r"#\s*(!?)\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
ITEM = re.compile(
    r"""
    (?:\s|//[^\n]*|/\*.*?\*/|\#\s*\[[^\]]*\])*
    (?:pub(?:\s*\([^)]*\))?\s+)?
    (?:(?:unsafe|async|const|default|extern(?:\s+"[^"]*")?)\s+)*
    (?:(?:fn|mod|impl|struct|enum|trait|union|const|static|type|use|extern)\b|macro_rules!)
    """,
    re.DOTALL | re.VERBOSE,
)
RAW_STRING = re.compile(r'(#*)"')
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

Reader = Callable[[str], "str | None"]


@dataclass
class Region:
    start: int
    depth: int
    item: bool


@dataclass
class Classified:
    code: list[bool]
    test: list[bool]
    file_test: bool = False


@functools.lru_cache(maxsize=64)
def classify(source: str) -> Classified:
    """Marks each line as code or not, and as inside a #[cfg(test)] item."""
    lines = source.count("\n") + 1
    out = Classified([False] * lines, [False] * lines)
    region: Region | None = None
    depth = 0
    line = 0
    i = 0
    end = len(source)

    def close(at: int) -> None:
        nonlocal region
        assert region is not None
        for index in range(region.start, at + 1):
            out.test[index] = True
        region = None

    def span(start: int, stop: int) -> None:
        nonlocal line
        newlines = source.count("\n", start, stop)
        for index in range(line, line + newlines + 1):
            out.code[index] = True
        line += newlines

    while i < end:
        c = source[i]
        if c == "\n":
            line += 1
            i += 1
            continue
        if c in " \t\r":
            i += 1
            continue
        if source.startswith("//", i):
            newline = source.find("\n", i)
            i = end if newline < 0 else newline
            continue
        if source.startswith("/*", i):
            level = 0
            while i < end:
                if source.startswith("/*", i):
                    level += 1
                    i += 2
                elif source.startswith("*/", i):
                    level -= 1
                    i += 2
                    if level == 0:
                        break
                else:
                    if source[i] == "\n":
                        line += 1
                    i += 1
            continue
        out.code[line] = True
        if c.isalpha() or c == "_":
            token = IDENTIFIER.match(source, i)
            assert token is not None
            i = token.end()
            raw = RAW_STRING.match(source, i)
            if token.group() in ("r", "br", "cr") and raw:
                closing = '"' + raw.group(1)
                stop = source.find(closing, raw.end())
                stop = end if stop < 0 else stop + len(closing)
                span(i, stop)
                i = stop
            continue
        if c == '"':
            j = i + 1
            while j < end and source[j] != '"':
                j += 2 if source[j] == "\\" else 1
            span(i, j + 1)
            i = j + 1
            continue
        if c == "'":
            if source.startswith("\\", i + 1):
                stop = source.find("'", i + 3)
                i = end if stop < 0 else stop + 1
            elif source.startswith("'", i + 2):
                i += 3
            else:
                i += 1
            continue
        if c == "#":
            attribute = TEST_ATTRIBUTE.match(source, i)
            if attribute:
                if attribute.group(1):
                    out.file_test = True
                elif region is None:
                    item = ITEM.match(source, attribute.end()) is not None
                    region = Region(line, depth, item)
                span(i, attribute.end())
                i = attribute.end()
                continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if region and (
                depth < region.depth or (c == "}" and depth == region.depth)
            ):
                close(line)
        elif (
            region
            and depth == region.depth
            and (c == ";" or (c == "," and not region.item))
        ):
            close(line)
        i += 1
    if region:
        close(lines - 1)
    return out


def count(source: str, whole_file_test: bool) -> tuple[int, int]:
    """Counted (production, test) lines of one file."""
    classified = classify(source)
    everything_test = whole_file_test or classified.file_test
    production = test = 0
    for code, in_test in zip(classified.code, classified.test):
        if not code:
            continue
        if everything_test or in_test:
            test += 1
        else:
            production += 1
    return production, test


def test_by_name(path: str) -> bool:
    parts = PurePosixPath(path).parts
    name = parts[-1]
    return (
        "tests" in parts[:-1]
        or name == "tests.rs"
        or name.endswith("_tests.rs")
        or name == "fixtures.rs"
    )


def declaring_modules(path: str) -> tuple[str, list[str]] | None:
    """The module name a file defines and the files that may declare it."""
    file = PurePosixPath(path)
    if file.name in ("lib.rs", "main.rs", "build.rs"):
        return None
    if file.name == "mod.rs":
        name, directory = file.parent.name, file.parent.parent
    else:
        name, directory = file.stem, file.parent
    if not name or str(directory) == ".":
        return None
    parents = [f"{directory}.rs"] + [
        f"{directory}/{root}" for root in ("mod.rs", "lib.rs", "main.rs")
    ]
    return name, parents


def declaration(source: str, name: str) -> bool | None:
    """Whether `mod name;` in `source` is test code, or None if undeclared."""
    pattern = re.compile(
        rf"^\s*(?:#\s*\[[^\]]*\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+{re.escape(name)}\s*;"
    )
    classified = classify(source)
    for index, text in enumerate(source.split("\n")):
        if classified.code[index] and pattern.match(text):
            return classified.file_test or classified.test[index]
    return None


def is_test_file(path: str, read: Reader, cache: dict[str, bool]) -> bool:
    if path in cache:
        return cache[path]
    result = test_by_name(path)
    module = None if result else declaring_modules(path)
    if module:
        name, parents = module
        for parent in parents:
            source = read(parent)
            declared = None if source is None else declaration(source, name)
            if declared is not None:
                result = declared or is_test_file(parent, read, cache)
                break
    cache[path] = result
    return result


class GitReader:
    """Reads files at one revision through a single `git cat-file` process."""

    def __init__(self, root: Path, revision: str) -> None:
        self.revision = revision
        self.cache: dict[str, str | None] = {}
        self.process = subprocess.Popen(
            ["git", "cat-file", "--batch"],
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
        )

    def __call__(self, path: str) -> str | None:
        if path not in self.cache:
            assert self.process.stdin and self.process.stdout
            self.process.stdin.write(f"{self.revision}:{path}\n".encode())
            self.process.stdin.flush()
            header = self.process.stdout.readline().decode().split()
            if len(header) == 3 and header[1] == "blob":
                data = self.process.stdout.read(int(header[2]))
                self.process.stdout.read(1)
                self.cache[path] = data.decode("utf-8", errors="replace")
            else:
                self.cache[path] = None
        return self.cache[path]

    def close(self) -> None:
        assert self.process.stdin and self.process.stdout
        self.process.stdin.close()
        self.process.wait()
        self.process.stdout.close()


class WorkingTreeReader:
    def __init__(self, root: Path) -> None:
        self.root = root

    def __call__(self, path: str) -> str | None:
        file = self.root / path
        return file.read_text(errors="replace") if file.is_file() else None

    def close(self) -> None:
        pass


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, capture_output=True, text=True
    ).stdout


@dataclass
class Report:
    base: str
    head: str
    files: int = 0
    areas: dict[str, list[int]] = field(default_factory=dict)

    def total(self) -> tuple[int, int]:
        return (
            sum(net[0] for net in self.areas.values()),
            sum(net[1] for net in self.areas.values()),
        )


def collect(
    root: Path, base: str, head: str = "HEAD", working_tree: bool = False
) -> Report:
    """Net production and test lines per top-level directory."""
    merge_base = git(root, "merge-base", base, head).strip()
    if working_tree:
        changed = git(
            root, "diff", "--name-only", "--no-renames", "-z", merge_base, "--", "*.rs"
        )
        changed += git(
            root, "ls-files", "--others", "--exclude-standard", "-z", "--", "*.rs"
        )
        after: GitReader | WorkingTreeReader = WorkingTreeReader(root)
        head_label = "working tree"
    else:
        changed = git(
            root,
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            merge_base,
            head,
            "--",
            "*.rs",
        )
        after = GitReader(root, head)
        head_label = f"{head} ({git(root, 'rev-parse', '--short', head).strip()})"
    before = GitReader(root, merge_base)
    report = Report(f"{base} (merge base {merge_base[:10]})", head_label)
    sides: list[tuple[Reader, dict[str, bool], int]] = [
        (before, {}, -1),
        (after, {}, 1),
    ]
    try:
        for path in sorted(set(filter(None, changed.split("\0")))):
            report.files += 1
            area = PurePosixPath(path).parts[0] if "/" in path else "(root)"
            net = report.areas.setdefault(area, [0, 0])
            for read, cache, sign in sides:
                source = read(path)
                if source is None:
                    continue
                production, test = count(source, is_test_file(path, read, cache))
                net[0] += sign * production
                net[1] += sign * test
    finally:
        before.close()
        after.close()
    return report


def render(report: Report) -> str:
    rows = [
        f"Rust lines from {report.base} to {report.head}, {report.files} .rs files changed",
        "",
    ]
    rows.append(f"{'area':<16}{'production':>12}{'test':>10}")
    for area, (production, test) in sorted(report.areas.items()):
        rows.append(f"{area:<16}{production:>+12d}{test:>+10d}")
    production, test = report.total()
    rows.append(f"{'total':<16}{production:>+12d}{test:>+10d}")
    return "\n".join(rows)


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("base", help="base revision, such as origin/main")
    target = parser.add_mutually_exclusive_group()
    target.add_argument("--head", default="HEAD", help="head revision (default HEAD)")
    target.add_argument(
        "--working-tree",
        action="store_true",
        help="count the working tree, including uncommitted and untracked files",
    )
    args = parser.parse_args()
    root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip())
    print(render(collect(root, args.base, args.head, args.working_tree)))


if __name__ == "__main__":
    main()
