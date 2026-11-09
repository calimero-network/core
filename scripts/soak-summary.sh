#!/usr/bin/env bash
# Append a soak report (the JSON crates/node/tests/fault/suite.py's kv-soak writes
# with --soak-report, or merobox's fuzzy_test to MEROBOX_SOAK_REPORT: the same
# format) to the job summary as a table: per operation, its calls/s and the
# p50/p95/p99 latency of its calls (crates/node/tests/fault/soak.py report).
#
#   scripts/soak-summary.sh REPORT.json TITLE
#
# Writes to $GITHUB_STEP_SUMMARY, or stdout outside CI. A missing or empty
# report (no soak in the run) is said so.

set -euo pipefail

report=${1:?usage: soak-summary.sh REPORT.json TITLE}
title=${2:?usage: soak-summary.sh REPORT.json TITLE}
summary=${GITHUB_STEP_SUMMARY:-/dev/stdout}
root=$(cd "$(dirname "$0")/.." && pwd)

if [ -s "$report" ]; then
  python3 -I "$root/crates/node/tests/fault/soak.py" report "$report" --title "$title" >>"$summary"
else
  printf '### %s\n\nNo soak report: no soak ran.\n' "$title" >>"$summary"
fi
