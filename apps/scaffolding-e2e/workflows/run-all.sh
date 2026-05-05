#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=()
FAIL=()

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

RUN_TS="$(date +%Y%m%d_%H%M%S)"
LOG_DIR="$SCRIPT_DIR/logs/$RUN_TS"
mkdir -p "$LOG_DIR"

# Run from the logic/ directory so relative paths in YAMLs (res/, etc.) resolve correctly
cd "$SCRIPT_DIR/.."

WORKFLOWS=($(ls "$SCRIPT_DIR"/*.yml | sort))

echo ""
echo "Running ${#WORKFLOWS[@]} workflows from $SCRIPT_DIR"
echo "Logs: $LOG_DIR"
echo "=================================================="

for wf in "${WORKFLOWS[@]}"; do
  name="$(basename "$wf" .yml)"
  log="$LOG_DIR/${name}.log"

  echo ""
  echo -e "${YELLOW}▶ ${name}.yml${NC}"
  echo "--------------------------------------------------"

  if merobox bootstrap run "$wf" 2>&1 | tee "$log"; then
    PASS+=("$name")
    echo -e "${GREEN}✓ ${name}.yml passed${NC}"
    # Keep log anyway — useful for debugging flaky passes too
  else
    FAIL+=("$name")
    echo -e "${RED}✗ ${name}.yml FAILED — log saved to:${NC}"
    echo "    $log"
  fi
done

# Write summary file
SUMMARY="$LOG_DIR/summary.txt"
{
  echo "Run: $RUN_TS"
  echo "Passed: ${#PASS[@]}  Failed: ${#FAIL[@]}"
  echo ""
  for w in "${PASS[@]}"; do echo "PASS  $w"; done
  for w in "${FAIL[@]}"; do echo "FAIL  $w"; done
} > "$SUMMARY"

echo ""
echo "=================================================="
echo "Results: ${#PASS[@]} passed, ${#FAIL[@]} failed"
echo ""

if [ ${#PASS[@]} -gt 0 ]; then
  echo -e "${GREEN}Passed:${NC}"
  for w in "${PASS[@]}"; do echo "  ✓ ${w}.yml"; done
fi

if [ ${#FAIL[@]} -gt 0 ]; then
  echo ""
  echo -e "${RED}Failed:${NC}"
  for w in "${FAIL[@]}"; do echo "  ✗ ${w}.yml  →  $LOG_DIR/${w}.log"; done
  echo ""
  echo "Summary: $SUMMARY"
  echo ""
  exit 1
fi

echo "Summary: $SUMMARY"
echo ""
