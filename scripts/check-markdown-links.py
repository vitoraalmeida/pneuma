#!/usr/bin/env python3
"""Fail when a tracked Markdown document has a missing local relative link."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote


LINK = re.compile(r"(?<!!)\[[^]]*\]\(([^)\s]+)(?:\s+[^)]*)?\)")
IGNORED_PREFIXES = ("#", "http://", "https://", "mailto:")


def tracked_markdown_files() -> list[Path]:
    tracked = subprocess.run(
        ["git", "ls-files", "*.md"], check=True, capture_output=True, text=True
    ).stdout.splitlines()
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard", "*.md"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.splitlines()
    return [
        Path(path)
        for path in sorted(set(tracked + untracked))
        if Path(path).is_file()
    ]


def local_target(raw_target: str) -> str | None:
    target = unquote(raw_target.split("#", 1)[0])
    if not target or target.startswith(IGNORED_PREFIXES):
        return None
    return target


def main() -> int:
    missing: list[str] = []
    for document in tracked_markdown_files():
        for line_number, line in enumerate(document.read_text().splitlines(), start=1):
            for match in LINK.finditer(line):
                target = local_target(match.group(1))
                if target is not None and not (document.parent / target).exists():
                    missing.append(f"{document}:{line_number}: {match.group(1)}")

    if missing:
        print("Missing local Markdown links:", file=sys.stderr)
        print("\n".join(missing), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
