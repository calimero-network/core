#!/usr/bin/env python3
"""The facade methods one regeneration added, removed or changed, as Markdown.

    python3 codegen/facade-diff.py <facade dir before> <facade dir after>

Reads the `*Facade.kt` files codegen/facade.py writes (one class per group, one
`suspend fun` per operation) and compares each method's parameters and answer, keyed by
`group.method` (`orgs.inviteCreate`). Prints a Markdown section for a pull request body. The
exit code is 0 when nothing a caller compiles against was removed or changed, and 3 when a
method was removed or its signature changed (a major version for the SDK).
"""

import os
import re
import sys

METHOD = re.compile(r"    suspend fun (`?\w+`?)\(([^)]*)\)(?::\s*([^=]+?))?\s*=", re.S)


def read(directory):
    methods = {}
    if not os.path.isdir(directory):
        return methods
    for name in sorted(os.listdir(directory)):
        if not name.endswith("Facade.kt"):
            continue
        group = name[: -len("Facade.kt")]
        group = group[:1].lower() + group[1:]
        text = open(os.path.join(directory, name)).read()
        for m in METHOD.finditer(text):
            params = " ".join(m.group(2).split()).rstrip(",").strip()
            answer = " ".join((m.group(3) or "Unit").split())
            methods[f"{group}.{m.group(1).strip('`')}"] = f"({params}): {answer}"
    return methods


def main():
    before, after = read(sys.argv[1]), read(sys.argv[2])
    added = sorted(set(after) - set(before))
    removed = sorted(set(before) - set(after))
    changed = sorted(k for k in set(before) & set(after) if before[k] != after[k])

    out = ["### Facade changes", ""]
    if not (added or removed or changed):
        out.append("No facade method was added, removed or changed.")
    for title, names, show in (
        ("Added", added, lambda k: f"- `{k}{after[k]}`"),
        ("Removed", removed, lambda k: f"- `{k}{before[k]}`"),
        ("Changed", changed, lambda k: f"- `{k}`\n  - was `{before[k]}`\n  - now `{after[k]}`"),
    ):
        if names:
            out += [f"**{title}** ({len(names)})", ""] + [show(k) for k in names] + [""]
    breaking = bool(removed or changed)
    if breaking:
        out += [
            "**A method was removed or its signature changed: this needs a major version of the SDK** "
            "(callers that compile against it break).",
        ]
    print("\n".join(out).rstrip() + "\n")
    sys.exit(3 if breaking else 0)


if __name__ == "__main__":
    main()
