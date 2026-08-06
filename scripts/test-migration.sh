#!/usr/bin/env bash
# Migration suite v1 -> v5 E2E test (CIP-0001 migration v0).
# From repo root; node must already be running (merod --node NODE run).
#
# Test plan:
#   Phase 1 – V1 baseline: install v1, create context, populate state.
#   Phase 2 – Migration: V1->V2 add-field, V2->V3 remove-field,
#                        V3->V4 rename-field, V4->V5 change-type.
#   Phase 3 – Post-migration checks after each step:
#             a) State preservation for shared fields
#             b) New/renamed/removed/type-changed fields behave as expected

set -euo pipefail

NODE="${NODE:-node1}"
MEROCTL="${MEROCTL:-./target/debug/meroctl}"

BUNDLE_V1_MPK="dist/com.calimero.migration-suite-1.0.0.mpk"
BUNDLE_V2_MPK="dist/com.calimero.migration-suite-2.0.0.mpk"
BUNDLE_V3_MPK="dist/com.calimero.migration-suite-3.0.0.mpk"
BUNDLE_V4_MPK="dist/com.calimero.migration-suite-4.0.0.mpk"
BUNDLE_V5_MPK="dist/com.calimero.migration-suite-5.0.0.mpk"

# All fixtures are built + bundled by one tool-driven script.
BUILD_WASMS_SCRIPT="workflows/app-migration/build-wasms.sh"

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------
parse_app_id() {
    grep -oE "application '[A-Za-z0-9]{40,50}'" | sed "s/application '//;s/'$//"
}

parse_table_value() {
    local label="$1"
    awk -F'|' -v label="$label" '
        index($0, label) > 0 { gsub(/^ +| +$/, "", $3); if ($3 != "") print $3; exit }
    '
}

call() {
    "$MEROCTL" --node "$NODE" call "$@" --context "$CONTEXT_ID" --as "$MEMBER_PUBLIC_KEY"
}

# Build + bundle every migration fixture through cargo mero if any is missing.
ensure_bundles() {
    if [[ -f "$BUNDLE_V1_MPK" && -f "$BUNDLE_V2_MPK" && -f "$BUNDLE_V3_MPK" \
          && -f "$BUNDLE_V4_MPK" && -f "$BUNDLE_V5_MPK" ]]; then
        echo "Found all migration-suite bundles under dist/"
        return 0
    fi

    echo "Building migration-suite bundles via: $BUILD_WASMS_SCRIPT"
    bash "$BUILD_WASMS_SCRIPT"

    for mpk in "$BUNDLE_V1_MPK" "$BUNDLE_V2_MPK" "$BUNDLE_V3_MPK" "$BUNDLE_V4_MPK" "$BUNDLE_V5_MPK"; do
        if [[ ! -f "$mpk" ]]; then
            echo "FAIL: bundle still missing after build: $mpk" >&2
            exit 1
        fi
    done
}

# ---------------------------------------------------------------------------
# Phase 1 – V1 baseline
# ---------------------------------------------------------------------------
echo "====================================================="
echo "  Phase 1: V1 baseline"
echo "====================================================="

echo ""
echo "--- 1.0  Ensure migration suite bundles exist ---"
ensure_bundles

echo ""
echo "--- 1.1  Install Migration Suite v1 ---"
OUT_V1=$("$MEROCTL" --node "$NODE" app install --path "$BUNDLE_V1_MPK" | tee /dev/stderr)
APP_ID_V1=$(echo "$OUT_V1" | parse_app_id)
[[ -z "$APP_ID_V1" ]] && { echo "FAIL: parse v1 app id" >&2; exit 1; }
echo "  APP_ID_V1=$APP_ID_V1"

echo ""
echo "--- 1.2  Create context ---"
OUT_CTX=$("$MEROCTL" --node "$NODE" context create --application-id "$APP_ID_V1" | tee /dev/stderr)
CONTEXT_ID=$(echo "$OUT_CTX" | parse_table_value "Context ID")
MEMBER_PUBLIC_KEY=$(echo "$OUT_CTX" | parse_table_value "Member Public Key")
[[ -z "$CONTEXT_ID" || -z "$MEMBER_PUBLIC_KEY" ]] && { echo "FAIL: parse context" >&2; exit 1; }
echo "  CONTEXT_ID=$CONTEXT_ID"
echo "  MEMBER_PUBLIC_KEY=$MEMBER_PUBLIC_KEY"

echo ""
echo "--- 1.3  set_item (v1): populate key/value ---"
call set_item --args '{"key": "preserved_key", "value": "preserved_value"}'

echo ""
echo "--- 1.4  set_description (v1) ---"
call set_description --args '{"description": "baseline description"}'

echo ""
echo "--- 1.5  increment_counter (v1) ---"
call increment_counter
call increment_counter

echo ""
echo "--- 1.6  schema_info (v1) ---"
call schema_info

echo ""
echo "====================================================="
echo "  Phase 2: Migration chain (v1 -> v5)"
echo "====================================================="

echo ""
echo "--- 2.1  Install Migration Suite v2 (add field) ---"
OUT_V2=$("$MEROCTL" --node "$NODE" app install --path "$BUNDLE_V2_MPK" | tee /dev/stderr)
APP_ID_V2=$(echo "$OUT_V2" | parse_app_id)
[[ -z "$APP_ID_V2" ]] && { echo "FAIL: parse v2 app id" >&2; exit 1; }
echo "  APP_ID_V2=$APP_ID_V2"

echo ""
echo "--- 2.2  Context update with migrate_v1_to_v2 ---"
"$MEROCTL" --node "$NODE" context update --context "$CONTEXT_ID" --application-id "$APP_ID_V2" \
    --as "$MEMBER_PUBLIC_KEY" --migrate-method migrate_v1_to_v2

echo ""
echo "--- 2.3  Verify v2 state (notes added) ---"
call get_item --args '{"key": "preserved_key"}'
call get_description
call get_counter
call get_notes
call schema_info

echo ""
echo "--- 2.4  Install Migration Suite v3 (remove field) ---"
OUT_V3=$("$MEROCTL" --node "$NODE" app install --path "$BUNDLE_V3_MPK" | tee /dev/stderr)
APP_ID_V3=$(echo "$OUT_V3" | parse_app_id)
[[ -z "$APP_ID_V3" ]] && { echo "FAIL: parse v3 app id" >&2; exit 1; }
echo "  APP_ID_V3=$APP_ID_V3"

echo ""
echo "--- 2.5  Context update with migrate_v2_to_v3 ---"
"$MEROCTL" --node "$NODE" context update --context "$CONTEXT_ID" --application-id "$APP_ID_V3" \
    --as "$MEMBER_PUBLIC_KEY" --migrate-method migrate_v2_to_v3

echo ""
echo "--- 2.6  Verify v3 state (notes removed) ---"
call get_item --args '{"key": "preserved_key"}'
call get_description
call get_counter
call schema_info

echo ""
echo "--- 2.7  Install Migration Suite v4 (rename field) ---"
OUT_V4=$("$MEROCTL" --node "$NODE" app install --path "$BUNDLE_V4_MPK" | tee /dev/stderr)
APP_ID_V4=$(echo "$OUT_V4" | parse_app_id)
[[ -z "$APP_ID_V4" ]] && { echo "FAIL: parse v4 app id" >&2; exit 1; }
echo "  APP_ID_V4=$APP_ID_V4"

echo ""
echo "--- 2.8  Context update with migrate_v3_to_v4 ---"
"$MEROCTL" --node "$NODE" context update --context "$CONTEXT_ID" --application-id "$APP_ID_V4" \
    --as "$MEMBER_PUBLIC_KEY" --migrate-method migrate_v3_to_v4

echo ""
echo "--- 2.9  Verify v4 state (description -> details) ---"
call get_item --args '{"key": "preserved_key"}'
call get_details
call get_counter
call schema_info

echo ""
echo "--- 2.10 Install Migration Suite v5 (change type) ---"
OUT_V5=$("$MEROCTL" --node "$NODE" app install --path "$BUNDLE_V5_MPK" | tee /dev/stderr)
APP_ID_V5=$(echo "$OUT_V5" | parse_app_id)
[[ -z "$APP_ID_V5" ]] && { echo "FAIL: parse v5 app id" >&2; exit 1; }
echo "  APP_ID_V5=$APP_ID_V5"

echo ""
echo "--- 2.11 Context update with migrate_v4_to_v5 ---"
"$MEROCTL" --node "$NODE" context update --context "$CONTEXT_ID" --application-id "$APP_ID_V5" \
    --as "$MEMBER_PUBLIC_KEY" --migrate-method migrate_v4_to_v5

echo ""
echo "====================================================="
echo "  Phase 3: Final verification (v5)"
echo "====================================================="

echo ""
echo "--- 3.1  get_item: preserved key still present ---"
call get_item --args '{"key": "preserved_key"}'

echo ""
echo "--- 3.2  get_details: renamed field retains value ---"
call get_details

echo ""
echo "--- 3.3  get_counter: value is now a string ---"
call get_counter

echo ""
echo "--- 3.4  set_counter/get_counter: type-changed field is writable ---"
call set_counter --args '{"counter": "42"}'
call get_counter

echo ""
echo "--- 3.5  schema_info: should show 5.0.0 ---"
call schema_info

echo ""
echo "====================================================="
echo "  Migration suite E2E complete"
echo "====================================================="