#!/usr/bin/env python3
"""Every reference in the code's comments and AGENTS.md files must lead somewhere.

A reviewer reading a comment should be able to follow what it cites: open the
docs page, open the file, read the rule. This gate reads the comments of every
Rust source and every AGENTS.md under crates/, apps/ and sdks/ and checks four
kinds of reference:

  * site-link  - a link to the docs site (https://calimero-network.github.io/core/...)
                 must name a page under docs/src/content/docs/, never a path the
                 site only redirects (docs/astro.config.mjs `redirects`), and its
                 #anchor must be a heading of that page. Renaming a heading that
                 code links to fails here.
  * dead-path  - a repository path in backticks or a Markdown link target
                 (`crates/...`, `docs/...`, `scripts/...`, ...) must exist,
                 relative to the repository root or to the citing file's
                 directory or one of its parents (an SDK's AGENTS.md names its
                 own `scripts/`).
  * bare-tag   - a short id such as `(D20)`, `R8`, `review finding F6` or
                 `Ruling 39` tells a reader nothing and links nowhere. Write the
                 rule out, link the page that states it, or drop the id.
  * bare-issue - `#4839` is not a link. Use the full issue URL for open work;
                 drop it where it only records history (git blame has it).

site-link and dead-path must have no findings: a statement whose path is right
for a reason the check cannot see (it names a file of another repository, or a
file that is described as removed) goes in ALLOWLIST, one line each:
`<file> | <a quoted piece of the line> | <why>`. An entry that matches nothing is
reported, so the list shrinks as sites are fixed.

bare-tag and bare-issue predate this gate, so they are held to BASELINE: the
count each file has today. A file may only go down. After cleaning a file, run
with --update-baseline and commit the smaller baseline.

Inline code (`...`) is not prose: a constant such as `T0` or `RC16` is not a tag,
and a path in backticks is checked as a path, not scanned for tags.

Usage:
  python3 scripts/check-code-refs.py                    # the gate
  python3 scripts/check-code-refs.py --list [<check>]   # every finding, baselined ones too
  python3 scripts/check-code-refs.py --update-baseline  # write today's counts
"""

from __future__ import annotations

import argparse
import re
import sys
import unicodedata
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs" / "src" / "content" / "docs"
ASTRO_CONFIG = ROOT / "docs" / "astro.config.mjs"
ALLOWLIST = ROOT / "scripts" / "check-code-refs.allow"
BASELINE = ROOT / "scripts" / "check-code-refs.baseline"

SCAN_DIRS = ("crates", "apps", "sdks")
SKIP_PARTS = {"target", "node_modules", "Generated", "generated", ".gradle", "build", "dist"}

SITE = "https://calimero-network.github.io/core/"
SITE_LINK = re.compile(r"https://calimero-network\.github\.io/core/([A-Za-z0-9_./-]*)(#[A-Za-z0-9_-]+)?")

PATH_ROOTS = ("crates", "apps", "sdks", "docs", "scripts", ".github", "fuzz")
BACKTICKED = re.compile(r"`([^`\n]+)`")
MD_LINK_TARGET = re.compile(r"\]\(([^)\s#]+)(?:#[^)\s]*)?\)")
# A path is at least two segments and ends in a name, not a placeholder.
PATH_SHAPE = re.compile(r"^(?:\.\./)*(?:\.github|[A-Za-z0-9_-]+)(?:/[A-Za-z0-9_.@-]+)+/?$")

TAG = re.compile(r"\b(?:[A-Z]{1,2}\d{1,3}[a-z]?|Ruling \d+)\b")
# Not tags: types, algorithms, curves, encodings, hardware.
NOT_TAGS = {
    *(f"U{n}" for n in (8, 16, 32, 64, 128)), *(f"I{n}" for n in (8, 16, 32, 64, 128)),
    "F32", "F64", "ES256", "ES384", "RS256", "PS256", "P256", "P384", "P521",
    "SHA1", "SHA2", "SHA3", "SHA256", "SHA384", "SHA512", "BM25", "LZ4", "B58", "BS58",
    "BIP32", "BIP39", "BIP44", "H2", "H3", "M1", "M2", "M3", "M4", "IPV4", "IPV6",
    "SEC1", "EC2", "NAT64", "AES128", "AES256", "UTF8", "UTF16", "X509", "K8S", "S3",
    "E2E", "L0", "L1", "L2", "P2P", "V1", "V2", "V3", "B2B",
}
# `#4723`, `(#4723)`; not `&#1234;`, `[#1234]`-style anchors, `x#1234` or a URL's fragment.
ISSUE = re.compile(r"(?<![\w/&\[#])#(\d{3,5})\b")

FENCE = re.compile(r"^\s*(```|~~~)")


# ---------- the docs site

def slug(text: str) -> str:
    """github-slugger, which Starlight uses for heading ids."""
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)  # a link keeps its words
    text = re.sub(r"<[^>]+>", "", text)
    text = text.strip().lower()
    out = []
    for ch in text:
        cat = unicodedata.category(ch)
        if ch in "-_ " or cat[0] in "LN" or cat == "Mn":
            out.append(ch)
    return "".join(out).replace(" ", "-")


def page_anchors(page: Path) -> set[str]:
    anchors: set[str] = set()
    seen: Counter = Counter()
    in_fence = False
    for line in page.read_text(encoding="utf-8").splitlines():
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        m = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", line)
        if m:
            base = slug(m.group(2))
            anchors.add(base if seen[base] == 0 else f"{base}-{seen[base]}")
            seen[base] += 1
        for explicit in re.findall(r"""\bid=["']([^"']+)["']""", line):
            anchors.add(explicit)
    return anchors


def site_page(path: str) -> Path | None:
    path = path.strip("/")
    candidates = [DOCS / "index.mdx", DOCS / "index.md"] if not path else [
        DOCS / f"{path}.mdx", DOCS / f"{path}.md", DOCS / path / "index.mdx", DOCS / path / "index.md",
    ]
    return next((p for p in candidates if p.is_file()), None)


def redirects() -> set[str]:
    if not ASTRO_CONFIG.is_file():
        return set()
    text = ASTRO_CONFIG.read_text(encoding="utf-8")
    block = re.search(r"redirects:\s*\{(.*?)\n\s*\}", text, re.S)
    if not block:
        return set()
    return {k.strip("/") for k in re.findall(r"""['"](/[^'"]+)['"]\s*:""", block.group(1))}


# ---------- sources

def sources() -> list[Path]:
    files = []
    for top in SCAN_DIRS:
        for p in sorted((ROOT / top).rglob("*")):
            if SKIP_PARTS.intersection(p.relative_to(ROOT).parts) or p.is_symlink() or not p.is_file():
                continue
            if p.suffix == ".rs" or p.name == "AGENTS.md":
                files.append(p)
    return files


def prose_lines(path: Path):
    """(line number, text) of a file's prose: a Rust file's comments, a Markdown file outside code fences."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    if path.suffix == ".rs":
        for n, line in enumerate(lines, 1):
            i = line.find("//")
            if i < 0:
                continue
            before = line[:i]
            if before.count('"') % 2 == 1:  # `//` inside a string literal (a URL)
                j = line.find("//", i + 2)
                if j < 0 or line[:j].count('"') % 2 == 1:
                    continue
                i = j
            yield n, line[i:]
        return
    in_fence = False
    for n, line in enumerate(lines, 1):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence:
            yield n, line


def path_exists(ref: str, cited_from: Path) -> bool:
    ref = ref.rstrip("/")
    if (ROOT / ref).exists():
        return True
    d = cited_from.parent
    while True:
        if (d / ref).exists():
            return True
        if d == ROOT or d == d.parent:
            return False
        d = d.parent


def looks_like_path(token: str) -> bool:
    if any(c in token for c in "<>{}*$…") or "..." in token or " " in token:
        return False
    if not PATH_SHAPE.match(token):
        return False
    first = token.lstrip("./").split("/")[0]
    return token.startswith("../") or first in PATH_ROOTS


# ---------- the checks

def findings():
    gone = redirects()
    anchors_of: dict[Path, set[str]] = {}
    for path in sources():
        rel = path.relative_to(ROOT).as_posix()
        for n, line in prose_lines(path):
            for m in SITE_LINK.finditer(line):
                page_path, anchor = m.group(1), m.group(2)
                page_path = page_path.rstrip(".")
                if page_path.strip("/") in gone:
                    yield "site-link", rel, n, line, f"{m.group(0)} is a redirect; link the page it redirects to"
                    continue
                page = site_page(page_path)
                if page is None:
                    yield "site-link", rel, n, line, f"{m.group(0)}: no page docs/src/content/docs/{page_path.strip('/')}.mdx"
                    continue
                if anchor:
                    if page not in anchors_of:
                        anchors_of[page] = page_anchors(page)
                    if anchor[1:] not in anchors_of[page]:
                        yield "site-link", rel, n, line, f"{m.group(0)}: no heading #{anchor[1:]} on {page.relative_to(ROOT)}"

            refs = [t for t in BACKTICKED.findall(line)]
            if path.suffix == ".md":
                refs += MD_LINK_TARGET.findall(line)
            for ref in refs:
                ref = ref.strip().rstrip(".,;:")
                if ref.startswith(("http://", "https://")) or not looks_like_path(ref):
                    continue
                target = ref
                if path.suffix == ".md" and ref.startswith("../"):
                    target = (path.parent / ref).resolve().relative_to(ROOT).as_posix() if (path.parent / ref).resolve().is_relative_to(ROOT) else ref
                if not path_exists(target, path):
                    yield "dead-path", rel, n, line, f"{ref} does not exist"

            prose = BACKTICKED.sub(" ", line)
            prose = SITE_LINK.sub(" ", prose)
            prose = re.sub(r"https?://\S+", " ", prose)
            for m in TAG.finditer(prose):
                if m.group(0) not in NOT_TAGS:
                    yield "bare-tag", rel, n, line, m.group(0)
            for m in ISSUE.finditer(prose):
                yield "bare-issue", rel, n, line, f"#{m.group(1)}"


def load_allowlist():
    entries = []
    if ALLOWLIST.is_file():
        for raw in ALLOWLIST.read_text(encoding="utf-8").splitlines():
            if not raw.strip() or raw.lstrip().startswith("#"):
                continue
            parts = [p.strip() for p in raw.split(" | ")]
            if len(parts) != 3 or not all(parts):
                sys.exit(f"{ALLOWLIST.name}: want `<file> | <quoted piece> | <why>`: {raw}")
            entries.append(parts)
    return entries


def load_baseline() -> Counter:
    base: Counter = Counter()
    if BASELINE.is_file():
        for raw in BASELINE.read_text(encoding="utf-8").splitlines():
            if not raw.strip() or raw.startswith("#"):
                continue
            check, count, file = raw.split(maxsplit=2)
            base[(check, file)] = int(count)
    return base


BASELINE_HEADER = """\
# Counts of bare-tag and bare-issue findings per file (scripts/check-code-refs.py).
# A file may only go down: after cleaning one, run
#   python3 scripts/check-code-refs.py --update-baseline
# and commit the smaller numbers. Never raise one by hand.
"""

BASELINED = ("bare-tag", "bare-issue")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--list", nargs="?", const="all", metavar="CHECK", help="print every finding (of one check)")
    ap.add_argument("--update-baseline", action="store_true", help="write today's bare-tag/bare-issue counts")
    args = ap.parse_args()

    allow = load_allowlist()
    used = [False] * len(allow)
    strict, counts, listed = [], Counter(), []
    for check, rel, n, line, what in findings():
        if check in ("site-link", "dead-path"):
            hit = next((i for i, (f, piece, _) in enumerate(allow) if f == rel and piece in line), None)
            if hit is not None:
                used[hit] = True
                continue
            strict.append((check, rel, n, what))
        else:
            counts[(check, rel)] += 1
        listed.append((check, rel, n, what))

    if args.list:
        for check, rel, n, what in listed:
            if args.list in ("all", check):
                print(f"{rel}:{n}: [{check}] {what}")
        total = Counter(c for c, *_ in listed)
        print(", ".join(f"{c}: {total[c]}" for c in ("site-link", "dead-path", *BASELINED)), file=sys.stderr)
        return 0

    if args.update_baseline:
        rows = sorted((c, k, f) for (c, f), k in counts.items())
        BASELINE.write_text(BASELINE_HEADER + "".join(f"{c} {k} {f}\n" for c, k, f in rows), encoding="utf-8")
        print(f"wrote {BASELINE.relative_to(ROOT)}: " + ", ".join(
            f"{c} {sum(k for (cc, _), k in counts.items() if cc == c)}" for c in BASELINED))
        return 0

    failed = False
    for check, rel, n, what in strict:
        print(f"{rel}:{n}: [{check}] {what}")
        failed = True
    for (f, piece, _), u in zip(allow, used):
        if not u:
            print(f"{ALLOWLIST.relative_to(ROOT)}: matches nothing, remove it: {f} | {piece}")
            failed = True

    base = load_baseline()
    for key in sorted(set(counts) | set(base)):
        now, was = counts.get(key, 0), base.get(key, 0)
        if now > was:
            check, rel = key
            print(f"{rel}: [{check}] {now} findings, the baseline allows {was}; "
                  f"see them with --list {check}. Write the rule out or link the page that states it.")
            failed = True
    lower = [k for k in base if counts.get(k, 0) < base[k]]
    if lower and not failed:
        print(f"{len(lower)} file(s) are below the baseline; run --update-baseline to lock that in.", file=sys.stderr)

    if failed:
        print("\nHow to fix: CODING_STANDARDS.md, \"Documentation in code\".")
        return 1
    print("code references: ok", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
