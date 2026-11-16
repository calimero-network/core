#!/usr/bin/env python3
"""Changelog entries: one file per pull request in changelog.d/, gathered into
CHANGELOG.md when a release is cut.

Every pull request that changes behaviour adds its own file, so no two pull requests
edit the same lines of CHANGELOG.md and none conflicts with another over it. A file
holds one or more sections, each a `### <Section>` heading and its bullets:

    ### Fixed

    - **A snapshot built during an install holds the right state** (`calimero-log`):
      ...

    python3 scripts/changelog.py check                 # every entry file is well formed
    python3 scripts/changelog.py preview               # the section the next release gets
    python3 scripts/changelog.py release X [--date D]  # the release PR: gather and delete

`release` turns the entries (and anything still written under `## [Unreleased]`)
into `## [X] - D`, in the order the entries merged, and deletes the files.
The policy: RELEASING.md, "How a release is cut".
"""

from __future__ import annotations

import argparse
import datetime
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ENTRIES = ROOT / "changelog.d"
CHANGELOG = ROOT / "CHANGELOG.md"

# The order a release's sections are written in; an entry may use only these.
SECTIONS = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
    "Dependencies",
    "Documentation",
    "CI",
]
HEADING_RE = re.compile(r"^###\s+(.+?)\s*$")
UNRELEASED = "## [Unreleased]"


def entry_files(entries: Path = ENTRIES) -> list[Path]:
    """The entry files, oldest merge first (by the commit that added each; then by name)."""
    files = [p for p in entries.glob("*.md") if p.name != "README.md"] if entries.is_dir() else []

    def added_at(path: Path) -> int:
        try:
            out = subprocess.run(
                ["git", "log", "--diff-filter=A", "--format=%ct", "-1", "--", str(path)],
                cwd=entries.parent,
                capture_output=True,
                text=True,
                check=False,
            ).stdout.strip()
            return int(out) if out else sys.maxsize
        except (OSError, ValueError):
            return sys.maxsize

    return sorted(files, key=lambda p: (added_at(p), p.name))


def parse(text: str, name: str = "<entry>") -> tuple[dict[str, list[str]], list[str]]:
    """The bullets of each section in `text`, and what is wrong with it."""
    sections: dict[str, list[str]] = {}
    problems: list[str] = []
    current: str | None = None
    skipping = False  # inside a section already reported as unknown
    block: list[str] = []

    def close() -> None:
        if current is not None:
            body = "\n".join(block).strip("\n")
            if not body.lstrip().startswith("- "):
                problems.append(f"{name}: `### {current}` needs at least one `- ` bullet")
            else:
                sections.setdefault(current, []).append(body)

    for line in text.splitlines():
        m = HEADING_RE.match(line)
        if m:
            close()
            current, block, skipping = m.group(1), [], False
            if current not in SECTIONS:
                problems.append(f"{name}: `### {current}` is not one of: {', '.join(SECTIONS)}")
                current, skipping = None, True
            continue
        if line.startswith("#"):
            problems.append(f"{name}: only `### <Section>` headings belong in an entry: {line!r}")
            continue
        if current is None:
            if not skipping and line.strip() and not line.lstrip().startswith("<!--"):
                problems.append(f"{name}: text before the first `### <Section>` heading: {line!r}")
            continue
        block.append(line)
    close()
    if not sections and not problems:
        problems.append(f"{name}: no `### <Section>` heading with entries")
    return sections, problems


def split_unreleased(changelog: str) -> tuple[str, str, str]:
    """CHANGELOG.md as (everything up to `## [Unreleased]`, its body, the releases after)."""
    start = changelog.index(UNRELEASED) + len(UNRELEASED)
    nxt = re.search(r"^## \[", changelog[start:], re.MULTILINE)
    end = start + nxt.start() if nxt else len(changelog)
    return changelog[:start], changelog[start:end], changelog[end:]


def gather(texts: list[tuple[str, str]]) -> tuple[str, list[str]]:
    """The release body for `texts` ((name, text), in order), and what is wrong with them."""
    merged: dict[str, list[str]] = {}
    problems: list[str] = []
    for name, text in texts:
        sections, found = parse(text, name)
        problems += found
        for section, bodies in sections.items():
            merged.setdefault(section, []).extend(bodies)
    parts = [f"### {s}\n\n" + "\n\n".join(merged[s]) for s in SECTIONS if s in merged]
    return "\n\n".join(parts), problems


def unreleased_text(changelog: str) -> str:
    """What is still written under `## [Unreleased]`, without its comments."""
    body = split_unreleased(changelog)[1]
    return re.sub(r"<!--.*?-->", "", body, flags=re.DOTALL).strip()


def release_body(changelog: str, files: list[Path]) -> tuple[str, list[str]]:
    texts = []
    legacy = unreleased_text(changelog)
    if legacy:
        texts.append((UNRELEASED, legacy))
    texts += [(f"changelog.d/{p.name}", p.read_text()) for p in files]
    return gather(texts)


def release(changelog: str, version: str, date: str, body: str) -> str:
    """CHANGELOG.md with `body` as `## [version] - date` and an empty `## [Unreleased]`."""
    head, unreleased, rest = split_unreleased(changelog)
    comments = "".join(re.findall(r"\n*<!--.*?-->", unreleased, flags=re.DOTALL))
    section = f"## [{version}] - {date}\n\n{body}\n\n" if body else f"## [{version}] - {date}\n\n"
    return f"{head}{comments}\n\n{section}{rest.lstrip()}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("check", help="every entry file is well formed")
    sub.add_parser("preview", help="print the section the next release gets")
    cut = sub.add_parser("release", help="gather the entries into CHANGELOG.md and delete them")
    cut.add_argument("version")
    cut.add_argument("--date", default=datetime.date.today().isoformat())
    args = parser.parse_args(argv)

    changelog = CHANGELOG.read_text()
    if UNRELEASED not in changelog:
        print(f"::error::CHANGELOG.md has no `{UNRELEASED}` heading")
        return 1
    files = entry_files()
    body, problems = release_body(changelog, files)
    for problem in problems:
        print(f"::error::Changelog: {problem}")
    if problems:
        return 1

    if args.command == "check":
        print(f"Changelog: {len(files)} entry file(s) well formed.")
    elif args.command == "preview":
        print(body or "(no entries since the last release)")
    else:
        if f"## [{args.version}]" in changelog:
            print(f"::error::CHANGELOG.md already has a `## [{args.version}]` section")
            return 1
        CHANGELOG.write_text(release(changelog, args.version, args.date, body))
        for p in files:
            p.unlink()
        print(f"CHANGELOG.md: `## [{args.version}] - {args.date}` from {len(files)} entry file(s).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
