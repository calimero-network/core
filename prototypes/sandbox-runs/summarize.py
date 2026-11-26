"""Print the escape suite's verdicts from a host run (stdin: the host's JSON output)."""
import json, sys
raw = sys.stdin.read()
d = json.loads(raw[raw.index("{"):])
d = d.get("escape", d)["result"]
print(f"{d['passed']}/{d['total']} passed")
for c in d["cases"]:
    print(("PASS " if c["pass"] else "FAIL ") + c["name"])
    if not c["pass"] and "-v" in sys.argv:
        print("    ", json.dumps(c["evidence"])[:700])
