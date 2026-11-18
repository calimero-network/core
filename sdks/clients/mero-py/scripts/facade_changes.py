#!/usr/bin/env python3
"""Compare two versions of calimero_client_py/facade.py: the facade methods
added, removed and with a changed signature, as Markdown.

    python scripts/facade_changes.py OLD_FACADE NEW_FACADE

.github/workflows/openapi-sync.yml puts this in the pull request it opens,
comparing the checked-in facade with the one regenerated from core's new
document. Exits 0 whatever it finds; prints nothing about a part with no
change, and "No facade method changed." when nothing did.
"""

import ast
import sys
from pathlib import Path


def methods(source: str) -> dict:
    """`client.<group>.<method>` -> its parameters, for a facade.py's text."""
    tree = ast.parse(source)
    classes = {n.name: n for n in tree.body if isinstance(n, ast.ClassDef)}
    groups = {}
    for node in tree.body:
        target = getattr(node, "targets", [None])[0]
        if isinstance(target, ast.Name) and target.id == "GROUPS":
            for key, value in zip(node.value.keys, node.value.values):
                groups[key.value] = value.id
    found = {}
    for group, cls in groups.items():
        for item in classes[cls].body:
            if isinstance(item, ast.FunctionDef) and not item.name.startswith("_"):
                args = item.args
                args.args = [a for a in args.args if a.arg != "self"]
                found[f"client.{group}.{item.name}"] = ast.unparse(args)
    return found


def report(old: dict, new: dict) -> str:
    added = sorted(set(new) - set(old))
    removed = sorted(set(old) - set(new))
    changed = sorted(k for k in set(old) & set(new) if old[k] != new[k])
    lines = []
    if added:
        lines += ["**Added**", ""]
        lines += [f"- `{k}({new[k]})`" for k in added] + [""]
    if removed:
        lines += ["**Removed**", ""]
        lines += [f"- `{k}({old[k]})`" for k in removed] + [""]
    if changed:
        lines += ["**Signature changed**", ""]
        for k in changed:
            lines += [f"- `{k}`", f"  - was `({old[k]})`", f"  - now `({new[k]})`"]
        lines.append("")
    return "\n".join(lines) if lines else "No facade method changed.\n"


def main(argv) -> int:
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    old, new = (methods(Path(p).read_text()) for p in argv)
    sys.stdout.write(report(old, new))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
