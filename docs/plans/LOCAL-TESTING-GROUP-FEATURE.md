# Local Testing — Group Feature (meroctl commands)

All commands from `~/Developer/Calimero/core`.

## Phase 1 — Install App & Create Group

```bash
APP_ID=$(./target/release/meroctl --node node-a app install \
  --path apps/migrations/migration-suite-v1/res/migration-suite-1.0.0.mpk)
echo "APP_ID=$APP_ID"

GROUP_ID=$(./target/release/meroctl --node node-a group create --application-id "$APP_ID")
echo "GROUP_ID=$GROUP_ID"

./target/release/meroctl --node node-a group get "$GROUP_ID"
```

## Phase 2 — Set Default Capabilities & Visibility

```bash
./target/release/meroctl --node node-a group settings set-default-capabilities "$GROUP_ID" \
  --can-join-open-contexts --can-create-context

./target/release/meroctl --node node-a group settings set-default-visibility "$GROUP_ID" \
  --mode open
```

## Phase 3 — Invite Node B

```bash
JOINER_PK=$(./target/release/meroctl --node node-b node identity | sed 's/^ed25519://')
echo "JOINER_PK=$JOINER_PK"

INVITE=$(./target/release/meroctl --node node-a group invite "$GROUP_ID")
echo "INVITE=$INVITE"

./target/release/meroctl --node node-b group join "$INVITE"

./target/release/meroctl --node node-a group sync "$GROUP_ID"

./target/release/meroctl --node node-a group members list "$GROUP_ID"
./target/release/meroctl --node node-b group members list "$GROUP_ID"
```

## Phase 4 — Check & Modify Member Capabilities

```bash
./target/release/meroctl --node node-a group members get-capabilities "$GROUP_ID" "$JOINER_PK"

./target/release/meroctl --node node-a group members set-capabilities "$GROUP_ID" "$JOINER_PK" \
  --can-join-open-contexts --can-create-context --can-invite-members

./target/release/meroctl --node node-a group members get-capabilities "$GROUP_ID" "$JOINER_PK"
```

## Phase 5 — Create Context in Group (Admin)

```bash
CONTEXT_A=$(./target/release/meroctl --node node-a context create \
  --protocol near --application-id "$APP_ID" --group-id "$GROUP_ID")
echo "CONTEXT_A=$CONTEXT_A"

./target/release/meroctl --node node-a group contexts list "$GROUP_ID"

# Sync first to populate local visibility store
./target/release/meroctl --node node-a group sync "$GROUP_ID"
./target/release/meroctl --node node-a group contexts get-visibility "$GROUP_ID" "$CONTEXT_A"
```

## Phase 6 — Node B Joins Context via Group

```bash
# Join FIRST (establishes P2P mesh), then sync
./target/release/meroctl --node node-b group join-group-context "$GROUP_ID" \
  --context-id "$CONTEXT_A"

./target/release/meroctl --node node-b group sync "$GROUP_ID"

CTX_ID_A=$(./target/release/meroctl --node node-a context identity list --context "$CONTEXT_A" | head -1)
echo "CTX_ID_A=$CTX_ID_A"
./target/release/meroctl --node node-a call schema_info --context "$CONTEXT_A" --as "$CTX_ID_A" --args '{}'

CTX_ID_B=$(./target/release/meroctl --node node-b context identity list --context "$CONTEXT_A" | head -1)
echo "CTX_ID_B=$CTX_ID_B"
./target/release/meroctl --node node-b call schema_info --context "$CONTEXT_A" --as "$CTX_ID_B" --args '{}'
```

## Phase 7 — Restricted Context + Allowlist

```bash
CONTEXT_R=$(./target/release/meroctl --node node-a context create \
  --protocol near --application-id "$APP_ID" --group-id "$GROUP_ID")
echo "CONTEXT_R=$CONTEXT_R"

./target/release/meroctl --node node-a group contexts set-visibility "$GROUP_ID" "$CONTEXT_R" \
  --mode restricted

./target/release/meroctl --node node-a group sync "$GROUP_ID"
./target/release/meroctl --node node-a group contexts get-visibility "$GROUP_ID" "$CONTEXT_R"

./target/release/meroctl --node node-a group contexts allowlist list "$GROUP_ID" "$CONTEXT_R"

# Should FAIL — not on allowlist
./target/release/meroctl --node node-b group join-group-context "$GROUP_ID" \
  --context-id "$CONTEXT_R"

# Add Node B to allowlist
./target/release/meroctl --node node-a group contexts allowlist add \
  "$GROUP_ID" "$CONTEXT_R" "$JOINER_PK"

./target/release/meroctl --node node-a group contexts allowlist list "$GROUP_ID" "$CONTEXT_R"

# Should SUCCEED now
./target/release/meroctl --node node-b group join-group-context "$GROUP_ID" \
  --context-id "$CONTEXT_R"

./target/release/meroctl --node node-b context identity list --context "$CONTEXT_R"
```

## Phase 8 — Group Upgrade with Migration (v1 → v2)

```bash
APP_V2_ID=$(./target/release/meroctl --node node-a app install \
  --path apps/migrations/migration-suite-v2-add-field/res/migration-suite-2.0.0.mpk)
echo "APP_V2_ID=$APP_V2_ID"

./target/release/meroctl --node node-a group upgrade trigger "$GROUP_ID" \
  --target-application-id "$APP_V2_ID" --migrate-method migrate_v1_to_v2

./target/release/meroctl --node node-a group upgrade status "$GROUP_ID"

./target/release/meroctl --node node-a call schema_info --context "$CONTEXT_A" --as "$CTX_ID_A" --args '{}'

# Sync Node B + verify propagation
./target/release/meroctl --node node-b group sync "$GROUP_ID"
sleep 5
./target/release/meroctl --node node-b call schema_info --context "$CONTEXT_A" --as "$CTX_ID_B" --args '{}'
```

## Phase 9 — Cascade Group Member Removal

```bash
./target/release/meroctl --node node-b context identity list --context "$CONTEXT_A"
./target/release/meroctl --node node-b context identity list --context "$CONTEXT_R"

./target/release/meroctl --node node-a group members remove "$GROUP_ID" "$JOINER_PK"

./target/release/meroctl --node node-a group members list "$GROUP_ID"

# Both should only show Node A's identities
./target/release/meroctl --node node-a context identity list --context "$CONTEXT_A"
./target/release/meroctl --node node-a context identity list --context "$CONTEXT_R"

# Should get Unauthorized
./target/release/meroctl --node node-b call schema_info --context "$CONTEXT_A" --as "$CTX_ID_B" --args '{}'
```

## Phase 10 — Capability Lockdown Test

```bash
./target/release/meroctl --node node-a group members add "$GROUP_ID" "$JOINER_PK"

./target/release/meroctl --node node-a group members get-capabilities "$GROUP_ID" "$JOINER_PK"

# Strip all capabilities
./target/release/meroctl --node node-a group members set-capabilities "$GROUP_ID" "$JOINER_PK"

./target/release/meroctl --node node-a group members get-capabilities "$GROUP_ID" "$JOINER_PK"

# Should FAIL — no CAN_JOIN_OPEN_CONTEXTS
./target/release/meroctl --node node-b group sync "$GROUP_ID"
./target/release/meroctl --node node-b group join-group-context "$GROUP_ID" \
  --context-id "$CONTEXT_A"

# Restore capabilities
./target/release/meroctl --node node-a group members set-capabilities "$GROUP_ID" "$JOINER_PK" \
  --can-join-open-contexts --can-create-context --can-invite-members

# Should SUCCEED
./target/release/meroctl --node node-b group join-group-context "$GROUP_ID" \
  --context-id "$CONTEXT_A"
```
