# Admin-API permission map

Every `/admin-api/*` route is gated by the permission validator
(`crates/auth/src/auth/permissions/validator.rs`). Any route with **no**
explicit mapping falls through to the `/admin-api/*` **default-deny** and
requires full `admin` (fail-closed, introduced in #3040). This document is the
source of truth for what each route requires and why; the validator mirrors it.

## Decision categories

| Tag | Meaning | Reachable by |
|-----|---------|--------------|
| **APP** | An app acting on resources it owns (contexts, namespaces, groups it created, aliases, blobs, its app/packages). Mapped to the matching `context:*` / `application:*` / `blob:*` / `package:*` permission. | a normal client token (`context:create/list/execute`, etc.) |
| **GOV** | Governance *over other members* — adding/removing members, changing roles/capabilities, group settings. Mapped to `context:capabilities` (grant/revoke). | a token explicitly granted capability authority — **not** a plain client token, **not** admin |
| **ADMIN** | Node-owner / security-critical: node observability, dev tooling that reads the node FS, TEE admission, cryptographic key/proof issuance, specialized-node governance. | `admin` only |
| **PUBLIC** | Liveness / token self-check. No permission required. | any caller (health/ready) / any valid token (is-authed) |

Scope: reads/creates are scoped `Specific([id])` where an id is in the path; an
unscoped (`Global`) `context:*` token still satisfies a `Specific` requirement
(`Global` matches any), so client tokens work without per-resource narrowing,
and a narrowed token is still possible.

---

## Applications

| Route | Method | Decision | Permission |
|-------|--------|----------|------------|
| `/admin-api/applications` | GET | APP | `Application::List(Global)` |
| `/admin-api/applications/:id` | GET | APP | `Application::List([id])` |
| `/admin-api/applications/:id` | DELETE | APP | `Application::Uninstall([id])` |
| `/admin-api/applications/:id/versions` | GET | APP | `Application::List([id])` |
| `/admin-api/install-application` | POST | APP | `Application::Install(Global)` |
| `/admin-api/install-dev-application` | POST | **ADMIN** | reads an arbitrary **node-local filesystem path** — a node-owner operation, not something a remote app token should drive |

## Packages

| Route | Method | Decision | Permission |
|-------|--------|----------|------------|
| `/admin-api/packages` | GET | APP | `Package::ListPackages(Global)` |
| `/admin-api/packages/:p/versions` | GET | APP | `Package::ListVersions([p])` |
| `/admin-api/packages/:p/latest` | GET | APP | `Package::GetLatestVersion([p])` |

## Contexts

| Route | Method | Decision | Permission |
|-------|--------|----------|------------|
| `/admin-api/contexts` | GET | APP | `Context::List(Global)` |
| `/admin-api/contexts` | POST | APP | `Context::Create(Global)` |
| `/admin-api/contexts/:id` | GET | APP | `Context::List([id])` |
| `/admin-api/contexts/:id` | DELETE | APP | `Context::Delete([id])` |
| `/admin-api/contexts/:id/application` | POST | APP | `Context::Application(Update([id]))` |
| `/admin-api/contexts/:id/join` | POST | APP | `Context::Create([id])` — join = establish local membership |
| `/admin-api/contexts/:id/leave` | POST | APP | `Context::Leave([id], Any)` |
| `/admin-api/contexts/:id/identities` | GET | APP | `Context::List([id])` |
| `/admin-api/contexts/:id/identities-owned` | GET | APP | `Context::List([id])` |
| `/admin-api/contexts/:id/group` | GET | APP | `Context::List([id])` |
| `/admin-api/contexts/:id/storage` | GET | APP | `Context::List([id])` |
| `/admin-api/contexts/:id/resync` | POST | APP | `Context::List([id])` — refresh local replica |
| `/admin-api/contexts/sync`, `/sync/:id` | POST | APP | `Context::List(Global/[id])` |
| `/admin-api/contexts/for-application/:id` | GET | APP | `Context::List([app])` |
| `/admin-api/contexts/with-executors/for-application/:id` | GET | APP | `Context::List([app])` |
| `/admin-api/contexts/:id/capabilities/(grant\|revoke)` | POST | GOV | `Context::Capabilities(Grant/Revoke([id]))` |
| `/admin-api/identity/context` | POST | APP | `Context::Create(Global)` — generate a context identity (prereq to join) |
| `/admin-api/contexts/invite-specialized-node` | POST | **ADMIN** | invites specialized/TEE infrastructure into a context — node-level governance |

## Aliases (friendly names — all APP)

`create`/`delete` → `Alias::Create/Delete`, `lookup`/`list` → `Alias::Lookup/List`,
over `AliasType::{Context,Application,Identity}`. Aliases are app-level naming
with no authority beyond resolving a name; there is no reason for them to be
admin-only.

| Route family | Method | Decision | Permission |
|-------|--------|----------|------------|
| `/admin-api/alias/create/{context,application,identity/:ctx}` | POST | APP | `Context::Alias(Create(<type>, scope))` |
| `/admin-api/alias/lookup/{...}/:name` | POST | APP | `Context::Alias(Lookup(<type>, scope))` |
| `/admin-api/alias/delete/{...}/:name` | POST | APP | `Context::Alias(Delete(<type>, scope))` |
| `/admin-api/alias/list/{...}` | GET | APP | `Context::Alias(List(<type>, scope))` |

## Namespaces (shipped earlier in this PR)

| Route | Method | Decision | Permission |
|-------|--------|----------|------------|
| `/admin-api/namespaces` | GET / POST | APP | `Context::List` / `Context::Create` |
| `/admin-api/namespaces/for-application/:id` | GET | APP | `Context::List([app])` |
| `/admin-api/namespaces/:id` | GET / DELETE | APP | `Context::List([ns])` / `Context::Delete([ns])` |
| `/admin-api/namespaces/:id/groups` | GET / POST | APP | `Context::List([ns])` / `Context::Create([ns])` |
| `/admin-api/namespaces/:id/identity` | GET | APP | `Context::List([ns])` |
| `/admin-api/namespaces/:id/join` | POST | APP | `Context::Create([ns])` |
| `/admin-api/namespaces/:id/invite` | POST | GOV | `Context::Invite([ns], Any)` |
| `/admin-api/namespaces/:id/leave` | POST | APP | `Context::Leave([ns], Any)` |

## Groups

The group cluster splits three ways: app-level resource ops, governance over
other members, and security-critical operations.

### App-level (client token)

| Route | Method | Decision | Permission |
|-------|--------|----------|------------|
| `/admin-api/groups` | POST | APP | `Context::Create(Global)` |
| `/admin-api/groups/join` | POST | APP | `Context::Create(Global)` |
| `/admin-api/groups/:id` | GET | APP | `Context::List([id])` |
| `/admin-api/groups/:id` | DELETE | APP | `Context::Delete([id])` |
| `/admin-api/groups/:id/leave` | POST | APP | `Context::Leave([id], Any)` |
| `/admin-api/groups/:id/{contexts,subgroups,members,metadata}` | GET | APP | `Context::List([id])` |
| `/admin-api/groups/:id/members/:id/{metadata,capabilities}` | GET | APP | `Context::List([id])` |
| `/admin-api/groups/:id/contexts/:id/metadata` | GET | APP | `Context::List([id])` |
| `/admin-api/groups/:id/upgrade`, `/upgrade/retry` | POST | APP | `Context::Application(Update([id]))` — app-version management |
| `/admin-api/groups/:id/upgrade/status` | GET | APP | `Context::List([id])` |
| `/admin-api/groups/:ns/{cascade-status,migration-status}` | GET | APP | `Context::List([ns])` |
| `/admin-api/groups/:id/invite` | POST | GOV | `Context::Invite([id], Any)` |

### Governance over other members (capability-gated → `context:capabilities`)

Mutating another member's presence, role, capabilities, or the group's shared
settings is authority over others, not self-service. It maps to
`Context::Capabilities(Grant/Revoke)` — reachable by a token explicitly granted
capability authority, **not** a plain client token, but **not** admin either.

| Route | Method | Permission |
|-------|--------|------------|
| `/admin-api/groups/:id` (settings) | PATCH | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/members` (add) | POST | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/members/remove` | POST | `Capabilities(Revoke([id]))` |
| `/admin-api/groups/:id/members/:id/role` | PUT | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/members/:id/capabilities` | PUT | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/members/:id/auto-follow` | PUT | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/reparent` | POST | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/settings/default-capabilities` | PUT | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/settings/subgroup-visibility` | PUT | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/{metadata,members/:id/metadata,contexts/:id/metadata}` | PUT | `Capabilities(Grant([id]))` |
| `/admin-api/groups/:id/contexts/:id/remove` (detach) | POST | `Capabilities(Revoke([id]))` |
| `/admin-api/groups/:ns/migration/abort` | POST | `Capabilities(Grant([ns]))` |

### Security-critical (ADMIN)

| Route | Method | Why admin |
|-------|--------|-----------|
| `/admin-api/groups/:id/settings/tee-admission-policy` | GET/PUT | controls who may join as a TEE replica — attestation trust boundary |
| `/admin-api/groups/:id/signing-key` | POST | registers a cryptographic signing key |
| `/admin-api/groups/:id/issue-ownership-proof` | POST | mints an ownership proof — impersonation risk |
| `/admin-api/groups/:id/issue-namespace-ownership-proof` | POST | mints a namespace ownership proof |

## Blobs

| Route | Method | Decision | Permission |
|-------|--------|----------|------------|
| `/blobs/stream`, `/blobs/file`, `/blobs/url` | POST | APP | `Blob::Add(Stream/File/Url)` |
| `/admin-api/blobs` | PUT | APP | `Blob::Add(...)` |
| `/admin-api/blobs` | GET (list) | APP | `Blob::List` *(new variant — see below)* |
| `/admin-api/blobs/:id` | GET (download) | APP | `Blob::List([id])` |
| `/admin-api/blobs/:id` | DELETE | APP | `Blob::Remove([id])` |

`BlobPermission` has only `Add`/`Remove`; reading (list/download) has no
variant, so those reads are currently admin-only. Adding a `Blob::List`
variant is the clean fix and is part of this PR.

## TEE

| Route | Method | Decision | Why |
|-------|--------|----------|-----|
| `/admin-api/tee/info` | GET | **ADMIN** | node TEE posture |
| `/admin-api/tee/attest` | POST | **ADMIN** | produces an attestation quote |
| `/admin-api/tee/verify-quote` | POST | **ADMIN** | attestation verification |
| `/admin-api/tee/fleet-join` | POST | **ADMIN** | a TEE node joining the fleet — infrastructure/governance |

## Node observability & liveness

| Route | Method | Decision | Why |
|-------|--------|----------|-----|
| `/admin-api/usage` | GET | **ADMIN** | node resource usage |
| `/admin-api/network/status` | GET | **ADMIN** | node network posture |
| `/admin-api/peers` | GET | **ADMIN** | node peer set |
| `/admin-api/certificate` | GET | **ADMIN** | node certificate |
| `/admin-api/health`, `/ready` | GET | **PUBLIC** | liveness — no auth |
| `/admin-api/is-authed` | GET | **PUBLIC** | token self-check — any valid token, no permission |

## Keys / auth (`/admin/*`, not `/admin-api/*`)

Unchanged by this document. `/admin/keys*` remain key-management perms;
`PUT /admin/keys/:id/permissions` requires `admin` (privilege-escalation
guard, #3040).

---

## Invariants

- **Fail-closed preserved.** Genuinely unmapped `/admin-api/*` routes (a new
  subpath, a mapped path with an unhandled method) still require `admin`.
- **Least privilege.** Destructive/governance/security ops are never reachable
  by a plain client token: deletion needs `*:delete`, member governance needs
  `context:capabilities`, security ops need `admin`.
- **No frontend change.** APP routes map onto permissions a client token
  already holds (`context:create/list`, …), so existing tokens work with no
  re-login or new scopes.
