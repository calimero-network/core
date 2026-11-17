# Mero.js — the JavaScript client for Calimero 1.0

Your app's whole backend, like Firebase. Except nobody can read your data, not even us.
[Calimero](https://calimero-network.github.io/core/) gives an app auth (passkeys,
sessions, scoped API keys), a database (replicated app state), functions (Rust app
methods, compute jobs, timers, pipelines), realtime (events, presence), sealed file
and blob storage, search, hosting for its frontend and typed clients, all running
inside attested Intel TDX hardware. Clients check a node's attestation against
Sigstore-signed releases before they send it anything, then talk to it over a sealed
Noise channel.

This package is the JavaScript/TypeScript client, for the browser and Node 18+.
`CalimeroClient` verifies the node's TDX attestation, seals every request to it end
to end (Noise NK, `calimero/sealed-http/v2`), signs people in with WebAuthn passkeys
and machines with API keys, and calls your app. It is byte-compatible with core's
`calimero-sealed` and `calimero-client`.

What you can do with it:

- Connect to a Calimero Cloud cluster by name, with failover between its nodes.
- Sign up and sign in with passkeys, use API keys, and ask for passkey consent on sensitive calls.
- Install packages, create app instances, and call and view their methods.
- Follow an app's events and presence as streams.
- Upload and download sealed files and blobs.
- Manage organizations, teams, invites, deployments and every other route of the node, through methods generated from core's OpenAPI document.
- Build and sign a compute-only pipeline package (`.mpk`).

> **From version 27 this is the Calimero 1.0 client only.** The v1 SDK for merod 0.x nodes
> (`MeroJs`, the admin and auth API clients, JSON-RPC, contexts and namespaces)
> is in 26.x. See [Migrating from v1](https://calimero-network.github.io/mero-js/reference/migrating-from-v1/).

> **Before committing, titling a pull request, or releasing**, read [RELEASING.md](RELEASING.md): the PR title is the release commit, and the rules are shared by every Calimero repository that ships releases.

## Documentation

**<https://calimero-network.github.io/mero-js/>**: quickstart, guides, and the
reference for every method and the route it calls. The source is under [`docs/`](docs/).

## Installation

```bash
npm install @calimero-network/mero-js
```

The package ships ESM, CommonJS and a minified browser build with its own
TypeScript declarations. Its one dependency, `@phala/dcap-qvl`, verifies TDX
quotes and is loaded on first use.

## Quick start

```ts
import { CalimeroClient, browserPasskey, passkeyConfirm } from '@calimero-network/mero-js';

// Reach a Calimero Cloud cluster by name.
const client = await CalimeroClient.connect({
  cluster: 'eu-shared-1',
  // Trust the signed mero-tee release the node names, from this one up.
  policy: { release: { minReleaseVersion: '2.3.109' } },
});

await client.signUp(browserPasskey(), 'alice'); // or signIn(browserPasskey())
await client.packages.install({ package: 'com.example.kv', version: '1.0.0' }); // signed .mpk from the registry
// No `org`: the instance goes in your personal organization, which sign-up made.
const { appId: app } = await client.apps.create({ name: 'kv', package: 'com.example.kv', version: '1.0.0' });

await client.apps.call({ app, method: 'set', args: { key: 'a', value: '1' } });
const { result } = await client.apps.view({ app, method: 'get', args: { key: 'a' } });
await client.apps.call({ app, method: 'clear', args: {} }, { confirm: passkeyConfirm(browserPasskey()) }); // passkey consent
for await (const event of client.events(app)) console.log(event);
await client.apps.upgrade({ app, version: '1.1.0' }, { confirm: passkeyConfirm(browserPasskey()) }); // the owner approves
```

A development cluster (`meroctl local up`, `merod run --dev`) sends mock quotes,
which pass only with `policy: { allowMock: true }`. Nothing is read from the
environment.

## Connecting to a cluster

`CalimeroClient.connect({ cluster, policy })` is the recommended way to reach a
Calimero Cloud cluster. It reads the cluster's nodes from the public node
directory (`DEFAULT_DIRECTORY`, `https://manager.cloud.calimero.network/api/directory`;
`directory` names another), puts those in `zone` first when you give one, and
orders each group at random, weighted by the directory's weight. A cluster the
directory does not know, or one with no usable node, throws `DirectoryError`.

The directory only picks which nodes to try, and in what order. It is not
trusted: every node is attested under **your** `policy` before anything is
sent to it, so a wrong or tampered list can only make connecting fail.

Any node of a cluster serves any request. While the node in use answers `503`
or cannot be reached, a call or a view is retried there (`retry`); when those
retries run out it fails over to the next node and starts them again there,
with the **same** idempotency key, which the app's replicated log answers once
cluster-wide. Each node is tried once per request; when all have failed, the
last `UnavailableError` reaches you. After a failover, and once the directory's
`refreshAfterSeconds` has passed (checked at the next request), the client
reads the directory again in the background: the node in use stays first,
nodes no longer listed are dropped. `client.nodeUrl` is the node in use,
`client.nodes` the list.

For a fixed list of nodes or a cluster you run yourself, construct the client
directly: `new CalimeroClient({ baseUrl, nodes, policy })`. It fails over
between `baseUrl` and `nodes` the same way, and never reads a directory.

## What the client checks

Before it sends anything to a node, as core's Rust client does (`Measurements::Release`):

1. The node names the mero-tee release it runs. A release below `minReleaseVersion` is refused before anything is fetched.
2. The release's `published-mrtds.json` is fetched and its Sigstore signature is checked against the mero-tee node release workflow.
   By default it comes from the `cloud.calimero.network` mirror; `releaseSource: 'github'` reads the GitHub release instead (Node only: GitHub sends no CORS headers).
3. The quote is checked against Intel's collateral with `@phala/dcap-qvl`: the signature chain and the TCB status. The collateral comes from the cloud's mirror (`DEFAULT_COLLATERAL_MIRROR`), which browsers can read; it only carries Intel-signed items. `collateralSource: 'intel'` asks Intel's PCS (no CORS: not in browsers), `fetchCollateral` replaces the fetch, and `maxCollateralAgeDays` refuses collateral Intel issued longer ago.
4. A debug TD is refused.
5. The registers must be the release's `locked-read-only` image; a debug profile is trusted only with `allowDebugProfile`.
6. The quote's report data binds the client's nonce and the transport key the sealed channel opens to.

Pinned measurements go through `policy.verifyQuote` (`createQuoteVerifier`).

## The API

Every route of the node is a generated method, in a group per area
(`client.apps`, `client.auth`, `client.orgs`, `client.packages`,
`client.deployments`, `client.consent`, `client.ops`, `client.public`); see
[below](#the-runtime-and-the-route-layer). The hand-written methods left are the
flows and streams a single route cannot be:

- **Credentials**: `signUp(passkey, name, { invite?, origin?, signal? })`, `signIn(passkey, { origin?, signal? })`, `refresh`, `signOut`, `signOutEverywhere`, `useSession`, `useApiKey`, `addPasskey(newPasskey, approver)`.
- **Account recovery** (core #4844): signed out, `startAccountRecovery(code, passkey)` and, 72 hours later, `finishAccountRecovery(passkey, accountId?)`. The code itself is `client.auth.recoveryCreate({}, { confirm })`.
- **Organization recovery**: `startRecovery(org, seed)` (the seed signs here and never leaves the process); `newRecoverySeed`, `recoveryPublicKey`.
- **Files**: `uploadFile(app, data)` (a large file goes up in parts), `exportFile(app, id)` and `exportFileTo(app, id, sink)` (the plaintext, checked against its id, size and hash).
- **Blobs**: `downloadBlob(app, id)`, `downloadBlobGranted(app, id, grant)` (bytes; `client.apps.blobDownload` answers a `Blob` with its type).
- **Streams**: `events(app, from?, signal?)`, `presenceStream(app, signal?)`.
- **Approvals from an app**: `pendingApprovals(org?)`, `appsReportAll(org)`, then `openConsentPage(request, …)` to send the user to the node's consent page, and `waitForConsent(requestId)`; back on `returnTo`, `resumeConsent()` reads the decision on the request this page started from the node (never from the URL) and refuses a return for another request, one still pending, or one a newer request replaced (`CONSENT_REPLACED`: an account has one undecided request per node). Besides releases, upgrades and outbound hosts, `createOrgConsent`, `createInviteConsent` and `acceptInviteConsent` ask for a new organization, an invite or joining with one, and `leaveOrgConsent`, `removeMemberConsent`, `setMemberRoleConsent`, `setMemberTeamsConsent`, `removeMemberTeamsConsent`, `revokeInviteConsent`, `renameOrgConsent`, `deleteOrgConsent`, `setCapabilitiesConsent` and `setAllAppsConsent` for managing one; the result's `outcome` (`{ orgId }`, `{ token }`, `{ org }`, a rename's `{ orgId, name }`, a deletion's `{ apps }`) is typed by `kind`. Show the answer's `code` to the user before sending them to `url` (`openConsentPage`'s `confirm` hook); they should see the same code on the page (`confirmationCode(requestId)` checks it). See [Ask your users to approve node images and app upgrades](https://calimero-network.github.io/mero-js/guides/user-approvals/).

Refusals are typed (`LoginRequired`, `PasskeyUnknownHere`, `WrongNode`, `NotFound`, `ConfirmRequired`, `ConfirmCancelled`, `ConfirmPromptBlocked`,
`ArgsTooLarge`, `IdempotencyMismatch`, `ReleaseDeadlineError`, `RuntimeTooOld`, `ConsentStale`, `AppError`, `TrapError`,
`UnavailableError`, `OriginNotRegistered`, `BadOrigin`, `OriginTaken`, `QuotaExceeded`),
all extending `CalimeroError` with the node's `status` and `code`. A `401` once
the node that issued the session is replaced or out of reach is answered by one
refresh and a repeat; a `421 WRONG_NODE` by moving to the next of the `nodes` the client was
given. See the
[error model](https://calimero-network.github.io/mero-js/reference/error-model/).

The pieces it is built from are exported too: the sealed transport
(`createSealedFetch`), quote and signed-release verification
(`createQuoteVerifier`, `createSignedReleaseVerifier`, `trustSignedRelease`), and
the client for mero-tee's private agent gate (`attestAgent`, `provisionAgent`,
`releaseAgent`).

## The runtime and the route layer

The client is two layers. The **runtime** (`@calimero-network/mero-js/runtime`,
`src/runtime/`) is everything that is not a route: the node's attestation, the
sealed channel, the credential and the session's refresh on a `401`, moving on
`421 WRONG_NODE`, D70 retries and failover, error mapping and server-sent
events. The **route layer** is generated from core's OpenAPI document
(`openapi/core-openapi.json`, the node's `docs/public/openapi.json`) by
`pnpm gen:api`: `openapi-typescript` writes the types (`src/generated/api.ts`)
and `scripts/gen-methods.mjs` one method per operation
(`src/generated/methods.ts`), grouped by the operation's tag. `CalimeroClient`
exposes the groups, which share `client.transport`'s session:

```ts
import { passkeyConfirm } from '@calimero-network/mero-js';

const org = await client.orgs.get({ org: orgId });             // the typed payload
const { result } = await client.apps.call({ app, method: 'set', args: { key: 'a' } });
const { token } = await client.orgs.inviteCreate(               // a step-up (412)
  { org: orgId, role: 'member' },
  { confirm: passkeyConfirm(passkey) },
);
await client.apps.blobUpload({ app, body: bytes, contentType: 'image/png' });
```

- **Groups**: `client.apps`, `client.auth`, `client.orgs`, `client.packages`,
  `client.deployments`, `client.consent`, `client.ops`, `client.public`.
- **One argument object**: path parameters, query parameters and body fields
  merged, all typed from the document. The body goes whole under `body` when it is
  raw bytes (with an optional `contentType`) or when its field names would collide.
  A route whose query parameter is named like a path parameter takes its query
  under `query`.
- **The answer** is the route's typed success payload (a `Blob` for a download).
  A refusal is thrown as the typed `CalimeroError` (`NotFound`, `ConfirmRequired`, ...).
- **Challenges**: the second argument `{ confirm?, operatorProof?, signal? }`
  answers this call's `412 CONFIRM_REQUIRED` (`passkeyConfirm(passkey)`) or
  `428 OPERATOR_SIGNATURE_REQUIRED` (the operator key's signature over the
  statement). The client's own `new CalimeroClient(options, { confirm, operatorProof })`
  answers every call that gives none.
- **Retries**: a route the document marks `x-calimero-retry-safe` (a call, a
  view, a method by its path) is sent again on a `503` as D70 says; one whose
  retries require an `idempotencyKey` gets one when it has none, which its retries
  and its confirmed retry share. A `421 WRONG_NODE` moves to the next node.
- **Step-ups** ask the passkey for the route's own user verification
  (`x-calimero-user-verification`: `preferred` for the sign-up's confirmation,
  `required` elsewhere).
- **Streams** (`events`, `presenceStream`) stay hand-written on the runtime.
- `createRoutes(createRouteClient(transport, { confirm, operatorProof }))` builds
  the same groups on any `NodeTransport`. The route client under them sends each
  request through the transport (sealed, with the credential; `/attestation` and
  `/health` in the clear); write against the groups, not against it.

The single-route methods of 29.x (`createOrg`, `call`, ...) are gone; see
[Upgrading from 29.x](#upgrading-from-29x). The ceremonies (`signUp`, `signIn`, `addPasskey`,
account and organization recovery, `openConsentPage`, ...) stay as flows.

The runtime's own surface toward routes is still one interface, for a route
layer of your own:

```ts
interface Transport {
  request(method: HttpMethod, path: string, options?: RequestOptions): Promise<Response>; // 2xx, body unread
  json<T>(method: HttpMethod, path: string, options?: RequestOptions): Promise<T>;
  events(path: string, options?: { query?; signal? }): AsyncGenerator<NodeEvent>;
}
// RequestOptions: { query?, body? (JSON) | bytes? + contentType?, passkey? (answers a 412),
//                   userVerification?, retry?: true | 'once' (D70), signal?, promptSignal? }
```

`NodeTransport` implements it, and `exchange(method, path, options)` answers a
refusal as the node's response instead of throwing it (what the generated
client sends through). The `/runtime` entry re-exports the package's own bundle,
so its error classes are the ones `CalimeroClient` throws (`instanceof` holds
across both imports).

### Refreshing the OpenAPI document

`openapi/core-openapi.json` is vendored from core at the commit `openapi/SOURCE`
names. `scripts/refresh-openapi.sh [<core ref>]` (`pnpm refresh:openapi`)
fetches it again, records the commit and regenerates `src/generated/`;
`pnpm check:api` fails when the generated types or methods are not those of the
vendored document.

`.github/workflows/openapi-sync.yml` does this on its own: core sends a
`repository_dispatch` (`core-openapi`, with the core commit) whenever its
document changes, and a weekly run catches a missed one (or run it by hand with a
`ref`). When the document or the generated code changed, it opens or refreshes a
pull request on `chore/core-openapi` that lists the generated methods added,
removed or changed (`scripts/openapi-sync-summary.mjs`); a removed or renamed
one needs a major release.

It opens that pull request with a GitHub App's token, so the pull request's CI
runs: the organization's shared `vars.ACTIONS_APP_ID` /
`secrets.ACTIONS_APP_PRIVATE_KEY`, which must be shared with this repository. The
app must be installed on this repository
with contents and pull-requests write.

## Upgrading from 29.x

30.0 removes the single-route methods 29.x deprecated, and the untyped `get`,
`post`, `put`, `delete`, `postConfirmed` and `deleteConfirmed`. Each is a
generated method now, which takes one argument object and answers the route's
payload (pick the field the old method answered: `(await client.orgs.create({
name })).orgId`). A step-up takes `{ confirm: passkeyConfirm(passkey) }` as the
second argument. `client.packages` and `client.deployments` are groups, no longer
callable (`(await client.packages.list()).packages`).

| 29.x | 30.0 |
| --- | --- |
| `signUpPolicy()` | `client.deployments.signUpPublic()` |
| `listSessions()` | `(await client.auth.sessionsList()).sessions` |
| `revokeSession(session)` | `client.auth.sessionRevoke({ session })` |
| `listActivity(after)` | `(await client.auth.activityList({ after })).activity` |
| `me()` | `client.auth.me()` |
| `removePasskey(credentialId, passkey)` | `client.auth.passkeyRemove({ cred: credentialId }, { confirm: passkeyConfirm(passkey) })` |
| `labelPasskey(credentialId, label)` | `client.auth.passkeyLabel({ cred: credentialId, label })` |
| `createRecoveryCode(passkey)` | `(await client.auth.recoveryCreate({}, { confirm: passkeyConfirm(passkey) })).code` |
| `removeRecoveryCode(passkey)` | `client.auth.recoveryRemove({}, { confirm: passkeyConfirm(passkey) })` |
| `cancelAccountRecovery(passkey)` | `client.auth.accountRecoveryCancel({}, { confirm: passkeyConfirm(passkey) })` |
| `usage()` | `client.auth.usage()` |
| `attestation()` | `client.public.attestation({ nonce: '00'.repeat(32) })` |
| `listApiKeys()` | `(await client.auth.apikeyList()).keys` |
| `revokeApiKey(keyId, passkey)` | `client.auth.apikeyRevoke({ key: keyId }, { confirm: passkeyConfirm(passkey) })` |
| `createApiKey(options, passkey)` | `client.auth.apikeyCreate({ ...options }, { confirm: passkeyConfirm(passkey) })` |
| `uploadBundle(wasm)` | `(await client.packages.bundlePut({ body: wasm, contentType: 'application/wasm' })).hash` |
| `uploadBlob(app, data, contentType)` | `client.apps.blobUpload({ app, body: data, contentType })` |
| `blobContent(app, id, options)` | `client.apps.blobDownload({ app, id, grant })` (a `Blob`: `arrayBuffer()` and `type`) |
| `listBlobs(app)` | `(await client.apps.blobList({ app })).blobs` |
| `blobInfo(app, id)` | `client.apps.blobInfo({ app, id })` |
| `deleteBlob(app, id)` | `client.apps.blobDelete({ app, id })` |
| `uploadPackage(mpk)` | `client.packages.upload({ body: mpk, contentType: 'application/octet-stream' })` |
| `installPackage(pkg, version)` | `client.packages.install({ package: pkg, version })` |
| `packages()` | `(await client.packages.list()).packages` |
| `package(pkg)` | `client.packages.get({ package: pkg })` |
| `uninstallPackage(pkg, version)` | `client.packages.uninstall({ package: pkg })` or `client.packages.versionUninstall({ package: pkg, version })` |
| `listApps(filter)` | `(await client.apps.list({ ...filter })).apps` |
| `getApp(app)` | `client.apps.get({ app })` |
| `deleteApp(app)` | `client.apps.delete({ app })` |
| `setAccess(app, access)` | `client.apps.access({ app, access })` |
| `setAppMetadata(app, metadata)` | `client.apps.metadata({ app, metadata })` |
| `appXcalls(app)` | `client.apps.xcalls({ app })` |
| `appStorage(app)` | `client.apps.storage({ app })` |
| `appAbi(app)` | `client.apps.abi({ app })` |
| `transferApp(app, to, passkey)` | `client.apps.transfer({ app, to }, { confirm: passkeyConfirm(passkey) })` |
| `moveApp(app, toOrg, options, passkey)` | `client.apps.move({ app, toOrg, ...options }, { confirm: passkeyConfirm(passkey) })` |
| `upgradeApp(app, target, passkey)` | `client.apps.upgrade({ app, ...target }, { confirm: passkeyConfirm(passkey) })` |
| `createApp(name, bundle, options)` | `(await client.apps.create({ name, bundle, ...options })).appId` |
| `call(app, method, args, idempotencyKey)` | `client.apps.call({ app, method, args, idempotencyKey })` |
| `callConfirmed(app, method, args, passkey, idempotencyKey)` | `client.apps.call({ app, method, args, idempotencyKey }, { confirm: passkeyConfirm(passkey) })` |
| `view(app, method, args, options)` | `client.apps.view({ app, method, args, consistency: options.stale ? 'stale' : undefined, minIndex: options.minIndex })` |
| `publishPresence(app, seq, state)` | `client.apps.presencePublish({ app, seq, state: presenceState(state) })` |
| `presence(app)` | `(await client.apps.presenceGet({ app })).present` |
| `setAlias(alias, app)` | `client.auth.aliasSet({ alias, app })` |
| `aliases()` | `(await client.auth.aliasList()).aliases` |
| `alias(alias)` | `(await client.auth.aliasGet({ alias })).appId` |
| `deleteAlias(alias)` | `client.auth.aliasDelete({ alias })` |
| `createOrg(name, options)` | `(await client.orgs.create({ name, ...options })).orgId` |
| `setRecoveryKey(org, publicKey, passkey)` | `client.orgs.recoverySet({ org, publicKey }, { confirm: passkeyConfirm(passkey) })` |
| `cancelRecovery(org, passkey)` | `client.orgs.recoveryCancel({ org }, { confirm: passkeyConfirm(passkey) })` |
| `finishRecovery(org, passkey)` | `client.orgs.recoveryFinish({ org }, { confirm: passkeyConfirm(passkey) })` |
| `getOrg(org)` | `client.orgs.get({ org })` |
| `orgMembersSecurity(org)` | `(await client.orgs.members({ org, detail: 'security' })).members` |
| `deleteOrg(org, passkey)` | `client.orgs.delete({ org }, { confirm: passkeyConfirm(passkey) })` |
| `setOrgMetadata(org, metadata)` | `client.orgs.metadata({ org, metadata })` |
| `setMemberMetadata(org, account, metadata)` | `client.orgs.memberMetadata({ org, account, metadata })` |
| `upgradeOrg(org, target, passkey)` | `client.orgs.upgrade({ org, ...target }, { confirm: passkeyConfirm(passkey) })` |
| `upgradeRun(org, run)` | `client.orgs.upgradeStatus({ org, run })` |
| `retryUpgradeRun(org, run, passkey)` | `client.orgs.upgradeRetry({ org, run }, { confirm: passkeyConfirm(passkey) })` |
| `approveRelease(org, version, passkey, approved)` | `client.orgs.releaseApproval({ org, version, approved }, { confirm: passkeyConfirm(passkey) })` (an account's own apps are in its personal organization: approve there; core 1.0.0-rc.20 removed the account-level approval) |
| `approveOutbound(org, pkg, passkey, options)` | `client.orgs.outboundApproval({ org, package: pkg, ...options }, { confirm: passkeyConfirm(passkey) })` (the personal organization's for the account's own apps, as above) |
| `appsReport(org, page)` | `client.orgs.apps({ org, ...page })`; `client.auth.accountApps({ ...page })` lists the apps the account owns in any organization |
| `releaseRules()` | `client.public.releasesPublic()` |
| `requestConsent(request)` | `client.consent.pageCreate({ body: request })` |
| `consentResult(requestId)` | `client.consent.pageResult({ request: requestId })` |
| `publisherBinding(signerId)` | `client.public.publisherBinding({ signer: signerId })` |
| `bindPublisher(org, proof, passkey)` | `client.orgs.publisher({ org, ...proof }, { confirm: passkeyConfirm(passkey) })` |
| `upgradePublished(org, pkg, target, passkey)` | `client.orgs.publisherUpgrade({ org, package: pkg, ...target }, { confirm: passkeyConfirm(passkey) })` |
| `opsApps(page)` | `client.ops.apps({ ...page })` |
| `opsOrgs(page)` | `client.ops.orgs({ ...page })` |
| `opsServedOrigins()` | `client.ops.served()` |
| `setReleaseDeadline(version, after)` | `client.ops.releaseDeadline({ version, after }) (after as an RFC 3339 string)` |
| `liftReleaseDeadline(version)` | `client.ops.releaseDeadlineLift({ version })` |
| `setMember(org, account, role, passkey)` | `client.orgs.setMember({ org, account, role }, { confirm: passkeyConfirm(passkey) })` |
| `removeMember(org, account, passkey)` | `client.orgs.removeMember({ org, account }, { confirm: passkeyConfirm(passkey) })` |
| `setCapabilities(org, capabilities, passkey, at)` | `client.orgs.capabilities({ org, capabilities, ...at }, { confirm: passkeyConfirm(passkey) })` |
| `setAllApps(org, account, allApps, passkey)` | `client.orgs.allApps({ org, account, allApps }, { confirm: passkeyConfirm(passkey) })` |
| `setTeamDefaultCapabilities(org, team, capabilities, passkey)` | `client.orgs.capabilities({ org, team, capabilities }, { confirm: passkeyConfirm(passkey) })` |
| `createTeam(org, name, join, parent)` | `(await client.orgs.teamCreate({ org, name, join, parent })).teamId` |
| `getTeam(org, team)` | `client.orgs.teamGet({ org, team })` |
| `updateTeam(org, team, update)` | `client.orgs.teamUpdate({ org, team, ...update })` |
| `deleteTeam(org, team, passkey)` | `client.orgs.teamDelete({ org, team }, { confirm: passkeyConfirm(passkey) })` |
| `setTeamMember(org, team, account, passkey, row)` | `client.orgs.teamSetMember({ org, team, account, ...row }, { confirm: passkeyConfirm(passkey) })` |
| `removeTeamMember(org, team, account, passkey)` | `client.orgs.teamRemoveMember({ org, team, account }, { confirm: passkeyConfirm(passkey) })` |
| `invite(org, role, passkey, teams, options)` | `(await client.orgs.inviteCreate({ org, role, teams, ...options }, { confirm: passkeyConfirm(passkey) })).token` |
| `redeemInvite(token)` | `client.orgs.inviteRedeem({ token })` |
| `listInvites(org)` | `(await client.orgs.inviteList({ org })).invites` |
| `revokeInvite(org, inviteKey, passkey)` | `client.orgs.inviteRevoke({ org, key: inviteKey }, { confirm: passkeyConfirm(passkey) })` |
| `listAppTeams(org)` | `client.orgs.appTeamList({ org })` |
| `scopeApps(org, passkey, options)` | `client.orgs.appTeamsScope({ org, ...options }, { confirm: passkeyConfirm(passkey) })` |
| `packageOrigins.list(pkg)` | `(await client.packages.originsList({ package: pkg })).origins` |
| `packageOrigins.add(pkg, origin, passkey, options)` | `client.packages.originsAdd({ package: pkg, origin, ...options }, { confirm: passkeyConfirm(passkey) })` |
| `packageOrigins.remove(pkg, origin, passkey)` | `client.packages.originsRemove({ package: pkg, origin }, { confirm: passkeyConfirm(passkey) })` |
| `grantDeployer(deployer, passkey, options)` | `client.deployments.deployerGrant({ deployer, ...options }, { confirm: passkeyConfirm(passkey) }) (until as an RFC 3339 string)` |
| `deployers()` | `client.deployments.deployerList()` |
| `revokeDeployer(deployer, passkey)` | `client.deployments.deployerRevoke({ key: deployerKey(deployer) }, { confirm: passkeyConfirm(passkey) })` (or the key string itself) |
| `createDeployment(name, owner, passkey)` | `(await client.deployments.create({ name, owner }, { confirm: passkeyConfirm(passkey) })).id` |
| `deployments()` | `(await client.deployments.list()).deployments` |
| `deployment(id)` | `client.deployments.get({ id })` |
| `deleteDeployment(id, passkey)` | `client.deployments.delete({ id }, { confirm: passkeyConfirm(passkey) })` |
| `offerApp(id, offer, passkey)` | `client.deployments.offer({ id, ...offer }, { confirm: passkeyConfirm(passkey) })` |
| `withdrawApp(id, app, passkey)` | `client.deployments.withdraw({ id, app }, { confirm: passkeyConfirm(passkey) })` |
| `deploymentOrgs(id)` | `client.deployments.orgs({ id })` |
| `get(path)` | the route's generated method: `client.get('orgs/' + org)` → `client.orgs.get({ org })` |
| `post(path, body)` | the route's generated method: `client.post('orgs', { name })` → `client.orgs.create({ name })` |
| `put(path, body)` | the route's generated method: `client.put('auth/aliases/' + alias, { app })` → `client.auth.aliasSet({ alias, app })` |
| `delete(path, body)` | the route's generated method: `client.delete('apps/' + app)` → `client.apps.delete({ app })` |
| `postConfirmed(path, body, passkey)` | the route's generated method with `{ confirm: passkeyConfirm(passkey) }`: `client.postConfirmed('orgs/' + org + '/upgrade', body, passkey)` → `client.orgs.upgrade({ org, ...body }, { confirm: passkeyConfirm(passkey) })` |
| `deleteConfirmed(path, passkey, body)` | the route's generated method with `{ confirm: passkeyConfirm(passkey) }`: `client.deleteConfirmed('orgs/' + org, passkey)` → `client.orgs.delete({ org }, { confirm: passkeyConfirm(passkey) })` |

## Development

```bash
pnpm install
pnpm build
pnpm test:unit   # unit tests, against a stand-in node over a real sealed channel
pnpm lint
pnpm typecheck
pnpm typecheck:consumer
pnpm typecheck:tests
```

The cluster tests (`tests/cluster`) start three `merod run --dev` nodes and
run the client against them:

```bash
MEROD_BINARY=/path/to/merod KV_STORE_WASM=/path/to/kv_store.wasm pnpm test:cluster
```

`KV_STORE_WASM` is core's kv component (`apps/fixture-kv`, `apps/v2-fixtures/kv`
in older checkouts: a `calimero:app` component for `wasm32-wasip2`). Without the variables they use
`../core/target/debug/merod` and `../core/target/app-components/kv.wasm` from a
sibling core checkout (1.0.0-rc.25 or later), built there with core's
`meroctl`:

```bash
cargo build -p merod -p meroctl
target/debug/meroctl app build apps/fixture-kv --out target/app-components/kv.wasm
```

`scripts/download-released-merod.sh` fetches the newest released 1.0 `merod`
and the `app.wasm` of its `kv-store-test-fixture.mpk` (as `./merod` and
`./kv_store.wasm`), which is what CI runs them against; core's `e2e.yml` runs
the same suite against core's own tree.

## Related repositories

- [core](https://github.com/calimero-network/core): the node (`merod`), the Rust SDK and client, `meroctl`, and the [docs](https://calimero-network.github.io/core/).
- [mero-tee](https://github.com/calimero-network/mero-tee): the TDX node images and the signed releases this client verifies.
- [app-registry](https://github.com/calimero-network/app-registry): the registry of signed app packages.
- [mero-react](https://github.com/calimero-network/mero-react): React bindings built on this client.
- [calimero-client-py](https://github.com/calimero-network/calimero-client-py), [swift-sdk](https://github.com/calimero-network/swift-sdk), [kotlin-sdk](https://github.com/calimero-network/kotlin-sdk): the clients for Python, Swift and Kotlin.
- [apps](https://github.com/calimero-network/apps): Calimero apps.

## License

MIT — see [LICENSE](LICENSE).
