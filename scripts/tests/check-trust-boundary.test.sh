#!/usr/bin/env bash
#
# Tests for check-trust-boundary.py, using the real PR template as the body source.
#
#   bash scripts/tests/check-trust-boundary.test.sh

set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd -P)
ROOT=$(cd "$HERE/../.." && pwd -P)
CHECK="$ROOT/scripts/check-trust-boundary.py"
TEMPLATE=$(cat "$ROOT/.github/pull_request_template.md")
ANSWERED=$(printf '%s\n' "$TEMPLATE" | sed 's/: \[ \] yes \[ \] no$/: [ ] yes [x] no/')
IDENTITY='- Takes an identity from a message field instead of a signature or the authenticated channel'
SENSITIVE="crates/node/src/sync/manager/mod.rs"
NBSP=$(printf '\302\240')
ZWSP=$(printf '\342\200\213')
PASS=0
FAIL=0

# check <label> <want-exit> <changed-paths> <body> [<substring the output must contain>]
check() {
  local label="$1" want="$2" expect="${5:-}" out got
  out=$(printf '%s\n' "$3" | PR_BODY="$4" python3 "$CHECK" 2>&1)
  got=$?
  if [ "$got" -eq "$want" ] && [[ "$out" == *"$expect"* ]]; then
    PASS=$((PASS + 1)); printf '  ok    %s\n' "$label"
  else
    FAIL=$((FAIL + 1)); printf '  FAIL  %s\n     exit %s, want %s; output:\n%s\n' "$label" "$got" "$want" "$out"
  fi
}

# with_identity_line <replacement>: the answered body with the identity item's line replaced
with_identity_line() {
  printf '%s\n' "$ANSWERED" | NEW="$1" awk -v id="$IDENTITY" 'index($0, id) == 1 { print ENVIRON["NEW"]; next } { print }'
}

[ "$ANSWERED" != "$TEMPLATE" ] || { echo "  FAIL  could not answer the template"; exit 1; }

check "untouched paths need no block"            0 "crates/storage/src/lib.rs"   ""
check "an answered block passes"                  0 "$SENSITIVE"                  "$ANSWERED"
check "a CRLF body passes"                        0 "$SENSITIVE"                  "$(printf '%s\n' "$ANSWERED" | awk '{ printf "%s\r\n", $0 }')"
check "a trailing note after the boxes passes"    0 "$SENSITIVE"                  "$(with_identity_line "$IDENTITY: [ ] yes [x] no - see the handshake proof")"
check "a closed <details> elsewhere passes"       0 "$SENSITIVE"                  "<details>log</details>"$'\n'"$ANSWERED"
check "a tag in inline code hides nothing"       0 "$SENSITIVE"                  'Uses `<details>` and ``<!--``.'$'\n'"$ANSWERED"
check "a literal <!-- in a fence hides nothing"   0 "$SENSITIVE"                  '```'$'\n''<!--'$'\n''```'$'\n'"$ANSWERED"
check "a workflow change is covered"              1 ".github/workflows/x.yml"     ""                 "missing:"
check "a sibling module file is covered"          1 "crates/runtime/src/logic/host_functions.rs" "" "missing:"
check "a prefix-sharing path is not covered"      0 "crates/nodex/src/lib.rs"     ""
check "a missing block fails"                     1 "$SENSITIVE"                  "## Description"   "missing:"
check "the unanswered template fails"             1 "$SENSITIVE"                  "$TEMPLATE"        "tick exactly one"
check "both boxes ticked fails"                   1 "$SENSITIVE"                  "$(with_identity_line "$IDENTITY: [x] yes [x] no")" "tick exactly one"
check "a dropped item fails"                      1 "$SENSITIVE"                  "$(with_identity_line "")" "missing: Takes an identity"
check "an item listed twice fails"                1 "$SENSITIVE"                  "$(with_identity_line "$IDENTITY: [ ] yes [x] no"$'\n'"$IDENTITY: [x] yes [ ] no")" "more than once"
check "an extra item-shaped line fails"           1 "$SENSITIVE"                  "$(with_identity_line "$IDENTITY: [ ] yes [x] no"$'\n'"- Something else: [ ] yes [x] no")" "not a template item"
check "a malformed answer line fails"             1 "$SENSITIVE"                  "$(with_identity_line "$IDENTITY: [ ] yes")" "shape"
check "a non-breaking space fails"                1 "$SENSITIVE"                  "$(with_identity_line "-$NBSP${IDENTITY#- }: [ ] yes [x] no")" "non-breaking"
check "a zero-width space fails"                  1 "$SENSITIVE"                  "$(with_identity_line "$IDENTITY: [ ] yes [x$ZWSP] no")" "non-breaking"
check "items under another heading fail"          1 "$SENSITIVE"                  "$(printf '%s\n' "$ANSWERED" | sed 's/^## Trust boundary$/## Notes/')" "missing:"
check "a block in an HTML comment fails"          1 "$SENSITIVE"                  "<!--"$'\n'"$ANSWERED"$'\n'"-->" "missing:"
check "a block in a code fence fails"             1 "$SENSITIVE"                  '~~~'$'\n'"$ANSWERED"$'\n''~~~' "missing:"
check "a fence closed by the other char fails"    1 "$SENSITIVE"                  '~~~'$'\n''```'$'\n'"$ANSWERED" "missing:"
check "a long fence closed by a short one fails"  1 "$SENSITIVE"                  '````'$'\n''```'$'\n'"$ANSWERED" "missing:"
check "an indented code block fails"              1 "$SENSITIVE"                  "$(printf '%s\n' "$ANSWERED" | sed 's/^- /    - /')" "missing:"
check "a block in <details> fails"                1 "$SENSITIVE"                  "<details>"$'\n'"$ANSWERED"$'\n'"</details>" "missing:"
check "nested <details> stay hidden"              1 "$SENSITIVE"                  "<details><details></details>"$'\n'"$ANSWERED"$'\n'"</details>" "missing:"
check "a block in <script> fails"                 1 "$SENSITIVE"                  "<SCRIPT type=x>"$'\n'"$ANSWERED" "missing:"

# Every crate or workflow file the AGENTS.md section cites as an example is itself a trust-boundary path.
SECTION=$(sed -n '/^## Security: trust boundaries/,/^## [^S]/p' "$ROOT/AGENTS.md")
LINKED=$(printf '%s\n' "$SECTION" | grep -oE '\]\((crates|\.github)/[^)#]+' | sed 's/^](//' | sort -u)
[ -n "$LINKED" ] || { echo "  FAIL  found no crate links in the AGENTS.md section"; FAIL=$((FAIL + 1)); }
for path in $LINKED; do
  check "AGENTS.md example is covered: $path" 1 "$path" "" "missing:"
done

printf '\n%s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
