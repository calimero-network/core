#!/usr/bin/env bash
# Append a soak report (the JSON a fuzzy_test step writes to MEROBOX_SOAK_REPORT)
# to the job summary as a table: per operation, its calls/s and the p50/p95/p99
# latency of its calls (`merobox cloud soak-report`).
#
#   scripts/soak-summary.sh REPORT.json TITLE
#
# Writes to $GITHUB_STEP_SUMMARY, or stdout outside CI. A missing or empty
# report (no fuzzy_test in the run, or a merobox that writes none) is said so.

set -euo pipefail

report=${1:?usage: soak-summary.sh REPORT.json TITLE}
title=${2:?usage: soak-summary.sh REPORT.json TITLE}
summary=${GITHUB_STEP_SUMMARY:-/dev/stdout}

if [ -s "$report" ]; then
  merobox cloud soak-report "$report" --title "$title" >>"$summary"
else
  printf '### %s\n\nNo soak report: no fuzzy_test step ran, or merobox at %s writes none.\n' \
    "$title" "${MEROBOX_REF:-its revision}" >>"$summary"
fi
