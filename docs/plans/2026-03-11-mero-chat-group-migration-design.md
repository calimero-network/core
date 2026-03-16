# Design: Migrating mero-chat to Group-Based Context Management

**Date:** March 2026
**Status:** In Progress (brainstorming)
**Approach:** B (Full) — every channel and DM becomes its own context within a group

---

## 1. Current Architecture (mero-chat)

```
┌──────────────────────────────────────┐
│ Main Context (1 per workspace)       │
│  ├── channels[] (app-level state)    │
│  │   ├── #general (app-level)        │
│  │   ├── #engineering (app-level)    │
│  │   └── #private-team (app-level)   │
│  ├── messages[] (all channels mixed) │
│  └── members[] (all users)           │
├──────────────────────────────────────┤
│ DM Context 1 (alice ↔ bob)           │
│ DM Context 2 (alice ↔ carol)         │
└──────────────────────────────────────┘
```

### How it works today

- **1 shared context** holds all channels. Channels are app-level constructs
  managed by the WASM/JS logic (`logic-js/src/channelManagement/`).
- **1 context per DM**. DMs already use separate contexts.
- **Context-level invitations** for joining the workspace.
- **No group concept**. Membership, channel visibility, and upgrade
  propagation are all manual.

### Key files

| File | Purpose |
|------|---------|
| `logic-js/src/channelManagement/` | Channel CRUD (create, join, leave, list, invite) |
| `logic-js/src/dmManagement/` | DM lifecycle (create, accept, delete) |
| `logic-js/src/messageManagement/` | Messages (send, get, edit, delete, reactions) |
| `app/src/api/dataSource/nodeApiDataSource.ts` | Context CRUD (create, join, list) |
| `app/src/api/dataSource/clientApiDataSource.ts` | RPC calls to WASM logic |
| `app/src/components/sideSelector/` | Channel list, DM list UI |
| `app/src/contexts/WebSocketContext.tsx` | Multi-context WebSocket subscriptions |
| `app/src/utils/session.ts` | DM context ID storage |
| `app/src/utils/invitation.ts` | Invitation payload parsing |

### Current API calls

**Node admin API:**
- `POST /admin-api/contexts` — create context
- `GET /admin-api/contexts` — list contexts
- `POST /admin-api/contexts/join` — join via invitation
- `POST /admin-api/identity/context` — create identity

**Client RPC (app logic):**
- `join_chat`, `get_chat_members` — workspace membership
- `create_channel`, `get_channels`, `join_channel`, `leave_channel` — channel CRUD
- `invite_to_channel`, `get_channel_members`, `get_non_member_users` — channel membership
- `create_dm_chat`, `get_dms`, `accept_invitation`, `delete_dm` — DM lifecycle
- `send_message`, `get_messages`, `edit_message`, `delete_message` — messages
- `update_reaction` — reactions

---

## 2. Target Architecture (group-based)

```
┌──────────────────────────────────────────┐
│ Group "Team Chat" (workspace)            │
│  app: mero-chat-v2                       │
│  members: [alice, bob, carol]            │
│  default_capabilities:                   │
│    CAN_JOIN_OPEN_CONTEXTS                │
│    CAN_CREATE_CONTEXT                    │
│  default_visibility: Open                │
│                                          │
│  Contexts (each = one channel or DM):    │
│  ├── #general       (Open)              │
│  ├── #engineering   (Open)              │
│  ├── #private-team  (Restricted)         │
│  │    allowlist: [alice, bob]            │
│  ├── DM:alice↔bob   (Restricted)         │
│  │    allowlist: [alice, bob]            │
│  └── DM:alice↔carol (Restricted)         │
│       allowlist: [alice, carol]          │
└──────────────────────────────────────────┘
```

### Key shifts

| Concept | Before | After |
|---------|--------|-------|
| Workspace | 1 shared context | 1 group |
| Channel | App-level state in shared context | Own context (Open visibility) |
| Private channel | App-level state with invite | Own context (Restricted + allowlist) |
| DM | Separate context (already) | Restricted context in group (2-person allowlist) |
| Channel list | App RPC: `get_channels` | Group API: `GET /groups/:id/contexts` |
| Join channel | App RPC: `join_channel` | Group API: `POST /groups/:id/join-context` |
| Channel members | App RPC: `get_channel_members` | Context API: identity list |
| Channel visibility | App-level (public/private flag) | System-level: Open vs Restricted |
| Workspace invitation | Context invitation | Group invitation |
| Upgrade propagation | Manual per-context | `group upgrade trigger` (all contexts) |
| Member removal | Manual per-context | Cascade removal from all channels |

---

## 3. Simplified App Logic (mero-chat-v2)

The WASM/JS logic shrinks dramatically. Each context only needs:

### What stays (per-context app logic)

```
mero-chat-v2 logic:
├── messages/
│   ├── send_message(content, reply_to?)
│   ├── get_messages(before?, limit?)
│   ├── edit_message(id, content)
│   ├── delete_message(id)
│   └── update_reaction(message_id, emoji)
├── metadata/
│   ├── init(name, description, type)  // called once at context creation
│   ├── get_info() -> { name, description, type, created_at }
│   └── update_info(name?, description?)
└── members/
    ├── set_profile(username, avatar?)  // per-context display name
    └── get_profiles() -> [{ identity, username, avatar }]
```

### What gets removed (handled by group system)

| Removed from app logic | Replaced by |
|------------------------|-------------|
| `create_channel` | `POST /admin-api/contexts` with `--group-id` |
| `get_channels` | `GET /admin-api/groups/:id/contexts` |
| `join_channel` / `leave_channel` | `POST /admin-api/groups/:id/join-context` |
| `invite_to_channel` | `POST /admin-api/groups/:id/contexts/:ctx/allowlist` (for Restricted) |
| `get_channel_members` | Context identity list |
| `get_non_member_users` | Group members minus context members |
| `create_dm_chat` | Create Restricted context + allowlist add |
| `accept_invitation` / `update_invitation_payload` | Group invitation flow |
| `join_chat` / `get_chat_members` | Group membership APIs |
| All channel management state | Group context list |
| All DM setup state machine | Simplified: create context + set restricted + add allowlist |

---

## 4. Frontend Changes

### 4.1 Onboarding Flow

**Before:**
```
Open app → Create identity → Join context via invitation → join_chat RPC
```

**After:**
```
Open app → Join GROUP via invitation → Auto-subscribe to group P2P topic
  → See channel list from group contexts → Join channels via group membership
```

**API calls:**
```
1. POST /admin-api/groups/join         (join group via invitation payload)
2. GET  /admin-api/groups/:id/contexts (list available channels)
3. POST /admin-api/groups/:id/join-context (join #general or any Open channel)
```

### 4.2 Channel List (Sidebar)

**Before:** `useChannels` hook → `get_channels` RPC → app-level channel state

**After:** `useGroupContexts` hook → `GET /admin-api/groups/:id/contexts` → system-level context list

Each context needs metadata (name, type). Two options:
- **Option 1:** Store in context init params (set at creation, read from context config)
- **Option 2:** RPC call `get_info()` per context (read from app state)

Recommend **Option 2** — app state is mutable (rename channel), and the RPC
is local (no network call).

**Channel categorization:**
```
Group contexts → for each context:
  get_info() → { name, type: "channel" | "dm", description }

Render:
  CHANNELS
    #general        (type: channel, visibility: Open)
    #engineering    (type: channel, visibility: Open)
    #private-team   (type: channel, visibility: Restricted)
  DIRECT MESSAGES
    alice ↔ bob     (type: dm, visibility: Restricted)
    alice ↔ carol   (type: dm, visibility: Restricted)
```

### 4.3 Creating a Channel

**Before:**
```ts
clientApi.createChannel({ name: "engineering", is_public: true })
```

**After:**
```ts
// 1. Create context in group
const contextId = await nodeApi.createContext({
  applicationId: APP_ID,
  protocol: "near",
  groupId: GROUP_ID,
  initializationParams: encode({ name: "engineering", type: "channel" }),
});

// 2. If private, set Restricted visibility + add members to allowlist
if (!isPublic) {
  await groupApi.setContextVisibility(GROUP_ID, contextId, "restricted");
  await groupApi.addToAllowlist(GROUP_ID, contextId, memberPKs);
}
```

### 4.4 Creating a DM

**Before:** Complex multi-step state machine (`dmSetupState.ts`) with
invitation payloads, identity exchange, and polling.

**After:**
```ts
// 1. Create context in group
const contextId = await nodeApi.createContext({
  applicationId: APP_ID,
  protocol: "near",
  groupId: GROUP_ID,
  initializationParams: encode({ name: null, type: "dm" }),
});

// 2. Set Restricted visibility
await groupApi.setContextVisibility(GROUP_ID, contextId, "restricted");

// 3. Add both participants to allowlist
await groupApi.addToAllowlist(GROUP_ID, contextId, [myPK, otherPK]);

// 4. Other user joins via group membership (they're already in the group)
// They see the new context on next sync/notification and join it
```

The entire DM state machine (`Creator`, `Invitee`, `WaitingForInvitation`,
`SyncWaiting`, etc.) collapses. Both users are already group members — they
just need to be on the allowlist and join the context.

### 4.5 Joining a Channel

**Before:** `join_channel` RPC (app-level)

**After:**
```ts
// Open channel — any group member with CAN_JOIN_OPEN_CONTEXTS
await groupApi.joinGroupContext(GROUP_ID, contextId);

// Restricted channel — must be on allowlist first (admin or creator adds them)
// Then:
await groupApi.joinGroupContext(GROUP_ID, contextId);
```

### 4.6 WebSocket Subscriptions

**Before:** Subscribe to main context + all DM contexts.

**After:** Subscribe to ALL contexts the user has joined in the group.
```ts
// On login, after joining group:
const myContexts = await groupApi.listGroupContexts(GROUP_ID);
// Filter to contexts user has joined
for (const ctx of joinedContexts) {
  subscribeToContext(ctx.id);
}
```

The existing `useMultiWebSocketSubscription` already handles multiple
contexts. The change is just the source of context IDs (group API instead of
session storage).

### 4.7 Switching Channels

**Before:** Channels within same context → just change app-level filter.

**After:** Channels are different contexts → switch `contextId` and
`executorPublicKey`. The user has a different identity per context.

```ts
const switchChannel = (contextId: string) => {
  setContextId(contextId);
  const identity = getContextIdentity(contextId); // stored locally after join
  setExecutorPublicKey(identity);
  // Messages load from the new context
};
```

This is a bigger change than before — channel switching now means context
switching. But it's cleaner (each channel's state is isolated).

---

## 5. New Frontend API Layer

### Group API service (`groupApiDataSource.ts`)

```ts
// Group CRUD
createGroup(applicationId: string): Promise<string>
getGroup(groupId: string): Promise<GroupInfo>
deleteGroup(groupId: string): Promise<void>

// Membership
createInvitation(groupId: string): Promise<string>
joinGroup(invitationPayload: string): Promise<void>
listMembers(groupId: string): Promise<GroupMember[]>
removeMember(groupId: string, identity: string): Promise<void>

// Contexts (channels/DMs)
listGroupContexts(groupId: string): Promise<ContextId[]>
joinGroupContext(groupId: string, contextId: string): Promise<void>
syncGroup(groupId: string): Promise<void>

// Visibility & Permissions
setContextVisibility(groupId: string, contextId: string, mode: "open" | "restricted"): Promise<void>
getContextVisibility(groupId: string, contextId: string): Promise<VisibilityInfo>
addToAllowlist(groupId: string, contextId: string, members: string[]): Promise<void>
removeFromAllowlist(groupId: string, contextId: string, members: string[]): Promise<void>
getContextAllowlist(groupId: string, contextId: string): Promise<string[]>

// Capabilities
setMemberCapabilities(groupId: string, identity: string, caps: Capabilities): Promise<void>
getMemberCapabilities(groupId: string, identity: string): Promise<Capabilities>

// Upgrade
triggerUpgrade(groupId: string, targetAppId: string, migrateMethod?: string): Promise<void>
getUpgradeStatus(groupId: string): Promise<UpgradeStatus>
```

### Mapping to HTTP endpoints

| Method | HTTP |
|--------|------|
| `createGroup` | `POST /admin-api/groups` |
| `getGroup` | `GET /admin-api/groups/:id` |
| `createInvitation` | `POST /admin-api/groups/:id/invite` |
| `joinGroup` | `POST /admin-api/groups/join` |
| `listMembers` | `GET /admin-api/groups/:id/members` |
| `listGroupContexts` | `GET /admin-api/groups/:id/contexts` |
| `joinGroupContext` | `POST /admin-api/groups/:id/join-context` |
| `syncGroup` | `POST /admin-api/groups/:id/sync` |
| `setContextVisibility` | `PUT /admin-api/groups/:id/contexts/:ctx/visibility` |
| `getContextVisibility` | `GET /admin-api/groups/:id/contexts/:ctx/visibility` |
| `addToAllowlist` | `POST /admin-api/groups/:id/contexts/:ctx/allowlist` |
| `getContextAllowlist` | `GET /admin-api/groups/:id/contexts/:ctx/allowlist` |
| `setMemberCapabilities` | `PUT /admin-api/groups/:id/members/:id/capabilities` |
| `getMemberCapabilities` | `GET /admin-api/groups/:id/members/:id/capabilities` |
| `triggerUpgrade` | `POST /admin-api/groups/:id/upgrade` |
| `getUpgradeStatus` | `GET /admin-api/groups/:id/upgrade/status` |

---

## 6. What Gets Deleted

### Logic layer (`logic-js/`)

| Module | Status |
|--------|--------|
| `channelManagement/` | **Delete entirely** — replaced by group context APIs |
| `dmManagement/` | **Delete entirely** — replaced by restricted contexts + allowlists |
| `messageManagement/` | **Keep** — messages stay as app-level state |
| `types/` | **Simplify** — remove channel/DM types, keep message types |
| `index.ts` (CurbChat class) | **Rewrite** — remove channel/DM methods, add `init`, `get_info`, `set_profile` |

### Frontend (`app/src/`)

| File/Component | Status |
|----------------|--------|
| `utils/dmSetupState.ts` | **Delete** — DM state machine no longer needed |
| `utils/session.ts` (DM parts) | **Simplify** — remove DM context ID tracking |
| `components/contextOperations/` | **Rewrite** — group join replaces context join |
| `components/popups/StartDMPopup.tsx` | **Rewrite** — create restricted context + allowlist |
| `components/popups/InvitationHandlerPopup.tsx` | **Rewrite** — group invitation, not context |
| `hooks/useChannels.ts` | **Rewrite** — fetch from group contexts API |
| `hooks/useDMs.ts` | **Rewrite** — filter group contexts by type=dm |
| `api/dataSource/clientApiDataSource.ts` | **Simplify** — remove channel/DM RPC calls |
| `api/dataSource/nodeApiDataSource.ts` | **Keep** — still need context create |
| New: `api/dataSource/groupApiDataSource.ts` | **Create** — all group API calls |

---

## 7. Migration Path (Implementation Order)

### Phase 1: New app logic (`mero-chat-v2` WASM/JS)

1. Strip `channelManagement/` and `dmManagement/` from logic
2. Keep `messageManagement/` as-is
3. Add `init(name, type, description)` — called once at context creation
4. Add `get_info()` — returns channel/DM metadata
5. Add `set_profile(username)` / `get_profiles()` — per-context display names
6. Build and sign the new bundle

### Phase 2: Group API layer (frontend)

1. Create `groupApiDataSource.ts` with all group HTTP calls
2. Create React hooks: `useGroup`, `useGroupContexts`, `useGroupMembers`
3. Add group state to app context (groupId, group info, member list)

### Phase 3: Onboarding flow

1. Replace context invitation with group invitation
2. New onboarding: create group (admin) or join group (member)
3. Auto-join #general channel after group join
4. Store `groupId` in session/localStorage

### Phase 4: Channel management

1. Replace `useChannels` → fetch from group contexts + `get_info()` per context
2. Channel creation → create context in group + init RPC
3. Channel joining → `joinGroupContext`
4. Private channels → Restricted visibility + allowlist
5. Channel switching → context switching (change contextId + identity)

### Phase 5: DM management

1. Replace DM state machine with: create restricted context + allowlist
2. DM list → filter group contexts where `type == "dm"`
3. DM creation → create context + set restricted + add 2 users to allowlist
4. Remove `dmSetupState.ts` entirely

### Phase 6: Admin features

1. Member management UI (capabilities, removal)
2. Channel visibility controls (open/restricted toggle)
3. Allowlist management UI
4. Upgrade trigger UI (optional)

---

## 8. Local Testing Plan

### Prerequisites

- 2 local `merod` nodes (node-a, node-b) running
- Contract deployed (`ctx-groups-v3.testnet`)
- `mero-chat-v2` app bundle built

### Test sequence (meroctl)

```bash
# 1. Install app
APP_ID=$(meroctl --node node-a app install --path mero-chat-v2.mpk)

# 2. Create workspace (group)
GROUP_ID=$(meroctl --node node-a group create --application-id "$APP_ID")

# 3. Set defaults: members can create contexts + join open ones
meroctl --node node-a group settings set-default-capabilities "$GROUP_ID" \
  --can-join-open-contexts --can-create-context

# 4. Invite Node B
INVITE=$(meroctl --node node-a group invite "$GROUP_ID")
meroctl --node node-b group join "$INVITE"

# 5. Create #general channel (Open)
GENERAL=$(meroctl --node node-a context create \
  --protocol near --application-id "$APP_ID" --group-id "$GROUP_ID")
# Init: meroctl --node node-a call init --context "$GENERAL" --as "$ID" \
#   --args '{"name":"general","type":"channel","description":"General chat"}'

# 6. Node B joins #general
meroctl --node node-b group join-group-context "$GROUP_ID" --context-id "$GENERAL"

# 7. Both nodes send messages
meroctl --node node-a call send_message --context "$GENERAL" --as "$ID_A" \
  --args '{"content":"Hello from Node A!"}'
meroctl --node node-b call send_message --context "$GENERAL" --as "$ID_B" \
  --args '{"content":"Hello from Node B!"}'

# 8. Create private channel (Restricted)
PRIVATE=$(meroctl --node node-a context create \
  --protocol near --application-id "$APP_ID" --group-id "$GROUP_ID")
meroctl --node node-a group contexts set-visibility "$GROUP_ID" "$PRIVATE" --mode restricted
meroctl --node node-a group contexts allowlist add "$GROUP_ID" "$PRIVATE" "$JOINER_PK"

# 9. Node B joins private channel (on allowlist)
meroctl --node node-b group join-group-context "$GROUP_ID" --context-id "$PRIVATE"

# 10. Create DM (Restricted, 2-person allowlist)
DM=$(meroctl --node node-a context create \
  --protocol near --application-id "$APP_ID" --group-id "$GROUP_ID")
meroctl --node node-a group contexts set-visibility "$GROUP_ID" "$DM" --mode restricted
ADMIN_PK=$(meroctl --node node-a node identity | sed 's/^ed25519://')
meroctl --node node-a group contexts allowlist add "$GROUP_ID" "$DM" "$ADMIN_PK" "$JOINER_PK"

# 11. Upgrade all channels at once
APP_V2=$(meroctl --node node-a app install --path mero-chat-v3.mpk)
meroctl --node node-a group upgrade trigger "$GROUP_ID" \
  --target-application-id "$APP_V2" --migrate-method migrate_v1_to_v2

# 12. Cascade removal — remove Node B from workspace
meroctl --node node-a group members remove "$GROUP_ID" "$JOINER_PK"
# Node B loses access to ALL channels and DMs instantly
```

### Frontend testing (browser)

1. Node A opens `http://localhost:2428` → creates group → gets invite link
2. Node B opens `http://localhost:2429` → pastes invite → joins group
3. Both see #general in sidebar → can send messages
4. Node A creates private channel → only visible to allowlisted members
5. Node A starts DM with Node B → both see it in DM list
6. Admin triggers upgrade → all channels migrate seamlessly
7. Admin removes Node B → Node B loses all channel access

---

## 9. Open Questions

1. **Channel metadata storage:** Should channel name/type be in context init
   params (immutable) or app state (mutable via RPC)? Leaning toward app state
   for rename support.

2. **Auto-join #general:** Should joining a group auto-create and auto-join a
   #general channel? Or is that app-level behavior?

3. **Channel discovery:** How does a member discover which Open channels exist
   without joining them? Currently `GET /groups/:id/contexts` returns all
   context IDs, but not metadata. Need to either:
   - Add metadata to the group context list response
   - Or have a "channel browser" that calls `get_info()` on each context
     (requires joining first, which is a chicken-and-egg problem)

4. **Unread counts / notifications:** Per-channel unread state needs to be
   tracked client-side since each channel is a separate context with its own
   WebSocket subscription.

5. **Context creation from frontend:** The `POST /admin-api/contexts` endpoint
   needs to accept `groupId` in the request body (verify this is supported).

---

## 10. Complexity Comparison

| Aspect | Before (app-level channels) | After (group contexts) |
|--------|---------------------------|----------------------|
| App logic complexity | High (channel + DM + message management) | Low (messages + metadata only) |
| Frontend complexity | High (DM state machine, channel state) | Medium (group API calls, context switching) |
| System-level features | None (manual everything) | Cascade removal, upgrade propagation, visibility, capabilities |
| Channel isolation | None (shared state) | Full (separate P2P, separate state) |
| Scalability | Limited (one context = all channels) | Better (per-channel resources) |
| Channel switching cost | Cheap (same context, filter messages) | Moderate (context switch, different identity) |
| Number of API calls | Fewer (one context) | More (one per channel interaction) |
