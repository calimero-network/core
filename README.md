# @calimero-network/mero-react

**Your app's whole backend, like Firebase. Except nobody can read your data, not even us.** [Calimero](https://calimero-network.github.io/core/) gives an app auth (passkeys, sessions, scoped API keys), a database (replicated app state), functions (Rust app methods), realtime events and presence, sealed blob storage, hosting for its frontend and typed clients, all running inside attested Intel TDX hardware. Clients verify a node's attestation against Sigstore-signed releases before they send it anything, then talk to it over a sealed Noise channel.

**mero-react** is the React layer over [mero-js](https://github.com/calimero-network/mero-js): a `<MeroProvider>` that holds one mero-js `CalimeroClient` (and its generated method groups `client.apps`, `client.orgs`, `client.auth`, ...), plus hooks and components on top of it:

- passkey sign-in and sign-up (`ConnectButton`, `LoginModal`);
- sessions that refresh by themselves;
- calls, views and live events on an app instance;
- organizations, teams and invites;
- the node's consent page for approvals;
- blobs and presence;
- passkeys and API keys for the signed-in account;
- typed errors your UI can switch on.

> **Upgrading from 10.x?** 11.0 is a rewrite for core 1.0, which removed `/admin-api`, `/jsonrpc`, `/ws`, `/sse`, mero-auth and `/auth/token`. Read [Migrating from 10.x](#migrating-from-10x) below.

> **Before committing, titling a pull request, or releasing**, read [RELEASING.md](RELEASING.md): the PR title is the release commit, and the rules are shared by every Calimero repository that ships releases.

## Installation

```bash
pnpm add @calimero-network/mero-react
```

`@calimero-network/mero-js` (^35) comes with it, and every mero-js export is re-exported, so apps need only this import. React 18 or 19 is a peer dependency.

## Quick start

```tsx
import {
  MeroProvider,
  ConnectButton,
  useMero,
  useView,
  useExecute,
  useSubscription,
} from '@calimero-network/mero-react';

export function Root() {
  return (
    // A frontend the node serves at {label}.{apps-domain}:
    <MeroProvider servedByNode minReleaseVersion="2.3.109" appId={APP_ID}>
      <App />
    </MeroProvider>
  );
}

function App() {
  const { isAuthenticated } = useMero();
  const items = useView<string[]>('list_items');
  const { execute, failure } = useExecute();
  useSubscription(undefined, (event) => {
    if (event.kind === 'ItemAdded') void items.refetch();
  });

  if (!isAuthenticated) return <ConnectButton appName="Notes" />;
  return (
    <>
      <ul>{items.data?.map((item) => <li key={item}>{item}</li>)}</ul>
      <button onClick={() => execute('add_item', { text: 'hello' })}>Add</button>
      {failure && <p role="alert">{failure.message}</p>}
    </>
  );
}
```

A frontend hosted elsewhere reaches a Calimero Cloud cluster by name, failing over between its nodes:

```tsx
<MeroProvider cluster="eu-shared-1" minReleaseVersion="2.3.109" appId={APP_ID}>
```

For local development against `merod local` (three `--dev` nodes with mock attestation on 127.0.0.1:2501-2503):

```tsx
<MeroProvider
  nodeUrl="http://127.0.0.1:2501"
  nodes={['http://127.0.0.1:2502', 'http://127.0.0.1:2503']}
  allowMock
>
```

`allowMock` accepts mock attestation quotes. Never set it from anything a visitor controls, such as a URL parameter or the environment of a deployed build.

## `<MeroProvider>`

| Prop | Type | What it does |
| --- | --- | --- |
| `minReleaseVersion` | `string` | The oldest mero-tee release to trust, for example `2.3.109`. This is shorthand for `policy: { release: { minReleaseVersion } }`. |
| `policy` | `NodePolicy` | The full attestation policy (profile, release source, `verifyQuote`, `appHash`). Takes precedence over `minReleaseVersion`. |
| `allowMock` | `boolean` | Accept mock quotes (`merod local`, `--dev` clusters). The provider throws if `minReleaseVersion`, `policy` and `allowMock` are all missing. |
| `servedByNode` | `boolean` | The node serves this page. The node is then `window.location.origin`, and a passkey ceremony here opens a session bound to this app's package. `ConnectButton` skips the node field. |
| `nodeUrl` | `string` | The node to use when the page is not node-served. Without it (and without `cluster`), the provider uses the node or cluster kept from the last visit, or waits for `connectToNode` / `connectToCluster`. |
| `nodes` | `string[]` | Other nodes of the cluster, with `nodeUrl`. On `421 WRONG_NODE`, or when a node stays unavailable, the client moves to the next one. |
| `cluster` | `string` | A Calimero Cloud cluster by name (`eu-shared-1`), in place of `nodeUrl`. The client reads the cluster's nodes from the public node directory (mero-js `CalimeroClient.connect`) and fails over between them. The directory only picks which nodes to try: each is attested under your policy first. `nodeUrl` wins when both are given. |
| `zone`, `directory` | `string` | With `cluster`: the zone whose nodes are tried first, and another node directory than mero-js `DEFAULT_DIRECTORY`. |
| `appId` | `string` | The app instance that hooks default to. |
| `sessionStore` | `SessionStore` | Where the session is kept. The default is `localStorageSessionStore`; `memorySessionStore()` keeps it for the tab's lifetime only. |
| `passkey` | `PasskeyProvider` | The WebAuthn prompt. The default is mero-js `browserPasskey()`. |
| `fetch`, `retry` | | Passed through to `CalimeroClient`. |
| `client` | `CalimeroClient` | Use this client instead of building one, for tests or custom setups. |
| `onError` | `(failure) => void` | Called with every failure that a hook or a sign-in reports. |

### `useMero()`

```ts
const {
  client,            // CalimeroClient | null (null until a node is known, or a cluster's nodes are)
  nodeUrl,           // the node in use
  cluster,           // the cluster connected to by name, or null
  isConnecting,      // the node directory is being read
  connectToNode,     // (url) => void
  connectToCluster,  // (name) => void
  disconnect,
  session, accountId, isAuthenticated, isLoading,
  signIn,            // () => Promise<CalimeroSession | null>: an existing passkey
  signUp,            // (name) => Promise<CalimeroSession | null>: a new account and passkey
  signOut, signOutEverywhere,
  appId, setAppId,
  passkey, servedByNode, allowMock,
  failure,           // the last sign-in, connection or session failure, described
  reportError,
} = useMero();
```

**Sessions.** A 1.0 session is a session token (`v2.…`) that lasts 15 minutes, plus a refresh token (`rt_…`) that lasts 30 days and can be used once.

- The client refreshes by itself on a `401`. One refresh is shared by every request waiting on it, because reusing a refresh token revokes the session. When the node that issued the session is gone, the refresh goes to another node of the cluster (mero-js#318).
- The provider saves every new session to `sessionStore`, so a reload resumes the session.
- When a refresh is refused, hooks get `LoginRequired`, and the provider ends the session and sets `failure.kind = 'login'`.
- `mero` is still on the value as a deprecated alias of `client`.

## Hooks

Every read hook returns `{ data, loading, error, failure, refetch }`. Every action hook returns its action plus `{ loading, error, failure }`.

- `error` is the error exactly as the client threw it, so `instanceof AppError` still works.
- `failure` is `describeError(error)`; see [Errors](#errors).
- An action resolves `null` when it fails. Pass `{ throwOnError: true }` to make it throw instead.

### App: calls, views, events

| Hook | Returns |
| --- | --- |
| `useApp(appId?)` | `{ appId, app, call, view, loading, error, failure, refetch }`. `call` and `view` are bound to the app and throw. |
| `useExecute(appId?, opts?)` | `{ execute, call, loading, error, failure }`. `execute(method, args?, { idempotencyKey?, confirm? })` resolves the method's result; `call` resolves the whole `CallResult` (`index`, `events`). `confirm: true` answers `412 CONFIRM_REQUIRED` with the passkey (`callConfirmed`). |
| `useView<T>(method, args?, { appId?, stale?, minIndex?, enabled? })` | `{ data: T, index, … }`. It reads again when the method or the arguments change. |
| `useSubscription(appIds?, callback, opts?)` | Calls `callback` with each `AppEvent` (`{ appId, id, index, seq, kind, data }`). |
| `useAppAbi(appId?)` | The app's ABI. |

`appId` defaults to the provider's `appId`.

How `useSubscription` behaves:

- Each app gets one SSE stream with an `AbortController`. Unmounting, or changing the app ids, aborts it.
- A stream that ends or fails reopens with backoff from the last event it saw. Events at or before that one are skipped, so nothing is delivered twice.
- `from` defaults to `'live'`. Pass a log index to replay from it (`0` replays the whole history).
- `onOpen` is called each time a stream opens. Refetch views there to cover a gap.

### Apps

| Hook | Was (10.x) |
| --- | --- |
| `useApps({ package?, bundle? })` | `useContexts` |
| `useCreateApp().create({ name, bundle, org?, access?, confirmMethods?, initArgs? })` | `useCreateContext` |
| `useDeleteApp().deleteApp(appId)` | `useDeleteContext` |
| `useSetAppMetadata().setMetadata(appId, metadata)` | `useSetContextMetadata` |
| `useUpgradeApp(appId?).upgrade({ version, … })` | `useUpgradeGroup`. It asks for the passkey. |
| `usePackages()` | none (new) |

### Organizations and teams

| Hook | Was (10.x) |
| --- | --- |
| `useOrg(orgId)` | `useNamespace` |
| `useCreateOrg().create(name)` | `useCreateNamespace` |
| `useDeleteOrg().deleteOrg(orgId)` | `useDeleteNamespace`. It asks for the passkey. |
| `useSetMember().setMember(org, account, role)` | `useUpdateMemberRole`, and `useAddGroupMembers` at org level. It asks for the passkey. |
| `useRemoveMember().removeMember(org, account)` | none (new). It asks for the passkey. |
| `useSetOrgMetadata()`, `useSetMemberMetadata()` | `useSetGroupMetadata`, `useSetMemberMetadata` |
| `useSetCapabilities().setCapabilities(org, bits, { account?, team? })` | `useSetDefaultCapabilities`. It asks for the passkey. |
| `useTeam(org, team)` | `useGroupInfo` |
| `useCreateTeam().create({ orgId, name, join?, parent? })` | `useCreateGroupInNamespace` |
| `useUpdateTeam().update(org, team, { name?, join?, parent?, metadata? })` | `useSetGroupMetadata`, `useReparentGroup`, `useSetSubgroupVisibility` |
| `useDeleteTeam()`, `useSetTeamMember()`, `useRemoveTeamMember()` | `useDeleteGroup`, `useAddGroupMembers`, `useRemoveGroupMembers`. Setting and removing a team member ask for the passkey; deleting a team asks for it when the team (or one below it) holds apps. |

**Passkey step-ups.** Since mero-js 28 the node approves membership changes, capabilities, invites, passkeys and API keys (creating and revoking) with the caller's passkey: it answers `412 CONFIRM_REQUIRED` and the hook answers that with the provider's `passkey` (`browserPasskey()` by default, through mero-js `passkeyConfirm`), so the person sees a passkey prompt. A dismissed prompt fails the action with `failure.kind = 'cancelled'`.

### Invites

| Hook | Notes |
| --- | --- |
| `useCreateInvite().create({ orgId, role?, teams?, ttlSecs?, uses?, linkBase? })` | Resolves `{ token, link }`. It asks for the passkey. The link carries the token in the fragment (`#invite=…`), so it stays out of server logs. |
| `useRedeemInvite().redeem(token)` | `POST /invites/{token}/redeem`. No passkey needed. |
| `useInviteRedemption({ autoRedeem? })` | The whole link flow. It reads the token from the address bar and removes it there, keeps it through sign-in, and redeems it once the person is signed in. `status` is `idle`, `waiting-for-sign-in`, `redeeming`, `redeemed` or `failed`. |

`inviteLink(token, base?)` and `readInviteToken(href)` are exported too.

### Consent page

Approvals happen only on the page the node serves on its own origin, never inside an app. A session opened on an app's frontend cannot approve anything.

```ts
const consent = useConsent();
await consent.open(upgradeConsent(appId, '1.2.0', window.location.href), {
  // Show the confirmation code before leaving; the node's page shows the same one.
  confirm: (started) => window.confirm(`The approval page will show the code ${started.code}. Continue?`),
}); // then navigates to the node's page
// … back on returnTo:
if (consent.returned) await consent.check(consent.returned.requestId);    // confirm the hint
// or, when the page was opened in another window:
await consent.wait(started.requestId);
```

`useConsent()` returns `{ request, open, check, wait, cancel, started, code, result, returned, … }`. `code` is the confirmation code (`XXX-XXX`) the consent page shows for the request under way: show it next to the link or in `open`'s `confirm`, so the person can check they landed on the page their request opened. `open` throws `CONSENT_CODE_MISMATCH` when the node's code does not match the request (mero-js `confirmationCode`). Build requests with mero-js `releaseConsent`, `withdrawConsent` and `upgradeConsent`.

### Blobs, presence, account

| Hook | Notes |
| --- | --- |
| `useUploadBlob({ appId? }).upload(bytes \| Blob \| File)` | Resolves `{ blobId, id, size }`. Blobs now belong to an app instance. |
| `useBlobInfo(blobId, { appId? })`, `useBlobs(appId?)` | |
| `useBlobUrl(blobId, { appId?, mimeType? })` | `{ url }` is an object URL. The bytes come over the sealed channel, and the URL is revoked on unmount. |
| `useDeleteBlob({ appId? }).deleteBlob(blobId)` | |
| `useEphemeral<T>(appId, { codec?, initial?, throttleMs?, includeSelf? })` | Presence. Returns `{ peers: Map<account, T>, entries, setPresence, leave, error }`. It sends a heartbeat every 5 s and retracts on unmount. |
| `useAccount()` | `GET /auth/me`. Was `useNodeIdentity`. |
| `usePasskeys()` | `{ add, remove, label }` for the account's passkeys. Adding and removing are approved with a passkey the account already has; `add({ passkey?, approver? })` takes other providers for the new passkey and the approving one. |
| `useApiKeys()` | `{ keys, create, revoke, refetch, … }` for API keys (agents, servers). `create({ label?, apps?, methods?, readOnly?, allowConfirm?, ttlSecs? })` asks for the passkey, needs a passkey session, and resolves `{ keyId, key }`: the key is shown once. |

## Errors

mero-js maps every refusal to a typed error (`errorFrom`, mero-js#312). mero-react keeps that error in `error` and adds `failure = describeError(error)`:

```ts
type Failure = {
  kind: FailureKind;          // see the table
  message: string;            // a sentence you can show as it is
  retryable: boolean;
  status?: number; code?: string;
  appCode?: string; fields?: Record<string, unknown>;   // kind 'app'
  committed?: false | 'unknown';                         // kind 'unavailable'
  release?: string;                                      // kind 'release-deadline'
  error: unknown;
};
```

| `kind` | From |
| --- | --- |
| `app` | `AppError` (`422 APP_ERROR`): the app's own error, with `appCode` and named `fields` |
| `trap` | `TrapError` (`422 TRAP`) |
| `login` | `LoginRequired`: the refresh was refused. The provider ends the session. |
| `confirm` / `cancelled` | `ConfirmRequired` (`412`) / `ConfirmCancelled` (the person dismissed the passkey prompt) |
| `wrong-node` | `WrongNode` / `421 NOT_LEADER` after failover ran out |
| `unavailable` | `UnavailableError` (`503` or no answer after retries). Check `committed`. |
| `not-found`, `forbidden`, `too-large`, `idempotency`, `rate-limited` | `404`, `403`, `413`, `409 IDEMPOTENCY_MISMATCH`, `429` |
| `release-deadline`, `consent-stale`, `operator-signature` | `409 RELEASE_DEADLINE`, `409 CONSENT_STALE`, `428 OPERATOR_SIGNATURE_REQUIRED` |
| `attestation` | The sealed channel or the attestation check failed |
| `directory` | `DirectoryError`: the node directory does not know the cluster (`code: 'not-found'`), lists no usable node (`empty`) or could not be read |

## Components

- `<ConnectButton appName? allowSignUp? initialTab? theme? logoOnly? label? />`. Signed out, it shows "Sign in" and opens `LoginModal`. Signed in, it shows the account, with a menu for Sign out and Sign out everywhere.
- `<LoginModal isOpen onClose onSignedIn? initialTab? allowSignUp? appName? theme? />`. Sign in with a passkey, or create an account, which asks for a name. Off a node-served page it asks first for the node URL or a cluster name.

### Theming

Pass a partial `MeroTheme` (`primary`, `primaryHover`, `primaryText`, `background`, `backgroundSecondary`, `backgroundTertiary`, `border`, `text`, `textSecondary`, `error`, `overlay`, `radius`), or set the `--mero-*` CSS variables globally:

```css
:root {
  --mero-accent: #ff4081;
  --mero-accent-hover: #e91e63;
}
```

The dialog's classes are `mero-dialog-overlay`, `mero-dialog-panel`, `mero-dialog-title`, `mero-dialog-body`, `mero-dialog-field`, `mero-dialog-input`, `mero-dialog-button`, `mero-dialog-primary`, `mero-dialog-secondary` and `mero-dialog-link`. The button's classes are `mero-connect-button`, `mero-dropdown` and `mero-dropdown-item`.

## Node shield

A header chip that shows what this browser verified about the node the client talks to, with a popover of the details and a link to the public verifier.
It is a separate entry point, so apps that do not use it do not ship it, and it needs no CSS import.

```tsx
import { NodeShield } from '@calimero-network/mero-react/node-shield';

<header>
  <h1>My app</h1>
  <NodeShield />
</header>;
```

Inside a `<MeroProvider>` it checks the provider's client; elsewhere pass one with `client={client}`.
It renders nothing until there is a client.

| State | Chip |
| --- | --- |
| The check is running, or the node could not be reached yet | "Checking node…" |
| A real TEE quote with the `locked-read-only` profile | "Verified node" (green) |
| A mock quote, or a real one with another profile | "Test server: not sealed" (grey) |
| The check failed; the popover shows the reason | "Node refused" (red) |

Props, all optional:

- `client`: the mero-js `CalimeroClient` to check; by default the provider's. `client={null}` shows nothing.
- `cluster`: the label shown as "Cluster"; by default the provider's cluster. `cluster={null}` leaves the row out.
- `compactBelow`: the window width in px at or below which the chip is the icon alone. It is always the icon alone at 560px or narrower.
- `appCode`: `(client, url) => Promise<AppCodeCheck>`, the app's own check that node `url` runs its code. It is asked when the popover opens on a real, verified node, and its `match` or `mismatch` is shown as the "App code" row. A new function checks again, so memoise it (`useCallback`).
- `colors`: colours for this shield (below).

`AppCodeCheck` is the shape an app's own code check returns:

```ts
type AppCodeCheck =
  | { status: 'match'; appHash: string; checkedAt: number; package?: string; version?: string }
  | { status: 'mismatch'; appHash: string; expected: string }
  | { status: 'unknown'; reason: string };
```

`unknown`, or a check that throws, shows no "App code" row.

### Colours

The shield reads these CSS custom properties, with light and dark defaults that follow `prefers-color-scheme`: `--node-shield-verified`, `--node-shield-refused`, `--node-shield-checking`, `--node-shield-neutral` (the grey state), `--node-shield-surface`, `--node-shield-text`, `--node-shield-muted`, `--node-shield-border` and `--node-shield-focus`.
Set them in CSS, on `:root` or on any ancestor of the shield:

```css
.app-header {
  --node-shield-verified: #0f9d58;
  --node-shield-surface: #10131c;
  --node-shield-text: #f2f4f8;
}
```

Or pass the same keys, without the prefix, as the `colors` prop:

```tsx
<NodeShield colors={{ verified: '#0f9d58', surface: '#10131c', text: '#f2f4f8' }} />
```

The popover renders in `document.body` and takes the colours the chip resolved, so both ways reach it.
Its classes start with `mero-node-shield`.

## Storage

- `localStorageSessionStore` and `memorySessionStore()`: session stores.
- `getNodeUrl` / `setNodeUrl` / `clearNodeUrl`, `getCluster` / `setCluster` / `clearCluster` and `getAppId` / `setAppId` / `clearAppId`.
- `clearAllStorage()` clears all of the above.
- Keys: `mero:session`, `mero:nodeUrl`, `mero:cluster`, `mero:appId`. 10.x tokens are not read.

## Migrating from 10.x

core 1.0 is a different node, and mero-js 28 is a different client, so 11.0 is a breaking release. Most app ports are mechanical:

1. **Provider.**
   - Drop `mode`, `packageName`, `packageVersion`, `registryUrl`, `cloudBaseUrl`, `timeoutMs`, `allowedNodeUrls` and `tokenStore`.
   - Add `minReleaseVersion` (production) or `allowMock` (`merod local`).
   - Keep `servedByNode`. Pass `appId` if the app works on one instance.
2. **Client.**
   - `useMero().mero` (a `MeroJs` or relay client) is now `useMero().client` (a `CalimeroClient`).
   - `mero.rpc.execute({ contextId, method, argsJson, executorPublicKey })` is now `client.apps.call({ app, method, args })`, or `client.apps.view({ app, method, args })` for reads (or `useApp(appId).call` / `.view`).
3. **Vocabulary.**
   - context is now app instance (`appId`);
   - namespace is now organization;
   - group is now team;
   - executor / node identity is now the signed-in account.
4. **Auth.**
   - There is no redirect, callback or `access_token` in the URL any more.
   - `connectToNode(url)` only picks the node. Signing in is `signIn()` or `signUp(name)` (passkeys), or `<ConnectButton />`.
5. **Errors.** Switch on `failure.kind`, or `instanceof AppError` / `ConfirmRequired` / `WrongNode`, instead of HTTP status checks.

| 10.x | 11.0 |
| --- | --- |
| `MeroProvider mode={AppMode.X}` | `MeroProvider minReleaseVersion=… \| allowMock` (no modes, no permission scopes) |
| `useMero().mero` | `useMero().client` (`mero` is kept as a deprecated alias) |
| `useMero().contextId` / `contextIdentity` / `applicationId` | `useMero().appId` / `accountId` / none |
| `useMero().isOnline` | none: the client retries and fails over by itself |
| `connectToNode(url)` + redirect login | `connectToNode(url)` + `signIn()` / `signUp(name)` |
| `connectWithAccount`, `isDelegated`, `can`, `admin` | none (no relays, no delegated accounts) |
| `useExecute(contextId, executorId).execute(m, p)` | `useExecute(appId).execute(m, args, { confirm?, idempotencyKey? })` |
| none | `useView(method, args)`, `useApp(appId)` |
| `useSubscription(contextIds, cb)` / `{ contextIds, groupIds }` | `useSubscription(appIds, cb, { from?, onOpen? })` |
| `useContexts`, `useApplicationContexts`, `useNamespacesForApplication` | `useApps({ package? })` |
| `useCreateContext`, `useCreatePrivateContext` | `useCreateApp` (`access` for private apps) |
| `useDeleteContext`, `useSetContextMetadata` | `useDeleteApp`, `useSetAppMetadata` |
| `useNamespace`, `useNamespaces`, `useCreateNamespace`, `useDeleteNamespace` | `useOrg`, `useAccount` (orgs), `useCreateOrg`, `useDeleteOrg` |
| `useGroupInfo`, `useNamespaceGroups`, `useSubgroups`, `useContextGroup` | `useTeam` (teams are listed in `useOrg`) |
| `useCreateGroupInNamespace`, `useDeleteGroup` | `useCreateTeam`, `useDeleteTeam` |
| `useAddGroupMembers`, `useRemoveGroupMembers`, `useGroupMembers` | `useSetTeamMember`, `useRemoveTeamMember`, `useTeam` |
| `useUpdateMemberRole`, `useGroupCapabilities`, `useSetDefaultCapabilities` | `useSetMember`, `useOrg`, `useSetCapabilities` |
| `useSetGroupMetadata`, `useGroupMetadata`, `useMemberMetadata` | `useSetOrgMetadata` / `useUpdateTeam`, `useOrg` / `useTeam` |
| `useReparentGroup`, `useSetSubgroupVisibility` | `useUpdateTeam({ parent, join })` |
| `useCreateNamespaceInvitation`, `useGroupInvitations` | `useCreateInvite` |
| `useRedeemInvitation`, `useJoinNamespace`, `useJoinGroup`, `useJoinContext` | `useRedeemInvite` |
| `useInviteRedemption`, `useJoinInvitation` | `useInviteRedemption` (no relay or sync step) |
| `useUpgradeGroup`, `useInstallFromRegistry` | `useUpgradeApp`, or `useConsent` + `upgradeConsent` from an app frontend |
| `useGroupUpgradeStatus`, `useRetryGroupUpgrade` | mero-js `client.orgs.upgradeStatus({ org, run })` / `client.orgs.upgradeRetry({ org, run }, { confirm })` (org upgrade runs) |
| `useUploadBlob`, `useBlobInfo`, `useBlobUrl` | Same names, now per app (`{ appId }`) |
| `useEphemeral(contextId, opts)` | `useEphemeral(appId, opts)`: peers keyed by account |
| `useNodeIdentity` | `useAccount` |
| `classifyError(err)` / `ClassifiedError` | `describeError(err)` / `Failure` |
| `getPermissionsForMode`, `AppMode`, `ConnectionType` | removed (no permission scopes) |
| `localStorageTokenStore`, `get/setContextId`, `get/setApplicationId`, `get/setContextIdentity` | `localStorageSessionStore`, `get/setAppId` |

**Removed with no replacement** (v1 or desktop only):

- admin-API hooks: `useSyncGroup`, `useResyncContext`, `useJoinSubgroupInheritance`, `useDetachContextFromGroup`, `useSetTeeAdmissionPolicy`, `useContextDiscovery`;
- migration hooks and components: `useMigrationStatus`, `useMigrationEvents`, `useMyAuthoredMigration`, `MigrationPendingBanner`, `MigrationAdminPanel`;
- version hooks: `useAppVersion`, `useLatestVersion`, `useGroupAppVersion`;
- the delegated / relay / account-enrolment layer: `useAccountEnrolment`, `useDelegatedBootstrap`, `ConnectButtonAccount`, `AccountSignInPanel`, and the relay, `joinWithNode` and `bootstrapFromInvitation` re-exports;
- invitation UI: `InvitationGate`, `InvitationDialog`, `JoinSyncBanner`, `useJoinSync`;
- local-node discovery: `discoverLocalNodes`;
- desktop (Tauri): `isDesktopWindow`.

## Related repositories

- [core](https://github.com/calimero-network/core): the node (`merod`), the Rust SDK for apps and `meroctl`; [docs](https://calimero-network.github.io/core/).
- [mero-tee](https://github.com/calimero-network/mero-tee): the TDX images nodes boot and the key service; its releases publish the measurements clients check attestation against (`minReleaseVersion`).
- [app-registry](https://github.com/calimero-network/app-registry): where signed app packages are published and clusters install them from.
- [mero-js](https://github.com/calimero-network/mero-js): the JavaScript/TypeScript client this package wraps and re-exports.
- [calimero-client-py](https://github.com/calimero-network/calimero-client-py), [swift-sdk](https://github.com/calimero-network/swift-sdk), [kotlin-sdk](https://github.com/calimero-network/kotlin-sdk): the Python, Swift and Android (Kotlin) clients.
- [apps](https://github.com/calimero-network/apps): example apps.

## License

MIT
