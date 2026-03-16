# E2E Testing Plan: Context Groups (Phases 1-6)

**Date**: 2026-02-25
**Status**: Proposed
**Branch**: `feat/context-management-proposal`

## Overview

End-to-end testing for the Context Groups feature using the merobox workflow framework. This requires two deliverables:

1. **New merobox step types** — Python step classes for all group admin API endpoints
2. **Workflow YAML file** — E2E test scenarios covering Phases 1-6

---

## Part 1: Merobox Group Step Types

### 1.1 New API Constants

**File**: `.merobox-src/merobox/commands/constants.py`

Add group API endpoint constants:

```python
# Group management API endpoints
ADMIN_API_GROUPS = f"{ADMIN_API_BASE}/groups"

# Workflow step type constants
STEP_CREATE_GROUP = "create_group"
STEP_DELETE_GROUP = "delete_group"
STEP_GET_GROUP_INFO = "get_group_info"
STEP_ADD_GROUP_MEMBERS = "add_group_members"
STEP_REMOVE_GROUP_MEMBERS = "remove_group_members"
STEP_LIST_GROUP_MEMBERS = "list_group_members"
STEP_LIST_GROUP_CONTEXTS = "list_group_contexts"
STEP_UPGRADE_GROUP = "upgrade_group"
STEP_GET_GROUP_UPGRADE_STATUS = "get_group_upgrade_status"
STEP_RETRY_GROUP_UPGRADE = "retry_group_upgrade"
STEP_CREATE_GROUP_INVITATION = "create_group_invitation"
STEP_JOIN_GROUP = "join_group"
```

### 1.2 Group API Helper Module

**New file**: `.merobox-src/merobox/commands/groups.py`

Since `calimero-client-py==0.2.7` does not have group methods, all group operations will use raw HTTP calls via `aiohttp` (already a merobox dependency). Each function follows the existing pattern: `@with_retry` decorator, returns `ok(data)` or `fail(message, error=e)`.

```python
# Functions to implement:
async def create_group_via_admin_api(rpc_url, app_key, application_id, upgrade_policy, admin_identity) -> dict
async def delete_group_via_admin_api(rpc_url, group_id, requester) -> dict
async def get_group_info_via_admin_api(rpc_url, group_id) -> dict
async def add_group_members_via_admin_api(rpc_url, group_id, members, requester) -> dict
async def remove_group_members_via_admin_api(rpc_url, group_id, members, requester) -> dict
async def list_group_members_via_admin_api(rpc_url, group_id, offset=None, limit=None) -> dict
async def list_group_contexts_via_admin_api(rpc_url, group_id, offset=None, limit=None) -> dict
async def upgrade_group_via_admin_api(rpc_url, group_id, target_application_id, requester, migrate_method=None) -> dict
async def get_group_upgrade_status_via_admin_api(rpc_url, group_id) -> dict
async def retry_group_upgrade_via_admin_api(rpc_url, group_id, requester) -> dict
async def create_group_invitation_via_admin_api(rpc_url, group_id, requester, invitee_identity=None, expiration=None) -> dict
async def join_group_via_admin_api(rpc_url, invitation_payload, joiner_identity) -> dict
```

**HTTP call pattern** (using `aiohttp`):

```python
import aiohttp
from merobox.commands.result import ok, fail
from merobox.commands.retry import NETWORK_RETRY_CONFIG, with_retry
from merobox.commands.constants import ADMIN_API_GROUPS

@with_retry(config=NETWORK_RETRY_CONFIG)
async def create_group_via_admin_api(rpc_url, app_key, application_id, upgrade_policy, admin_identity):
    try:
        url = f"{rpc_url}{ADMIN_API_GROUPS}"
        payload = {
            "appKey": app_key,
            "applicationId": application_id,
            "upgradePolicy": upgrade_policy,
            "adminIdentity": admin_identity,
        }
        async with aiohttp.ClientSession() as session:
            async with session.post(url, json=payload) as resp:
                data = await resp.json()
                if resp.status == 200:
                    return ok(data, endpoint=url)
                else:
                    return fail(f"create_group returned {resp.status}: {data}", endpoint=url)
    except Exception as e:
        return fail("create_group failed", error=e)
```

### 1.3 Admin API Endpoints → HTTP Mapping

| Step Type | HTTP Method | Endpoint | Request Body (camelCase JSON) |
|---|---|---|---|
| `create_group` | `POST` | `/admin-api/groups` | `{appKey, applicationId, upgradePolicy, adminIdentity}` |
| `delete_group` | `DELETE` | `/admin-api/groups/:group_id` | `{requester}` (as JSON body) |
| `get_group_info` | `GET` | `/admin-api/groups/:group_id` | — |
| `add_group_members` | `POST` | `/admin-api/groups/:group_id/members` | `{members: [{identity, role}], requester}` |
| `remove_group_members` | `POST` | `/admin-api/groups/:group_id/members/remove` | `{members: [identity], requester}` |
| `list_group_members` | `GET` | `/admin-api/groups/:group_id/members` | Query: `?offset=&limit=` |
| `list_group_contexts` | `GET` | `/admin-api/groups/:group_id/contexts` | Query: `?offset=&limit=` |
| `upgrade_group` | `POST` | `/admin-api/groups/:group_id/upgrade` | `{targetApplicationId, requester, migrateMethod?}` |
| `get_group_upgrade_status` | `GET` | `/admin-api/groups/:group_id/upgrade/status` | — |
| `retry_group_upgrade` | `POST` | `/admin-api/groups/:group_id/upgrade/retry` | `{requester}` |
| `create_group_invitation` | `POST` | `/admin-api/groups/:group_id/invite` | `{requester, inviteeIdentity?, expiration?}` |
| `join_group` | `POST` | `/admin-api/groups/join` | `{invitationPayload, joinerIdentity}` |

### 1.4 New Step Classes

All step classes go in: `.merobox-src/merobox/commands/bootstrap/steps/groups.py`

Each class extends `BaseStep` and follows the standard pattern (see `invite_open.py`, `blob.py`):

#### CreateGroupStep

```yaml
# YAML usage:
- type: create_group
  node: e2e-node-1
  app_key: "{{app_key_hex}}"           # hex-encoded 32 bytes
  application_id: "{{app_id}}"
  upgrade_policy: "admin_initiated"     # or "automatic"
  admin_identity: "{{node1_key}}"
  outputs:
    group_id: groupId
```

**Required fields**: `node`, `app_key`, `application_id`, `upgrade_policy`, `admin_identity`
**Exportable variables**: `groupId` → `group_id_{node_name}`

#### DeleteGroupStep

```yaml
- type: delete_group
  node: e2e-node-1
  group_id: "{{group_id}}"
  requester: "{{node1_key}}"
  outputs:
    is_deleted: isDeleted
```

**Required fields**: `node`, `group_id`, `requester`
**Exportable variables**: `isDeleted` → `group_deleted_{node_name}`

#### GetGroupInfoStep

```yaml
- type: get_group_info
  node: e2e-node-1
  group_id: "{{group_id}}"
  outputs:
    group_info: data
```

**Required fields**: `node`, `group_id`
**Exportable variables**: `groupId`, `appKey`, `targetApplicationId`, `upgradePolicy`, `memberCount`, `contextCount`

#### AddGroupMembersStep

```yaml
- type: add_group_members
  node: e2e-node-1
  group_id: "{{group_id}}"
  requester: "{{node1_key}}"
  members:
    - identity: "{{public_key_e2e-node-2}}"
      role: "member"
    - identity: "{{public_key_e2e-node-3}}"
      role: "admin"
```

**Required fields**: `node`, `group_id`, `members`, `requester`
**Exportable variables**: none (success/fail only)

#### RemoveGroupMembersStep

```yaml
- type: remove_group_members
  node: e2e-node-1
  group_id: "{{group_id}}"
  requester: "{{node1_key}}"
  members:
    - "{{public_key_e2e-node-3}}"
```

**Required fields**: `node`, `group_id`, `members`, `requester`
**Exportable variables**: none

#### ListGroupMembersStep

```yaml
- type: list_group_members
  node: e2e-node-1
  group_id: "{{group_id}}"
  outputs:
    members_list: data
```

**Required fields**: `node`, `group_id`
**Exportable variables**: `data` → `group_members_{node_name}_{group_id}`

#### ListGroupContextsStep

```yaml
- type: list_group_contexts
  node: e2e-node-1
  group_id: "{{group_id}}"
  outputs:
    contexts_list: data
```

**Required fields**: `node`, `group_id`
**Exportable variables**: `data` → `group_contexts_{node_name}_{group_id}`

#### UpgradeGroupStep

```yaml
- type: upgrade_group
  node: e2e-node-1
  group_id: "{{group_id}}"
  target_application_id: "{{new_app_id}}"
  requester: "{{node1_key}}"
  migrate_method: "migrate"           # optional
  outputs:
    upgrade_status: status
```

**Required fields**: `node`, `group_id`, `target_application_id`, `requester`
**Exportable variables**: `groupId`, `status`, `total`, `completed`, `failed`

#### GetGroupUpgradeStatusStep

```yaml
- type: get_group_upgrade_status
  node: e2e-node-1
  group_id: "{{group_id}}"
  outputs:
    status_data: data
```

**Required fields**: `node`, `group_id`
**Exportable variables**: `data` → `upgrade_status_{node_name}_{group_id}`

#### RetryGroupUpgradeStep

```yaml
- type: retry_group_upgrade
  node: e2e-node-1
  group_id: "{{group_id}}"
  requester: "{{node1_key}}"
```

**Required fields**: `node`, `group_id`, `requester`

#### CreateGroupInvitationStep

```yaml
- type: create_group_invitation
  node: e2e-node-1
  group_id: "{{group_id}}"
  requester: "{{node1_key}}"
  invitee_identity: "{{public_key_e2e-node-2}}"  # optional (targeted)
  expiration: 1740600000                           # optional (unix epoch)
  outputs:
    invitation_payload: payload
```

**Required fields**: `node`, `group_id`, `requester`
**Exportable variables**: `payload` → `group_invitation_{node_name}_{group_id}`

#### JoinGroupStep

```yaml
- type: join_group
  node: e2e-node-2
  invitation_payload: "{{invitation_payload}}"
  joiner_identity: "{{public_key_e2e-node-2}}"
  outputs:
    joined_group_id: groupId
    member_identity: memberIdentity
```

**Required fields**: `node`, `invitation_payload`, `joiner_identity`
**Exportable variables**: `groupId` → `joined_group_id_{node_name}`, `memberIdentity` → `joined_member_{node_name}`

### 1.5 Registration Changes

**File**: `.merobox-src/merobox/commands/bootstrap/steps/__init__.py`

Add imports:

```python
from merobox.commands.bootstrap.steps.groups import (
    CreateGroupStep,
    DeleteGroupStep,
    GetGroupInfoStep,
    AddGroupMembersStep,
    RemoveGroupMembersStep,
    ListGroupMembersStep,
    ListGroupContextsStep,
    UpgradeGroupStep,
    GetGroupUpgradeStatusStep,
    RetryGroupUpgradeStep,
    CreateGroupInvitationStep,
    JoinGroupStep,
)
```

**File**: `.merobox-src/merobox/commands/bootstrap/run/executor.py`

Add to `_create_step_executor()`:

```python
elif step_type == "create_group":
    from merobox.commands.bootstrap.steps.groups import CreateGroupStep
    return CreateGroupStep(step_config, manager=self.manager)
elif step_type == "delete_group":
    from merobox.commands.bootstrap.steps.groups import DeleteGroupStep
    return DeleteGroupStep(step_config, manager=self.manager)
elif step_type == "get_group_info":
    from merobox.commands.bootstrap.steps.groups import GetGroupInfoStep
    return GetGroupInfoStep(step_config, manager=self.manager)
elif step_type == "add_group_members":
    from merobox.commands.bootstrap.steps.groups import AddGroupMembersStep
    return AddGroupMembersStep(step_config, manager=self.manager)
elif step_type == "remove_group_members":
    from merobox.commands.bootstrap.steps.groups import RemoveGroupMembersStep
    return RemoveGroupMembersStep(step_config, manager=self.manager)
elif step_type == "list_group_members":
    from merobox.commands.bootstrap.steps.groups import ListGroupMembersStep
    return ListGroupMembersStep(step_config, manager=self.manager)
elif step_type == "list_group_contexts":
    from merobox.commands.bootstrap.steps.groups import ListGroupContextsStep
    return ListGroupContextsStep(step_config, manager=self.manager)
elif step_type == "upgrade_group":
    from merobox.commands.bootstrap.steps.groups import UpgradeGroupStep
    return UpgradeGroupStep(step_config, manager=self.manager)
elif step_type == "get_group_upgrade_status":
    from merobox.commands.bootstrap.steps.groups import GetGroupUpgradeStatusStep
    return GetGroupUpgradeStatusStep(step_config, manager=self.manager)
elif step_type == "retry_group_upgrade":
    from merobox.commands.bootstrap.steps.groups import RetryGroupUpgradeStep
    return RetryGroupUpgradeStep(step_config, manager=self.manager)
elif step_type == "create_group_invitation":
    from merobox.commands.bootstrap.steps.groups import CreateGroupInvitationStep
    return CreateGroupInvitationStep(step_config, manager=self.manager)
elif step_type == "join_group":
    from merobox.commands.bootstrap.steps.groups import JoinGroupStep
    return JoinGroupStep(step_config, manager=self.manager)
```

---

## Part 2: File Changes Summary (Merobox)

| File | Action | Description |
|---|---|---|
| `.merobox-src/merobox/commands/constants.py` | Edit | Add `ADMIN_API_GROUPS` + step type constants |
| `.merobox-src/merobox/commands/groups.py` | **New** | 12 async API helper functions using `aiohttp` |
| `.merobox-src/merobox/commands/bootstrap/steps/groups.py` | **New** | 12 step classes (one per group operation) |
| `.merobox-src/merobox/commands/bootstrap/steps/__init__.py` | Edit | Import + export 12 new step classes |
| `.merobox-src/merobox/commands/bootstrap/run/executor.py` | Edit | Register 12 new step types in `_create_step_executor()` |

---

## Part 3: E2E Workflow File

**New file**: `apps/e2e-kv-store/workflows/e2e-groups.yml`

### 3.1 Test Scenario Overview

| # | Scenario | Phases Covered | Steps |
|---|---|---|---|
| 1 | Setup: Install app & create identities | Prerequisite | `install_application`, `create_identity` |
| 2 | Group lifecycle: create, get info, delete | Phase 1 | `create_group`, `get_group_info`, `delete_group` |
| 3 | Member management: add, list, remove | Phase 2 | `create_group`, `add_group_members`, `list_group_members`, `remove_group_members` |
| 4 | Context association via group-aware create | Phase 3 | `create_group`, `create_context` (with group_id), `list_group_contexts` |
| 5 | Group upgrade propagation | Phase 4 | `create_group`, create contexts, `upgrade_group`, `get_group_upgrade_status` |
| 6 | Admin-only authorization | Phase 5 | Non-admin attempts group operations → `expected_failure: true` |
| 7 | Group invitations: open + targeted | Phase 6 | `create_group_invitation`, `join_group`, `list_group_members` verify |
| 8 | Full lifecycle integration | All | Complete flow: create group → add members → create contexts → invite → join → upgrade → delete |

### 3.2 Workflow YAML Spec

```yaml
name: E2E Context Groups - Full Feature Test
description: End-to-end tests for Context Groups features (Phases 1-6)

force_pull_image: false

nodes:
  chain_id: testnet-1
  count: 3
  image: ghcr.io/calimero-network/merod:edge
  prefix: e2e-node

steps:
  # ============================================================
  # SETUP: Install application and create identities
  # ============================================================

  - name: Install application on node-1
    type: install_application
    node: e2e-node-1
    path: res/e2e_kv_store.wasm
    dev: true
    outputs:
      app_id: applicationId

  - name: Assert application installed
    type: assert
    statements:
      - "is_set({{app_id}})"

  - name: Create identity on node-1
    type: create_identity
    node: e2e-node-1
    outputs:
      node1_key: publicKey

  - name: Create identity on node-2
    type: create_identity
    node: e2e-node-2
    outputs:
      node2_key: publicKey

  - name: Create identity on node-3
    type: create_identity
    node: e2e-node-3
    outputs:
      node3_key: publicKey

  # ============================================================
  # SCENARIO 1: Group Lifecycle (Phase 1)
  # ============================================================

  - name: Generate app_key for group
    type: script
    command: python3 -c "import os; print(os.urandom(32).hex())"
    outputs:
      app_key: stdout

  - name: Create a group
    type: create_group
    node: e2e-node-1
    app_key: "{{app_key}}"
    application_id: "{{app_id}}"
    upgrade_policy: "admin_initiated"
    admin_identity: "{{node1_key}}"
    outputs:
      group_id: groupId

  - name: Assert group created
    type: assert
    statements:
      - "is_set({{group_id}})"

  - name: Get group info
    type: get_group_info
    node: e2e-node-1
    group_id: "{{group_id}}"
    outputs:
      group_info: data

  - name: Assert group info correct
    type: json_assert
    statements:
      - 'json_equal({{group_info}}, {"memberCount": 1, "contextCount": 0})'

  # ============================================================
  # SCENARIO 2: Member Management (Phase 2)
  # ============================================================

  - name: Add node-2 as member to group
    type: add_group_members
    node: e2e-node-1
    group_id: "{{group_id}}"
    requester: "{{node1_key}}"
    members:
      - identity: "{{node2_key}}"
        role: "member"

  - name: Add node-3 as admin to group
    type: add_group_members
    node: e2e-node-1
    group_id: "{{group_id}}"
    requester: "{{node1_key}}"
    members:
      - identity: "{{node3_key}}"
        role: "admin"

  - name: List group members
    type: list_group_members
    node: e2e-node-1
    group_id: "{{group_id}}"
    outputs:
      members_list: data

  - name: Assert 3 members in group
    type: assert
    statements:
      - "len({{members_list}}) == 3"

  - name: Remove node-3 from group
    type: remove_group_members
    node: e2e-node-1
    group_id: "{{group_id}}"
    requester: "{{node1_key}}"
    members:
      - "{{node3_key}}"

  - name: List members after removal
    type: list_group_members
    node: e2e-node-1
    group_id: "{{group_id}}"
    outputs:
      members_after_remove: data

  - name: Assert 2 members remain
    type: assert
    statements:
      - "len({{members_after_remove}}) == 2"

  # ============================================================
  # SCENARIO 3: Context Association (Phase 3)
  # ============================================================
  # NOTE: Context creation with group_id requires the create_context
  # step to pass group_id. This may need the create_context step to
  # be extended, or use create_mesh with a group_id parameter.
  # For now, we verify contexts via list_group_contexts.

  - name: List group contexts (should be empty)
    type: list_group_contexts
    node: e2e-node-1
    group_id: "{{group_id}}"
    outputs:
      contexts_empty: data

  - name: Assert no contexts yet
    type: assert
    statements:
      - "len({{contexts_empty}}) == 0"

  # ============================================================
  # SCENARIO 4: Group Invitations (Phase 6)
  # ============================================================

  # --- 4a: Create a second group for invitation testing ---

  - name: Generate app_key for invitation group
    type: script
    command: python3 -c "import os; print(os.urandom(32).hex())"
    outputs:
      invite_app_key: stdout

  - name: Create invitation test group
    type: create_group
    node: e2e-node-1
    app_key: "{{invite_app_key}}"
    application_id: "{{app_id}}"
    upgrade_policy: "admin_initiated"
    admin_identity: "{{node1_key}}"
    outputs:
      invite_group_id: groupId

  # --- 4b: Open invitation (no target identity) ---

  - name: Create open group invitation
    type: create_group_invitation
    node: e2e-node-1
    group_id: "{{invite_group_id}}"
    requester: "{{node1_key}}"
    outputs:
      open_invitation: payload

  - name: Assert invitation created
    type: assert
    statements:
      - "is_set({{open_invitation}})"

  - name: Join group via open invitation (node-2)
    type: join_group
    node: e2e-node-2
    invitation_payload: "{{open_invitation}}"
    joiner_identity: "{{node2_key}}"
    outputs:
      joined_group: groupId

  - name: Assert joined correct group
    type: assert
    statements:
      - "{{joined_group}} == {{invite_group_id}}"

  # --- 4c: Targeted invitation ---

  - name: Create targeted group invitation for node-3
    type: create_group_invitation
    node: e2e-node-1
    group_id: "{{invite_group_id}}"
    requester: "{{node1_key}}"
    invitee_identity: "{{node3_key}}"
    outputs:
      targeted_invitation: payload

  - name: Join group via targeted invitation (node-3)
    type: join_group
    node: e2e-node-3
    invitation_payload: "{{targeted_invitation}}"
    joiner_identity: "{{node3_key}}"
    outputs:
      joined_group_3: groupId

  - name: List invitation group members
    type: list_group_members
    node: e2e-node-1
    group_id: "{{invite_group_id}}"
    outputs:
      invite_group_members: data

  - name: Assert 3 members after invitations
    type: assert
    statements:
      - "len({{invite_group_members}}) == 3"

  # --- 4d: Negative test - wrong identity for targeted invitation ---

  - name: Create targeted invitation for node-3
    type: create_group_invitation
    node: e2e-node-1
    group_id: "{{invite_group_id}}"
    requester: "{{node1_key}}"
    invitee_identity: "{{node3_key}}"
    outputs:
      wrong_target_invitation: payload

  - name: Wrong identity tries to join via targeted invitation
    type: join_group
    node: e2e-node-2
    invitation_payload: "{{wrong_target_invitation}}"
    joiner_identity: "{{node2_key}}"
    expected_failure: true

  # --- 4e: Negative test - duplicate join ---

  - name: Node-2 tries to join again (already a member)
    type: join_group
    node: e2e-node-2
    invitation_payload: "{{open_invitation}}"
    joiner_identity: "{{node2_key}}"
    expected_failure: true

  # ============================================================
  # SCENARIO 5: Authorization / Admin-Only (Phase 5)
  # ============================================================

  # Non-admin (node-2) tries to add member — should fail

  - name: Non-admin tries to add member (should fail)
    type: add_group_members
    node: e2e-node-1
    group_id: "{{invite_group_id}}"
    requester: "{{node2_key}}"
    members:
      - identity: "{{node1_key}}"
        role: "member"
    expected_failure: true

  # Non-admin tries to create invitation — should fail

  - name: Non-admin tries to create invitation (should fail)
    type: create_group_invitation
    node: e2e-node-1
    group_id: "{{invite_group_id}}"
    requester: "{{node2_key}}"
    expected_failure: true

  # Non-admin tries to delete group — should fail

  - name: Non-admin tries to delete group (should fail)
    type: delete_group
    node: e2e-node-1
    group_id: "{{invite_group_id}}"
    requester: "{{node2_key}}"
    expected_failure: true

  # ============================================================
  # SCENARIO 6: Group Delete (Phase 1)
  # ============================================================

  - name: Delete original group
    type: delete_group
    node: e2e-node-1
    group_id: "{{group_id}}"
    requester: "{{node1_key}}"
    outputs:
      delete_result: isDeleted

  - name: Assert group deleted
    type: assert
    statements:
      - "{{delete_result}} == true"

  - name: Get deleted group info (should fail)
    type: get_group_info
    node: e2e-node-1
    group_id: "{{group_id}}"
    expected_failure: true
```

### 3.3 Upgrade Scenario (Phase 4)

The upgrade test requires a **second version of the WASM app** to be available. This can be added as a separate workflow or as an extension once a v2 test app is built:

```yaml
  # ============================================================
  # UPGRADE SCENARIO (Phase 4) — requires v2 app
  # ============================================================

  # - name: Install v2 application
  #   type: install_application
  #   node: e2e-node-1
  #   path: res/e2e_kv_store_v2.wasm
  #   dev: true
  #   outputs:
  #     app_id_v2: applicationId

  # - name: Upgrade group to v2
  #   type: upgrade_group
  #   node: e2e-node-1
  #   group_id: "{{invite_group_id}}"
  #   target_application_id: "{{app_id_v2}}"
  #   requester: "{{node1_key}}"
  #   outputs:
  #     upgrade_status: status

  # - name: Check upgrade status
  #   type: get_group_upgrade_status
  #   node: e2e-node-1
  #   group_id: "{{invite_group_id}}"
  #   outputs:
  #     upgrade_info: data

  # - name: Assert upgrade completed
  #   type: json_assert
  #   statements:
  #     - 'json_equal({{upgrade_info}}, {"status": "completed"})'
```

---

## Part 4: Implementation Order

### Step 1: Group API helpers (`.merobox-src/merobox/commands/groups.py`)

Implement the 12 async HTTP helper functions. Each follows the same pattern:
- Build URL from `rpc_url` + endpoint constant
- Construct JSON payload with `camelCase` keys
- Use `aiohttp.ClientSession` for the request
- Return `ok(data)` or `fail(message)`

### Step 2: Group step classes (`.merobox-src/merobox/commands/bootstrap/steps/groups.py`)

Implement 12 step classes. Each follows the pattern from `InviteOpenStep`:
1. `_get_required_fields()` — list required YAML fields
2. `_validate_field_types()` — type-check each field
3. `_get_exportable_variables()` — define auto-export mappings
4. `async execute()` — resolve dynamic values, get RPC URL, call API helper, check result, export variables

### Step 3: Registration (constants, `__init__`, executor)

Wire everything together:
1. Add constants to `constants.py`
2. Add imports to `steps/__init__.py`
3. Add `elif` branches to `executor.py:_create_step_executor()`

### Step 4: Workflow YAML (`apps/e2e-kv-store/workflows/e2e-groups.yml`)

Write the test workflow file.

### Step 5: Verify

Run the workflow locally:
```bash
cd .merobox-src
merobox run ../apps/e2e-kv-store/workflows/e2e-groups.yml
```

---

## Part 5: Notes & Considerations

### `expected_failure` Support

Some step classes may need to support `expected_failure: true` for negative tests. This is already a pattern used in `ExecuteStep` (`execute.py`). The group step base pattern should check:

```python
expected_failure = self.config.get("expected_failure", False)
if not result["success"]:
    if expected_failure:
        console.print("[green]Expected failure occurred (as intended)[/green]")
        return True
    return False
```

### `DELETE` with JSON body

The `delete_group` endpoint uses `DELETE /admin-api/groups/:group_id` with a JSON body `{requester}`. When using `aiohttp`, this requires:

```python
async with session.delete(url, json=payload) as resp:
```

This is supported by `aiohttp` but is unconventional. An alternative is to use `session.request("DELETE", url, json=payload)`.

### Context Creation with Group

Phase 3 (context association) requires creating a context with a `group_id` parameter. The existing `create_context` step and `calimero_client_py.create_context()` method may not support this yet. Options:
1. Extend `CreateContextStep` to optionally pass `group_id`
2. Add a `create_group_context` step type that uses raw HTTP

This needs investigation when implementing — check if the `create_context` admin API endpoint accepts `groupId` in its request body.

### App Key Generation

Groups require an `app_key` (hex-encoded 32-byte string). In tests, this can be generated with the `script` step type:

```yaml
- type: script
  command: python3 -c "import os; print(os.urandom(32).hex())"
  outputs:
    app_key: stdout
```

### Response Shape

All server admin API responses use the pattern `{"data": {...}}` at the top level (see `ApiResponse<T>` in `service.rs`). The step classes should handle this by extracting `result["data"]["data"]` for the actual payload.
