#!/usr/bin/env bash
# The node-log gates every e2e scenario is held to, over that one scenario's logs.
#
#   e2e-scenario-log-gates.sh <logdir> <scenario>
#
# Run by scripts/e2e-run-scenario-group.sh after each scenario, whether or not
# the scenario itself passed, so a scenario's verdict names its gate. Three are
# HARD gates and fail the scenario; two only report. Every gate runs even after
# one fails. Exit 1 when any hard gate failed.
#
# The suite-wide floors (the projection gate concluded something somewhere, both
# key-serve grounds were exercised) are separate jobs in e2e-rust-apps.yml that
# read every group's uploaded logs.
set -uo pipefail

logs=${1:?usage: $0 <logdir> <scenario>}
scenario=${2:?usage: $0 <logdir> <scenario>}
failed=0

strip() { sed -E 's/\x1b\[[0-9;]*m//g'; }

# Assert no unified-op projection divergence (cutover gate).
projection_divergence() {
  # unified_projection_divergence marks the unified-op projection disagreeing with the live
  # decision. HARD gate; silence per-scenario isn't proof, so projection-coverage checks the total.
  local compares concluded refused
  compares=$(grep -rh "unified_projection_compare" "$logs" 2>/dev/null | strip || true)
  echo "membership comparisons:"
  if [ -n "${compares}" ]; then
      echo "${compares}" | grep -oE 'result="[a-z_]+"' | sort | uniq -c
      # By op kind too: a gate that only ever concludes on one plane is thinner than
      # its total suggests.
      echo "${compares}" | grep -oE 'op_kind="[a-z]+"' | sort | uniq -c
  else
      echo "  (none — this scenario folded no membership op)"
  fi
  concluded=$(echo "${compares}" | grep -cE 'result="(agree|diverged)"' || true)
  refused=$(echo "${compares}" | grep -cE 'result="skipped_[a-z_]+"' || true)

  if grep -rl "unified_projection_divergence" "$logs" 2>/dev/null | grep -q .; then
      echo "::error::unified-op projection diverged from the live decision in ${scenario} — cutover gate not met"
      echo "planes seen:"
      grep -rh "unified_projection_divergence" "$logs" \
        | strip | grep -o 'plane="[^"]*"' | sort | uniq -c
      echo "sample:"
      grep -rn "unified_projection_divergence" "$logs" | strip | head -30
      return 1
  fi

  # Observe-only per scenario: a single unresolved membership op is thin coverage, not a broken
  # gate. The systemic failure - the gate concluding nothing anywhere - is asserted suite-wide.
  if [ "${refused}" -gt 0 ] && [ "${concluded}" -eq 0 ]; then
      echo "::notice::every membership comparison in ${scenario} was refused (${refused}) — thin coverage here; the suite-wide assertion is what gates this"
  fi
  echo "no unified_projection_divergence markers across ${concluded} concluded comparison(s) (${refused} refused) — projection agrees with the live decision"
}

# Assert unified op-store completeness (C3 Stage 4, HARD gate).
op_store_completeness() {
  # op_store_incomplete marks the unified op-store missing a governance op the gov-DAG has. HARD
  # gate: the read now flips onto the op-store, so any marker is a correctness bug.
  local count gc
  if grep -rl "op_store_incomplete" "$logs" 2>/dev/null | grep -q .; then
      count=$(grep -rho "op_store_incomplete" "$logs" | wc -l | tr -d ' ')
      echo "::error::op-store incomplete in ${scenario} — $count marker(s); a governance apply path is not writing the op-store (read-flip is unsafe)"
      echo "sample (missing_count per namespace):"
      # ANSI escapes sit between the field name and '=' in node logs, so they're stripped first;
      # the match tolerates any field ordering and the field being absent.
      grep -rho 'op_store_incomplete.*' "$logs" \
        | sed -E 's/\x1b\[[0-9;]*m//g' \
        | grep -oE 'missing_count=[0-9]+' | sort | uniq -c | head -10
      grep -rn "op_store_incomplete" "$logs" | head -10
      return 1
  elif grep -rl "op_store_gate_unavailable" "$logs" 2>/dev/null | grep -q .; then
      # No gap markers, but the gate couldn't load the op-store somewhere — do
      # NOT claim a clean mirror, that would be a false "verified" line.
      gc=$(grep -rho "op_store_gate_unavailable" "$logs" | wc -l | tr -d ' ')
      echo "::notice::op-store completeness UNVERIFIED in ${scenario} — no gaps seen but $gc gate-load failure(s); mirror NOT confirmed"
      grep -rn "op_store_gate_unavailable" "$logs" | head -5
  else
      echo "no op_store_incomplete or op_store_gate_unavailable markers — op-store mirrors the gov-DAG for every exercised namespace"
  fi
}

# Assert no blob chunk was served mid-write (HARD gate).
blob_chunk_torn_read() {
  # "blob chunk hash mismatch; refusing to serve" means a torn read, since no scenario tampers
  # with blob bytes. Fixed via temp-file + rename (atomic); a marker means that regressed.
  local count
  if grep -rl "blob chunk hash mismatch" "$logs" 2>/dev/null | grep -q .; then
      count=$(grep -rho "blob chunk hash mismatch" "$logs" | wc -l | tr -d ' ')
      echo "::error::blob chunk served mid-write in ${scenario} — $count occurrence(s); a chunk failed its content-hash check on the serve path"
      echo "sample:"
      grep -rn "blob chunk hash mismatch" "$logs" | head -10
      return 1
  fi
  echo "no blob chunk hash mismatches — every chunk served passed its content-hash check"
}

# Report which ground authorized each group-key recovery (observe-only).
key_recovery_grounds() {
  # key_server_accepted allows exactly two grounds: a trusted anchor, or a certificate proving a
  # device of this node's own account. Informational only; key-recovery-coverage is the floor.
  if grep -rl "key_recovery_authorized_by" "$logs" 2>/dev/null | grep -q .; then
      echo "group-key recoveries by authorizing ground:"
      grep -rho 'key_recovery_authorized_by.*' "$logs" \
        | sed -E $'s/\x1B\\[[0-9;]*[mK]//g' \
        | grep -oE 'key_recovery_authorized_by="?[a-z_]+"?' | sort | uniq -c | sort -rn
  else
      # Not a failure: most scenarios never need a direct pull; delivery reaches
      # them first.
      echo "no key_recovery_authorized_by markers - this scenario recovered no group key by direct pull"
  fi
}

# Report scope_root governance-divergence pulls (P6.S2, observe-only).
gov_divergence_pulls() {
  # gov_divergence_pull_triggered fires when entities agree but scope_root differs, triggering a
  # pull from the diverging peer. Informational: also watches for a storm (one context dominating).
  local count per_context outcomes
  if grep -rl "gov_divergence_pull_triggered" "$logs" 2>/dev/null | grep -q .; then
      count=$(grep -rho "gov_divergence_pull_triggered" "$logs" | wc -l | tr -d ' ')
      echo "::notice::scope_root governance pulls in ${scenario} — $count trigger(s)"
      echo "per-context trigger counts (watch for a single context dominating = storm):"
      # tracing writes these logs with ANSI styling, so the '=' isn't adjacent to the field name -
      # hence the sed. The value is bs58 (ContextId's Display), not hex.
      per_context=$(grep -rho 'gov_divergence_pull_triggered.*' "$logs" \
        | sed -E $'s/\x1B\\[[0-9;]*[mK]//g' \
        | grep -oE 'context_id=[A-Za-z0-9]+' | sort | uniq -c | sort -rn | head -10)
      if [ -n "$per_context" ]; then
          echo "$per_context"
      else
          # Never silently print nothing again: an empty breakdown beside a
          # non-zero count means the field moved or stopped being emitted.
          echo "::warning::$count trigger(s) but no context_id parsed from them — the marker's fields changed shape, so this breakdown is blind"
          grep -rho 'gov_divergence_pull_triggered.*' "$logs" | head -2
      fi
      # ops_pulled == 0 throughout means the peer had nothing to give; a non-zero value repeating
      # while the scenario still fails means the fold doesn't converge on ops that do arrive.
      echo "pull outcomes (ops_pulled), which splits 'nothing to give' from 'ops do not take':"
      outcomes=$(grep -rho 'gov_divergence_pull_complete.*' "$logs" \
        | sed -E $'s/\x1B\\[[0-9;]*[mK]//g' \
        | grep -oE 'ops_pulled=[0-9]+' | sort | uniq -c | sort -rn | head -5)
      if [ -n "$outcomes" ]; then
          echo "$outcomes"
      else
          echo "::warning::triggers present but no gov_divergence_pull_complete found — the pull did not return, or the marker moved"
      fi
  else
      echo "no gov_divergence_pull_triggered markers — no scope_root governance divergence pulled this run"
  fi
}

echo "-- gate: projection divergence"
projection_divergence || failed=1
echo "-- gate: op-store completeness"
op_store_completeness || failed=1
echo "-- gate: blob chunk served mid-write"
blob_chunk_torn_read || failed=1
echo "-- report: key-recovery grounds"
key_recovery_grounds || true
echo "-- report: governance-divergence pulls"
gov_divergence_pulls || true

exit "$failed"
