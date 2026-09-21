#!/usr/bin/env bash
#
# Tests for resolve-paired-ref.sh. No network: SDK_REPO points at a local repo.
#
#   bash scripts/tests/resolve-paired-ref.test.sh

set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd -P)
RESOLVE="$HERE/../resolve-paired-ref.sh"
PASS=0
FAIL=0
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

git init -q "$ROOT/paired"
git -C "$ROOT/paired" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
git -C "$ROOT/paired" branch feat/paired

# check <label> <want-exit> <want-stdout> <key> <body> <head-branch>
check() {
  local label="$1" want="$2" expect="$3" out got
  out=$(REF_KEY="$4" PR_BODY="$5" HEAD_BRANCH="$6" SDK_REPO="$ROOT/paired" bash "$RESOLVE" main 2>/dev/null)
  got=$?
  if [ "$got" -eq "$want" ] && [ "$out" = "$expect" ]; then
    PASS=$((PASS + 1)); printf '  ok    %s\n' "$label"
  else
    FAIL=$((FAIL + 1)); printf '  FAIL  %s\n     exit %s, stdout "%s"\n' "$label" "$got" "$out"
  fi
}

check "sdk-ref is the default key"          0 "fix/x"       ""             "sdk-ref: fix/x"            ""
check "a custom key is read"                0 "fix/y"       "devtools-ref" "devtools-ref: fix/y"       ""
check "another key's line is ignored"       0 "main"        "devtools-ref" "sdk-ref: fix/x"            ""
check "same-named branch is the fallback"   0 "feat/paired" "devtools-ref" "no ref here"               "feat/paired"
check "the body wins over the branch"       0 "fix/y"       "devtools-ref" "devtools-ref: fix/y"       "feat/paired"
check "no match resolves to the default"    0 "main"        "devtools-ref" ""                          "feat/absent"
check "an unsafe ref is refused"            1 ""            "devtools-ref" "devtools-ref: --upload-pack=x" ""

printf '\n%s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
