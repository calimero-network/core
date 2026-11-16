# Changelog

Notable changes to MeroKit. Versions are git tags (see [RELEASING.md](RELEASING.md)).

## Unreleased: the sealed handshake and the directory drop `v2` from their paths

- `SealedTransport` opens a session at `POST /sealed/handshake`
  (`SealedTransport.handshakePath`). A node released before that path answers
  `404` or `405` there, and the handshake is then sent once more to
  `POST /sealed/v2/handshake` (the new `SealedTransport.legacyHandshakePath`),
  so older nodes are still reached. Any other refusal is reported as before.
- `defaultDirectory` is `https://manager.cloud.calimero.network/api/directory`
  (was `.../api/v2/directory`, which the manager still serves).

## Unreleased: consent requests manage organizations

`ConsentRequest` gains the ten organization kinds of calimero-network/core#5018:
`.leaveOrg(org:returnTo:)`, `.removeMember(org:account:returnTo:)`,
`.setMemberRole(org:account:role:returnTo:)`,
`.setMemberTeams(org:account:teams:returnTo:)` (rows of the new
`ConsentTeamMembership(team:role:capabilities:)`),
`.removeMemberTeams(org:account:teams:returnTo:)`,
`.revokeInvite(org:inviteKey:returnTo:)`, `.renameOrg(org:name:returnTo:)`,
`.deleteOrg(org:returnTo:)`,
`.setCapabilities(org:account:team:capabilities:returnTo:)` (`OrgCapabilities`)
and `.setAllApps(org:account:allApps:returnTo:)`. Each is checked before it is
sent, as the earlier kinds are.

- `ConsentOutcome` gains `.orgRenamed(orgId:name:)` and `.orgDeleted(apps:)`,
  with `ConsentResult.newOrgName` and `deletedApps`; `consentResult` refuses an
  approved `renameOrg` or `deleteOrg` without its outcome. The other eight kinds
  answer no outcome.
- `ConsentResult.changesOrganizations`: an approved request that changed an
  organization the account sees. `MeroAppSession` bumps `revision` on it, for
  every organization kind and also when a return is handled through
  `handle(url)`, where it used to bump only for a created or joined
  organization from `requestConsent`.
- `ConsentRequest.generated()` now gives the generated body for every kind
  (the vendored OpenAPI document has them since #106), where it threw for the
  organization kinds.
- Source compatibility: `ConsentRequest` and `ConsentOutcome` gain cases, so an
  exhaustive `switch` over either needs the new cases (or a `default`).

## Unreleased: consent requests create organizations and mint or accept invites

`ConsentRequest` gains core's new consent kinds (calimero-network/core#5014):
`.createOrg(name:returnTo:)`, `.createInvite(org:role:teams:ttlSecs:uses:allApps:returnTo:)`
and `.acceptInvite(token:returnTo:)`, plus `kind` and `returnTo`.
`ConsentResult` gains `outcome: ConsentOutcome?`, what the approval made
(`.orgCreated(orgId:)`, `.inviteCreated(token:)`, `.inviteAccepted(org:)`),
and `orgId`, `inviteToken`, `joinedOrg` and `isApproved`. The outcome is
read only from the sealed `/consent/{id}/result`, never from the page's
redirect. An invite token is redacted from every description and mirror
(`print`, `dump`, interpolation) of `ConsentRequest`, `ConsentResult` and
`ConsentOutcome`, and no error carries it.

- `CalimeroClient.requestConsent(_:)` and `consentResult(_:)` are back,
  hand-written: they send and read every kind (the generated
  `consent.pageCreate` / `pageResult` types know only `release`, `upgrade`
  and `outbound`). `openConsentPage`, `waitForConsent`,
  `ConsentLauncher.request`, `MeroClient.requestConsent` and
  `MeroAppSession` go through them. `ConsentRequest.generated()` throws for
  the new kinds.
- `CalimeroClient.confirmConsentReturn(_:returned:)` reads the result of the
  request the app started after the page sent the person back, and throws
  `CONSENT_REQUEST_MISMATCH` when the return names another request, or
  `CONSENT_NOT_DECIDED` when the app's request is still pending (another
  request approved, or a forged page), or `CONSENT_REPLACED` when the node
  answers 404 for it (an account has one undecided request per node, and a
  newer one replaced it). `MeroAppSession.handle(_:)` and
  `requestConsent(_:)` and `MeroClient.requestConsent` apply it.
- MeroKitUI's `ConsentLauncher.decide(client:request:callback:...)` asks,
  opens the page and answers the checked result. With an https (universal
  link) callback it uses `ASWebAuthenticationSession`'s `.https(host:path:)`
  callback on iOS 17.4+ / macOS 14.4+, and the browser plus `waitForConsent`
  before that.
- `MeroClient.requestConsent` takes `returnTo:` (default: `callback.url`), for
  a `returnTo` such as the package's served page.
- A `MeroAppSession` consent that creates or joins an organization bumps
  `revision`.

Upgrade notes:

- A `switch` over `ConsentRequest` that lists every case needs the three new
  cases (or a `default`).

## Unreleased: `apps.blobUpload` takes the blob's content type again

`apps.blobUpload(app:body:contentType:sealed:)` gains an optional
`contentType: String? = nil`. The node stores a blob with the request's
`Content-Type` and answers its downloads with it; since 3.0.0 the facade sent
`application/octet-stream` for every upload, because core's document lists
that type beside `*/*` and the facade generator only gave a `contentType`
argument to a route listing `*/*` alone. A route that lists `*/*` among other
types now takes it too; nil (the default) still sends the first listed type,
so existing calls are unchanged. Pass `contentType: "image/png"` to keep an
image's type.

## Unreleased: the `NoRelay*` aliases are gone

**Breaking.** The deprecated `NoRelayContextContract` typealias (the 0.x
name, kept since Calimero 1.0 has no relays) is removed.

Upgrade notes:

- Replace `NoRelayContextContract` with `SignedOutContextContract`.

## 1.0.0 (unreleased): the deprecated route wrappers are gone

**Breaking.** The 83 `CalimeroClient` methods 0.x deprecated in favour of the
generated facade are removed. Every route is reached through the facade
(`client.auth`, `.apps`, `.orgs`, `.consent`, `.ops`, `.packages`,
`.deployments`, `.publicApi`), which answers the generated types
(`import MeroAPI`) and throws the same typed errors.

Upgrade notes:

- The groups 0.x called `packagesApi` and `deploymentsApi` (their names
  were taken by `packages()` and `deployments()`) are `packages` and
  `deployments`.
- Replace each call with the method the table below names; the facade takes
  labelled arguments (`orgs.get(org: id)`, not `getOrg(id)`) and answers the
  generated type: `orgs.create(name:).orgId`, `orgs.inviteCreate(...).token`,
  `apps.create(...).appId`.
- Answers are the generated types, decoded strictly (core@94bcde3 types
  every answer and makes optional what older nodes leave out). To read one as
  a MeroKit type, go through its JSON:
  `OrgInfo(json: try JSONValue(encoding: try await client.orgs.get(org: id)))`,
  `try MeroJSON.convert(try await client.consent.pageResult(request: id), to: ConsentResult.self)`.
- Arguments that were MeroKit types take their `generated` form:
  `AppAccess.team(id).generated`, `Deployer.org(id).generated`,
  `OrgRole.admin.generated`, `TeamJoin.everyone.generated`,
  `try ConsentRequest.generated()`.
- A step-up method takes `passkey:` (or set `useStepUp(_:)` once); an
  operator decision takes `operatorSigner:` (or `useOperatorSigner(_:)`).
- Kept, hand-written (several requests, byte-exact numbers, streams, the
  sealed-file fields the blob shapes leave out, or a MeroKit-typed
  convenience): `call`/`callJSON`/`callConfirmed`/`view`/`viewJSON`,
  `events`/`presenceStream`, the sign-in ceremonies, `uploadFile`,
  `blobContent`, `listBlobs`, `blobInfo`, `setRecoveryKey`, `updateTeam`,
  `startRecovery`, `openConsentPage`, `waitForConsent`, `pendingApprovals`,
  `appsReportAll` and the raw routes.

| Removed (0.x) | Use (generated facade) |
| --- | --- |
| `signUpPolicy()` | `deployments.signUpPublic()` |
| `me()` | `auth.me()` |
| `removePasskey(_:passkey:)` | `auth.passkeyRemove(cred:passkey:)` |
| `labelPasskey(_:label:)` | `auth.passkeyLabel(cred:label:)` |
| `usage()` | `auth.usage()` |
| `attestation()` | `publicApi.attestation(nonce:)` |
| `listApiKeys()` | `auth.apikeyList()` |
| `revokeApiKey(_:passkey:)` | `auth.apikeyRevoke(key:passkey:)` |
| `createApiKey(label:apps:methods:readOnly:allowConfirm:ttlSecs:upgrades:passkey:)` | `auth.apikeyCreate` |
| `uploadBlob(_:_:contentType:)` | `apps.blobUpload(app:body:contentType:)` |
| `uploadBundle(_:)` | `packages.bundlePut(body:)` |
| `downloadBlob(_:_:)` | `apps.blobDownload(app:id:)` |
| `downloadBlobGranted(_:_:grant:)` | `apps.blobDownload(app:id:grant:)` |
| `deleteBlob(_:_:)` | `apps.blobDelete(app:id:)` |
| `uploadPackage(_:)` | `packages.upload(body:)` |
| `installPackage(_:version:)` | `packages.install(package:version:)` |
| `packages()` | `packages.list()` |
| `package(_:)` | `packages.get(package:)` |
| `uninstallPackage(_:version:)` | `packages.uninstall` or `packages.versionUninstall` |
| `listApps(package:bundle:)` | `apps.list(package:bundle:)` |
| `getApp(_:)` | `apps.get(app:)` |
| `deleteApp(_:)` | `apps.delete(app:)` |
| `setAccess(_:_:)` | `apps.access(app:access:)` |
| `setAppMetadata(_:_:)` | `apps.metadata(app:metadata:)` |
| `appXcalls(_:)` | `apps.xcalls(app:)` |
| `appStorage(_:)` | `apps.storage(app:)` |
| `appAbi(_:)` | `apps.abi(app:)` |
| `transferApp(_:to:passkey:)` | `apps.transfer(app:to:passkey:)` |
| `upgradeApp(_:version:package:service:bundle:migrate:passkey:)` | `apps.upgrade(app:bundle:package:version:service:migrate:passkey:)` |
| `createApp(_:bundle:org:access:confirmMethods:initArgs:)` | `apps.create` |
| `publishPresence(_:seq:state:)` | `apps.presencePublish(app:seq:state:)` |
| `presence(_:)` | `apps.presenceGet(app:)` |
| `setAlias(_:app:)` | `auth.aliasSet(alias:app:)` |
| `aliases()` | `auth.aliasList()` |
| `alias(_:)` | `auth.aliasGet(alias:)` |
| `deleteAlias(_:)` | `auth.aliasDelete(alias:)` |
| `createOrg(_:recoveryKey:)` | `orgs.create(name:recoveryKey:)` |
| `cancelRecovery(_:passkey:)` | `orgs.recoveryCancel(org:passkey:)` |
| `finishRecovery(_:passkey:)` | `orgs.recoveryFinish(org:passkey:)` |
| `getOrg(_:)` | `orgs.get(org:)` |
| `deleteOrg(_:passkey:)` | `orgs.delete(org:passkey:)` |
| `setOrgMetadata(_:_:)` | `orgs.metadata(org:metadata:)` |
| `setMemberMetadata(_:account:_:)` | `orgs.memberMetadata(org:account:metadata:)` |
| `upgradeOrg(_:_:passkey:)` | `orgs.upgrade(org:package:version:team:cascade:migrate:passkey:)` |
| `upgradeRun(_:_:)` | `orgs.upgradeStatus(org:run:)` |
| `retryUpgradeRun(_:_:passkey:)` | `orgs.upgradeRetry(org:run:passkey:operatorSigner:)` |
| `approveRelease(_:version:passkey:approved:)` | `orgs.releaseApproval` or `auth.releaseApproval` |
| `approveOutbound(_:package:passkey:prefixes:replace:)` | `orgs.outboundApproval` or `auth.outboundApproval` |
| `appsReport(_:page:)` | `orgs.apps(org:after:limit:)` or `auth.apps(after:limit:)` |
| `releaseRules()` | `publicApi.releasesPublic()` |
| `consentResult(_:)` | `consent.pageResult(request:)` |
| `publisherBinding(_:)` | `publicApi.publisherBinding(signer:)` |
| `bindPublisher(_:_:passkey:)` | `orgs.publisher(org:signerId:publicKey:signature:passkey:)` |
| `upgradePublished(_:package:version:migrate:passkey:)` | `orgs.publisherUpgrade(org:package:version:migrate:passkey:)` |
| `opsApps(page:)` | `ops.apps(after:limit:)` |
| `opsOrgs(page:)` | `ops.orgs(after:limit:)` |
| `setReleaseDeadline(_:after:)` | `ops.releaseDeadline(version:after:operatorSigner:)` |
| `liftReleaseDeadline(_:)` | `ops.releaseDeadlineLift(version:operatorSigner:)` |
| `setMember(_:account:role:passkey:)` | `orgs.setMember(org:account:role:passkey:)` |
| `removeMember(_:account:passkey:)` | `orgs.removeMember(org:account:passkey:)` |
| `setCapabilities(_:_:passkey:account:team:)` | `orgs.capabilities(org:account:team:capabilities:passkey:)` |
| `createTeam(_:name:join:parent:)` | `orgs.teamCreate(org:name:parent:join:)` |
| `getTeam(_:_:)` | `orgs.teamGet(org:team:)` |
| `deleteTeam(_:_:)` | `orgs.teamDelete(org:team:passkey:)` |
| `setTeamMember(_:_:account:passkey:role:lead:capabilities:)` | `orgs.teamSetMember` |
| `removeTeamMember(_:_:account:passkey:)` | `orgs.teamRemoveMember(org:team:account:passkey:)` |
| `invite(_:role:passkey:teams:ttlSecs:uses:)` | `orgs.inviteCreate(org:role:teams:ttlSecs:uses:allApps:passkey:)` |
| `redeemInvite(_:)` | `orgs.inviteRedeem(token:)` |
| `packageOrigins(_:)` | `packages.originsList(package:)` |
| `addPackageOrigin(_:origin:passkey:rpId:)` | `packages.originsAdd(package:origin:rpId:served:passkey:)` |
| `removePackageOrigin(_:origin:passkey:)` | `packages.originsRemove(package:origin:passkey:)` |
| `grantDeployer(_:passkey:until:)` | `deployments.deployerGrant(deployer:until:passkey:)` |
| `deployers()` | `deployments.deployerList()` |
| `revokeDeployer(_:passkey:)` | `deployments.deployerRevoke(key:passkey:)` |
| `revokeDeployer(key:passkey:)` | `deployments.deployerRevoke(key:passkey:)` |
| `createDeployment(_:owner:passkey:)` | `deployments.create(name:owner:passkey:)` |
| `deployments()` | `deployments.list()` |
| `deployment(_:)` | `deployments.get(id:)` |
| `deleteDeployment(_:passkey:)` | `deployments.delete(id:passkey:)` |
| `offerApp(_:app:package:version:passkey:)` | `deployments.offer(id:app:package:version:passkey:)` |
| `withdrawApp(_:app:passkey:)` | `deployments.withdraw(id:app:passkey:)` |
| `deploymentOrgs(_:)` | `deployments.orgs(id:)` |
| `requestConsent(_:)` | `consent.pageCreate(body:)` |


## Unreleased: the node API is generated from core's OpenAPI document

`CalimeroClient` gains a facade with one method per route of core's OpenAPI
document (`openapi/openapi.json`, core `docs/openapi`), grouped by its
`x-calimero-group`: `client.orgs.inviteCreate(org:role:...)`,
`client.apps.get(app:)`, `client.auth.me()`, `client.packages.list()`, ...
Retry-safe operations retry and fail over like `call`. It is generated (`scripts/generate-openapi.sh`)
over swift-openapi-generator's client (the new `MeroAPI` product, whose types
the facade answers) and checked in; CI fails when it is stale. Requests still
go through the attested sealed channel; refusals throw the same typed errors.

- **Step-ups are automatic.** A `412 CONFIRM_REQUIRED` is answered with the
  call's `passkey:` or the client's `useStepUp(_:)` passkey; a `428
  OPERATOR_SIGNATURE_REQUIRED` (new: `OperatorSignatureRequired`) with the
  call's `operatorSigner:` or `useOperatorSigner(_:)` (`Ed25519OperatorKey`).
- **Deprecated:** the 83 methods that wrap one route (`getOrg`, `invite`,
  `createApp`, `listApps`, ...). They keep their signatures and answers and
  name their replacement in the warning.
- **New dependencies:** `swift-openapi-runtime` 1.9.0 and `swift-http-types`
  1.5.1 (exact). `MeroKit.podspec` cannot express them yet.
- A path parameter's reserved characters are now percent-encoded
  (`/deployers/org%3Ao1`); the node decodes them as before.

## Unreleased: an account uploads and downloads blobs through its relay

`AdminSurface` gains `uploadBlob`, `getBlob(_:contextId:)`,
`getBlobInfo(_:contextId:)` and `listBlobs`, so an app holding an
`any AdminSurface` (a node's admin or an account's) can send and read
attachments and avatars the same way.

On `AccountAdmin` they go to the account's relay, which grants an account
session `blob:add-own` and `blob:get-own`: a blob is stored into, and read out
of, a context the account is a member of, and the relay checks that on every
call. So each call needs a `contextId` and throws `AccountError.invalidInput`
without one, before anything is sent. With no relay, an upload or read throws
`AccountError.noRelay` and `listBlobs` is empty. Deleting a blob stays a
node's call. A node's `AdminApi` and `NodeAdmin` keep their behaviour.
## Unreleased: a join signs at the schema version the node names

Relays in the cloud do not all move to a new core release at once. A relay
still on core 0.11.0-rc.90 refused every join signed at schema 27 with
`schema version mismatch: expected 26, got 27`, so nobody could join a
namespace it admits for. 26 and 27 sign the same layout (core#4625), so
`AccountJoin.joinWithNode` now signs once more at the version the node names
when it is one of `NamespaceOps.acceptedSchemaVersions` (26...27), and never
retries twice or guesses at a version it does not know.
`NamespaceOps.signMemberJoinOp` / `memberJoinSignable` take an optional
`schemaVersion` (default `NamespaceOps.schemaVersion`, still 27).

## Unreleased: request proofs refuse a window a node would refuse

- **A request-proof window is capped at `maxRequestProofTTLSeconds` (300)**,
  core's `MAX_REQUEST_LIFETIME_SECS`. `createProofSigner` (`ttlSeconds`),
  `signRequestProof` and `createProofAuthorizer` (`validForSeconds`) throw
  `invalidInput` for 0 or anything over 300, instead of minting a proof every
  node refuses. The window is what bounds replay: a two-link proof names no
  node (SEC-14).

## Unreleased: core 0.11.0, namespace ops signed at schema 27

- **`NamespaceOps.schemaVersion` is 27**, core 0.11.0's
  `SIGNED_NAMESPACE_OP_SCHEMA_VERSION` (calimero-network/core#4625; no layout
  change from 26). A 0.11.0 node refuses a join signed at 26, so an account
  join against it failed; Mero Chat's live tests on calimero-network/apps#622
  hit this. mero-js made the same change in its #292.
- CI and the merobox scenarios are pinned to core `0.11.0` (`ci/core-version`).

## Unreleased: `GroupInfo.effectiveVisibility`

- **`GroupInfo.effectiveVisibility`** (`"open"` / `"restricted"`) says who
  outside a group can reach it. Draw privacy (a folder lock) from it, not from
  `subgroupVisibility`: an Open folder inside a private one takes the private
  folder's members and is closed to the rest of the namespace, yet its own
  `subgroupVisibility` reads `"open"`. `nil` from a node that predates the
  field (core#4640).

## Unreleased: the sample app on 0.2.0's error APIs

- The sample's node sign-in words its errors from `LoginFailureReason`: a
  lockout ("Too many failed attempts. Try again in N s.") no longer reads as a
  wrong password, and a URL with no login behind it says so.
- Chat errors show a contract's own reason (`MeroError.callFailureReason`)
  instead of the bare `FunctionCallError`.

## Unreleased: typed login failures, and error words with no vendor in them

- **`LoginFailureReason`** classifies what a login threw:
  `.invalidCredentials`, `.rateLimited(retryAfter:)`, `.unreachable`,
  `.notFound`, `.serverError(status:)`, `.other`. `MeroClient` publishes it as
  `loginFailure`, next to `errorMessage`, after a failed
  `login(nodeURL:username:password:)`.
- **`Mero.authenticate` keeps a `429` and a `404` as `MeroError.http`.** Every
  `4xx` became `.authenticationFailed`, so a lockout after failed attempts
  (core answers `429` with `Retry-After`) read as "check your credentials", and
  so did a URL with no login behind it. Other `4xx` are still
  `.authenticationFailed`. A caller matching `.authenticationFailed` for those
  two now sees `.http` (status `429` / `404`).
- **`MeroClient`'s messages name no vendor.** A network failure read "Can't
  reach Calimero: …"; it is "Can't connect to the server: …" now. A lockout
  reads "Too many failed attempts. Try again in N seconds.", and a `404` from a
  login "No sign-in found at that address — check the node URL."
- **`MeroClient.errorMessages`** replaces the built-in words for any error
  (return `nil` to keep one), to rebrand or localise; the built-in ones are
  `MeroClient.defaultMessage(for:)`.

Found by apps built on the SDK, which could not tell a lockout from a wrong
password, and showed the SDK's vendor name in their own error text.

## Unreleased: password sessions that survive a relaunch, and an offline state

- **`MeroClient.login(nodeURL:username:password:)` is a session like an SSO
  login.** It stored nothing, so `restore()` had no node to go back to and a
  password session was lost on every relaunch. It now writes
  `MeroAppStorage.nodeURL` and `tokenNodeURL`, follows the node's event stream
  for `isOnline` (as `connectToNode` does), drops a trailing `/` from the URL,
  and uses `MeroProviderConfig.timeout`. A failed login stores nothing.
- **`login(..., packageName:)`** looks the application up on the node
  (`listApplications`, by package; the latest installed version) for
  `applicationId`. It defaults to `MeroProviderConfig.packageName`; with
  neither, `applicationId` is left as it was.
- **`MeroClient.resumeNodeSession() -> NodeSessionRestore`** tells a node that
  could not be reached from one that refused the token: `.restored`,
  `.offline`, `.rejected` or `.noSession`. On `.offline` the held tokens are
  adopted: `isAuthenticated` is `true`, `isOnline` is `false`, and the new
  `isOfflineSession` is `true` until the event stream connects. A node that
  later rejects the tokens logs out, as for any session. `restoreNodeSession()`
  keeps its answer (`false` for both).
- **`TokenValidation.isUnreachable`**: `AuthApi.validateToken` reported a
  network failure or a `5xx` as an invalid token. It still returns
  `valid: false` (and `status: 401` for a network failure), but now says the
  node never judged the token. A `5xx` keeps its own status.
- The health stream no longer logs out when re-validating after a `401` finds
  the node unreachable; it waits for the stream to reconnect.

Found by apps built on the SDK, which had to build their own fallback client to
keep a password session across launches and offline.

## Unreleased: a contract's reason, one property away

A contract that returns an error or panics answers
`{"type":"FunctionCallError","data":"<reason>"}`, with no `message`, so the
reason an app wants to show sat in `RpcError.data`.

- **`RpcError.reason`**: the reason in `data` (a string, or the text of a
  nested `{type, data}` / `{message}` object), else `message`.
  `RpcError.isFunctionCallError` and `RpcError.functionCallErrorType` name the
  case.
- **`MeroError.callFailureReason`**: the same from a JSON-RPC error and from a
  `/query` or `/intents` refusal (`ErrorRefusal.data`, else its `error`).
- `RpcError.message` for an error with object-shaped `data` now ends with that
  text (`"FunctionCallError: HostError: Panic: …"`), not the bare type. A
  string `data` already read `"FunctionCallError: <reason>"`.

Found by apps built on the SDK.

## Unreleased: testkit routes and transport errors

`MeroKitTestSupport` covers more of what an app's own tests need.

- **`MockURLProtocol.Stub.transportError(_:delay:)`** (and a new `error:`
  parameter on `Stub`) fails a request with a `URLError` and no HTTP response,
  which reaches the SDK as `MeroError.network`. Apps built on the SDK had been
  using a status of `0` as a stand-in, and that is an HTTP response, not an
  offline phone.
- **`FakeNode`** now serves, in core's wire shapes and behind the same auth guard:
  - `POST /admin/client-key`, which mints a scoped token pair the node then
    accepts (`clientKeys` lists what was asked for);
  - contexts: list, create, get, delete, and list for one application
    (`addContext(id:applicationId:groupId:)` seeds one);
  - applications: list and get (`addApplication(id:package:version:)`);
  - blobs: upload (the raw body, byte for byte), list, download, `HEAD` and
    delete (`putBlob(_:)` and `blob(_:)`);
  - group, member and context metadata: `PUT` and `GET`, where an omitted
    `name` keeps the current one.
  - `goOffline(_:)` / `goOnline()` fail every request at the transport.

  The routes it already served behave as before; a new node still lists no
  contexts.
- **A blob regression test.** All 256 byte values go out on the wire unchanged
  on upload and come back unchanged from `getBlob`. The Kotlin SDK once
  corrupted blobs by sending their bytes through a text encoding. Swift keeps
  them as `Data` throughout, and the test now pins that.

## Unreleased: every LoginView word and the logo tile through LoginBranding

`LoginView` could not be fully rebranded: "Manually enter node URL", "Use
Calimero Cloud instead", "Opens the Calimero wallet to approve this device",
the custom-wallet note and every string of the node panel were hardcoded, and
the logo tile's size, radius, fill and shadow were fixed. Apps built on the SDK
with their own brand had to fork the view to change them.

`LoginBranding` now carries all of them, each defaulted to what the view showed
before, so nothing changes for an app that sets none:

- `cloudHint` (`nil` hides the line under the Cloud button), `manualNodeLink`,
  `useCloudLink` and `customWalletNote`;
- `node: NodeLoginStrings`: the node panel's searching line, manual option,
  "local" tag, field labels and placeholders, and its two buttons;
- `logoTileSize`, `logoTileCornerRadius`, `logoTileBackground` (the theme's
  `primary` when `nil`) and `logoTileShadow`.

`AccountSignInPanel` takes `hint:` and `customWalletNote:`, and
`NodeLoginPanel` takes `strings:`, all defaulted, so existing calls compile
unchanged. The glow behind the hero stays `MeroTheme.showsGlow`.

## Unreleased: core 0.11.0-rc.90, namespace ops at schema version 26

The SDK now targets core `0.11.0-rc.90` (`ci/core-version` and the merod image
in every merobox scenario). Core rc.90 raised `SIGNED_NAMESPACE_OP_SCHEMA_VERSION`
to 26 (calimero-network/core#4605): a device link's scope is now judged at the
link's cut. The op layout is unchanged, but a node refuses any other version, so
`NamespaceOps.signMemberJoinOp` at 25 failed against an rc.90 relay with
`schema version mismatch: expected 26, got 25`. `NamespaceOps.schemaVersion` is
26 now, matching mero-js (its #290). Found by Mero Chat's live tests against an
rc.90 relay (calimero-network/apps#597).

No route this SDK calls changed between rc.89 and rc.90. A join refusal now
carries a code (core#4610), and `/admit` is served without a credential
(core#4611), which the SDK already sends none of.

## Unreleased: core 0.11.0-rc.89

The SDK now targets core `0.11.0-rc.89` (`ci/core-version` and the merod image
in every merobox scenario). No route this SDK calls changed between rc.83 and
rc.89. The two changes that touch the wire were already taken:

- request proofs sign the path and query under `calimero.auth.request.v2`
  (core#4549);
- namespace ops are signed at schema version 25 (core#4552, rc.85).

New: `TeeAttestRequest.eventLog` asks `tee/attest` for the TD event log, which
comes back as `TeeAttestResponseData.eventLogB64` (core#4591). A node on a mock
TEE answers `400`. Listing a namespace's revoked and withdrawn devices
(core#4589) was already wrapped.

## Unreleased: sign namespace ops at schema version 25

`NamespaceOps.signMemberJoinOp` signed at schema version 24, so every node on
core 0.11.0-rc.85 or later (calimero-network/core#4552) refused the join with
`signed_op signature does not verify: schema version mismatch: expected 25,
got 24`: a keyholder could not join a namespace from an invitation through a
current relay. `NamespaceOps.schemaVersion` is 25 now, matching mero-js (its
#261). Found by Mero Chat's live tests against a real relay.

## Unreleased: drop the warrant-nonce routes

### Breaking

- **The warrant-nonce routes are gone.** Core serves neither
  `GET /admin-api/contexts/{ctx}/warrant-nonce/{deviceKey}` nor
  `POST /admin-api/contexts/{ctx}/warrant-nonce`, so every call could only
  fail against a current node (mero-js dropped the same API). Removed:
  - `AdminApi.getWarrantNonce(_:authorDeviceKey:)`,
    `AdminApi.getWarrantNonceAsAuthor(_:authorProof:)` and `WarrantNonceState`;
  - `RelayClient.warrantNonce(contextId:)`, `RelayClient.recoverNonce(contextId:)`
    and `RelayWarrantNonceState`.
- **`RelayClient.execute` no longer retries a nonce refusal.** It used to ask
  the node where the sequence stood and retry once. A refusal that names the
  nonce now throws `AccountError.intentRefused` with `retryable: true`, as
  mero-js does; the `WarrantNonceStore` already moved past the refused nonce
  when it handed it out, so calling again signs a fresh warrant. Persist the
  store (`UserDefaultsWarrantNonceStore`): it is the only record of where the
  sequence stands. `WarrantNonceStore`, `MemoryWarrantNonceStore`,
  `UserDefaultsWarrantNonceStore` and `advance(relay:to:)` are unchanged.
- The sample app's explorer drops its `relay.nonce` (`warrantNonce`) entry.

## Unreleased: mero-js parity

### Breaking

- **SSO callbacks must answer a login this app began.** `buildAuthLoginUrl`
  now records a random, single-use `auth_state` (alive 10 minutes) in an
  `AuthLoginStateStore` and adds it to the callback URL's query;
  `parseAuthCallback` returns `nil` for a callback without a pending state, and
  consumes the state so a replay is refused (port of mero-js 25.3.2). Before,
  any URL with `#access_token=…` was adopted — anyone who could open the app
  could choose its session. Build the login URL with `buildAuthLoginUrl` before
  parsing; for a callback that can arrive after a relaunch, pass the same
  `AuthLoginStateStore.keychain()` (or any `AnyValueStore<PendingAuthLogins>`)
  to both calls. A host that injects tokens itself can pass
  `ParseAuthCallbackOptions(allowUnsolicited: true)`.
- **The login URL is form-encoded as mero-js encodes it**: `callback-url`,
  `permissions` and `registry-url` are percent-encoded (`myapp%3A%2F%2Fcb`),
  byte-identical with `URLSearchParams`.
- **Stored tokens are bound to their node.** `Mero` persists the bundle with
  `TokenData.nodeOrigin` (origin + path prefix, `TokenData.nodeKey(for:)`) and
  ignores a stored bundle for another node — or one with no `node_origin`,
  i.e. every bundle saved by an earlier release. **Sign in once after
  upgrading.** `nodeOrigin` is optional, so old bundles still decode.
- **The transport no longer retries.** Every request — a `POST`/`PUT`/`PATCH`/
  `DELETE` included — used to be sent up to three times on a network error or
  `5xx`/`429`, which could repeat a write that had already run. It is now sent
  once, as in mero-js; `401 token_expired` still drives one refresh and one
  replay. Wrap idempotent calls in the now-public `withRetry` to retry.
- `Mero.close()` now ends the event streams the instance started (it was a
  no-op), and so does `logout()`, which calls it.
- `AccountError` has new cases, `cloudNotReady`, `linkDeclined`,
  `notForAccount(method:)` and `noRelay(method:)`: an exhaustive `switch` over
  it needs them. `RpcTransportKind` gains `.relay`.
- **`AccountJoin.bootstrapFromInvitation` no longer throws**: it returns a
  `BootstrapResult` (`.joined(JoinOutcome)` / `.failed(BootstrapFailure)`), and
  every failure names its `BootstrapStep` (`admitters-lookup`, `no-nodes`,
  `not-invited`, `invited-unreachable`, `no-relay-url`, `sign`, `admit`,
  `no-credential`) with mero-js's reason and the HTTP status. `.get()` throws
  the failure. `resolveRelay` and `CloudSignIn.join` throw a `BootstrapFailure`
  (`no-credential` when nobody is signed in) instead of `AccountError`.
- **An invitation's unsigned admitter address is never a relay** (mero-js
  98d6e6e). A namespace the cloud routes to no node is `no-nodes` for the
  claimant, and `CloudSignIn.createNamespaceInvitation` refuses it as
  `invitationNotClaimable(…, "not-hosted")` instead of carrying the session's
  relay in `admitter_addrs`: that address sits outside the inviter's signature,
  so a host proxying a genuine relay could have been joined and logged in through.
- `CloudSignIn.foundNamespace` attests the founding relay now, even when its
  key is known: the founding relay holds the namespace's keys.
- `CloudClient.haRefusalMessages` takes mero-js's wording (`HA_REFUSAL_MESSAGES`).
- **Relay node keys are verified by quote.** The default `RelayKeyVerifier`
  (`CloudConfig`, `DelegatedEnvironment`, `RelayNodeKey.attest`) is now
  `DcapRelayKeyVerifier`: the relay's TDX quote is verified on the device
  against Intel's root and the signed mero-tee release the relay names
  (`locked-read-only`, at least 2.3.99), as mero-js does. A hosted relay
  answering a mock quote, or a quote of an untrusted image, no longer yields a
  key; a loopback relay's mock still does. Pass `TLSRelayKeyVerifier()` for
  the old TLS-only check.
- **Delegated sessions seal a hosted relay** (`DelegatedEnvironment.sealRelays`,
  `CloudConfig.sealRelays`, both `true` by default): the delegated client,
  its relay login and delegated governance go through
  `DelegatedAccounts.relayTransportSession(_:env:)`, sealed to the relay's
  attested TD. Set `sealRelays: false` for a plain rig.
  `DelegatedEnvironment(relayKeyVerifier:)` is now optional (`nil` = the
  default verifier over its `urlSession`).

### Added

- `withRetry(attempts:sleep:_:)` and `isRetryable(_:)` are public: opt-in
  backoff retry (base 250 ms, ±20% jitter) that honours `Retry-After`.
- `classifyError(_:) -> ClassifiedError` (`kind: ErrorKind`, `status`,
  `retryable`, `message`) over `MeroError`, `HTTPError`, `AccountError` and
  `URLError`, plus `HTTPError.explanation` (port of mero-js `classifyError`).
- `CapabilityPresets` (`none`, `joinOnly`, `contributor`, `collaborator` = 0,
  4, 7, 231; `byName`), `Capabilities.capMask(_:)` (names and/or bits via
  `CapabilityRef`; throws on an unknown name), `Capabilities.capNames(_:)`,
  `Capabilities.all`, `Capabilities.bit(named:)`.
- `NodeDiscovery`: `defaultLocalNodePorts`, `localNodeURL(port:)`,
  `nodeEndpoint(_:path:)`, `probeNodeHealth(_:timeout:session:)` and
  `discoverLocalNodes(ports:timeout:session:)` (parallel, per-probe timeout,
  cancellable) — port of mero-js `nodeDiscovery.ts`.
- Typed event streams, like the Kotlin SDK: `Mero.groupEvents(groupIds:)` →
  `GroupEvent`, `Mero.nodeEvents(contextIds:groupIds:)` → `NodeEvent`
  (`.context` / `.group`); the same on `SseClient`.
- `AdminApi.listNamespaceRevokedDevices(_:)` → `NamespaceRevokedDevices`
  (`GET /admin-api/namespaces/:id/revoked-devices`) and `AuthApi.getLoginPage()`
  (`GET /auth/login`), both served by core.
- **`SseConnection`**, one long-lived SSE connection (port of mero-js
  `SseClient`; `Mero.sse` is the instance's shared one):
  `subscribe(contextIds:groupIds:)` / `unsubscribe(...)` on the live session
  (`POST /sse/subscription`, 10 s timeout, only the ids that changed),
  `isSubscribed`, re-subscription of every remembered id after a reconnect
  before `.connect(sessionId:)` is announced, `updates()` / `events()` /
  `on(_:)`, and `presenceSeed(_:)`. `SseClient`'s per-call streams now run on
  it, and `SseClient.connection()` hands one out. A `401` naming
  `token_revoked` / `token_reuse` is now terminal too, like a `403`; other
  refused connects surface as `MeroError.http` (they were `.network`).
- **`ReconnectBackoff`** replaces the fixed 3 s SSE reconnect: 3 s first (as
  mero-js's `reconnectDelayMs`), doubling to 30 s while reconnects keep
  failing, ±20% jitter, reset once a stream opens. `.fixed(3)` is mero-js's
  flat delay; `SseClient`/`SseConnection` take `backoff:`.
- **Typed events**: `AppVersionChangedEvent`, `GroupMembershipEvent`
  (`MembershipChangeKind`), `GroupMigrationEvent` with `MigrationStartedData`,
  `MigrationProgressData`, `CascadeProgressData`, `MigrationCompletedData`
  (core's field names); `ContextEvent.appVersionChanged`, `.data`,
  `GroupEvent.membership`, `.migration`, `.data`; streams
  `appVersionChanges`, `migrationEvents`, `membershipEvents` and handlers
  `onAppVersionChanged` / `onMigrationEvent` on `SseConnection`, plus
  `Mero`/`SseClient` `appVersionChanges(contextIds:)` and
  `migrationEvents(groupIds:)`.
- **`WsClient`** (experimental, as in mero-js; `Mero.ws`): `GET /ws` with the
  token in an `Authorization: Bearer` header on the upgrade request (as SSE
  and the Kotlin SDK send it; core reads the header before `?token=`, and a
  header stays out of URL logs), `subscribe`/`unsubscribe` messages,
  re-subscribe after reconnect, backoff 1 s · 2ⁿ ≤ 30 s, over a
  `WebSocketTransport`/`WebSocketConnector` abstraction
  (`URLSessionWebSocketConnector` by default; `open(_:)` takes the upgrade
  `URLRequest`). `tokenInQuery: true` sends mero-js's `?token=` instead, for
  a node or proxy that drops the header.
- **Ephemeral presence**: `Mero.ephemeral` → `EphemeralClient`
  (`set(contextId:state:codec:)` via JSON-RPC `set_ephemeral`,
  `subscribe(contextId:codec:)`), `Codec` / `JSONCodec`, `EphemeralEntry`
  (`author`, `account`, `state`, `removed`, `ageMs`), `presenceStream(_:...)`
  seeded from the connection's kept presence. `RpcClient.call(_:params:)` for
  any JSON-RPC method.
- **Relay presence**: `PresenceStatement` (`calimero/presence/1` statement
  bytes and `stateHash`, pinned to core's vector; `signedIntent`),
  `RelayClient.presenceIntent(contextId:state:seq:sentAtMs:)` (signs with the
  device key), `AdminApi.postPresenceIntent(_:state:seq:sentAtMs:keys:authorProof:)`,
  and `RelayPresenceClient` (resends every 2.5 s, `set(nil)` retracts,
  `close()`), on every relay `CloudConnection` as `presence`.
- `MockURLProtocol.Stub(holdOpen:)` keeps a response open after its body — a
  live `/sse` in tests.
- `MeroKitTestSupport` is a library product, so apps can use `FakeNode` and
  `MockURLProtocol` in their own tests. It is not part of `MeroKit`, and the
  podspec does not ship it.
- **Signers** (port of mero-js `signer.ts`): the `Signer` protocol (public key
  + `sign(_:)`), `SeedSigner`, `signerFromSecret(_:label:)`,
  `resolveSigner(_:_:label:)`. Every new signing entry point takes a hex secret
  or a `Signer`, never both.
- **Account roots** (port of mero-js `account/account.ts` + `wordlist.ts`):
  `bip39EnglishWordlist` (pinned to its canonical SHA-256),
  `generateAccountRoot()`, `accountRootFromPhrase(_:)`,
  `accountRootFromSecret(_:)` — the 24 words are BIP39 *entropy* used as the
  Ed25519 seed, no PBKDF2 or BIP32, exactly as mero-js;
  `createAccountRootSigner(vault:)`, `accountRootSignerFromPhrase(_:vault:)`,
  `loadAccountRootSigner(from:)` over a `RootSeedVault`
  (`KeychainRootSeedVault`: `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`,
  never synchronized; `MemoryRootSeedVault`) whose `VaultRootSigner` reads the
  seed per signature and never returns it; `RootSource`, `resolveRoot(_:)`,
  `resolveRootPair(rootSecret:signer:)`; `signAccountLink(_:)` /
  `signAccountLogin(_:)` (mdma's domains, standard base64). Types
  `AccountRoot`, `RecoverableAccountRoot`, `AccountRootSigner`,
  `NewAccountRootSigner`, `ResolvedRoot`, `AccountLinkInput`, `AccountLoginInput`.
- **Owner-op root proofs** (port of mero-js `owner-op.ts`, pinned to core's
  `owner_op.rs` and `root_guarded_vectors_are_stable` vectors): `OwnerOpKind`
  (raw value = the signed byte), `OwnerOpPlane`, `OwnerOp`,
  `transferOwnershipOp`, `groupDeleteOp`, `adminChangedOp`,
  `teeAuthoringPolicyOp`, `teeAdmissionPolicyOp`, `teeReleaseAdmissionPolicyOp`,
  `ownerOpDigest`, `ownerOpSigningPayload`, `signOwnerOpProof(_:)`
  (`OwnerOpProofInput`, handoff `chain` and `keyEpoch`), `rootGuardedOpBytes`.
- **The SDK mints `rootProof` itself**: `AdminApi.transferOwnership(_:newOwner:root:)`,
  `changeNamespaceAdmin(_:newAdmin:root:)`, `ownerDeleteGroup(_:root:)`,
  `setTeeAuthoringPolicy(_:allowedMrtd:root:)`, `disableTeeAuthoringPolicy(_:root:)`
  and `setTeeAdmissionPolicy(_:request:root:)` read `namespaceId` and
  `ownerOpCounter`, build the op the node builds and send its proof
  (`ownerOpTarget(_:plane:)`, `signRootProof(_:op:root:)`,
  `SetTeeAdmissionPolicyRequest.ownerOp()`, `normalizeTeeReleaseVersion`). The
  raw-`rootProof` forms are unchanged.
- **Device scopes** (port of mero-js `device-scope.ts`, core vectors):
  `signDeviceScope(_:)`, `deviceScopePayload`, `DeviceScopeInput`, and
  `AdminApi.linkAccountDevice(_:credential:scope:)`, which signs the scope.
  `rescopeDevice` is unchanged: core signs that scope on the root's own node.
- **Request proofs** (port of mero-js `request/*`, `request-proof/*`, pinned to
  core's `request_wire_fixture.rs` / `proof_auth.rs`): `requestBodyHash`,
  `signRequest` (`RequestSigInput`), `callerProof` (`CallerProofInput`),
  `createProofSigner` (`ProofSignerOptions`, 120 s), `signRequestProof`
  (`RequestProofInput`, 300 s), `requestProofHeader` (`X-Calimero-Proof`),
  `createProofAuthorizer(_:)` (`ProofCredential`), `signedPath(_:)`,
  `carriesProof(_:)`, `ProofRequest`, `ProofCallback`, `RequestAuthorizer`.
  `MeroConfig.requestProof` (`RequestProofConfig`: `.keys`, `.custom`,
  `.authorizer`) and `TransportHooks.getProof` / `.authorizeRequest` sign every
  `auth`/`admin`/`rpc` request over the URL fetched and its exact body. **A
  request carrying a proof never follows a redirect**: the `3xx` surfaces as
  `MeroError.http` with status `0` and is not re-sent (as mero-js
  `refuseProofRedirect`). A file body cannot be signed and is refused.
- **Cloud login sessions** (port of mero-js `CloudClient`'s signed-in half):
  `CloudClient(sessionToken:sessionStore:onSession:)`, `signInWithGoogle(_:)`,
  `signInWithAccount(_:)` (root-signed, `POST /api/auth/account`),
  `getAccountLoginChallenge()` / `submitAccountLogin(_:)`, `refreshSession()`,
  `signOut()` (forgets the session even when the server call fails),
  `getSession()`, `isSignedIn`, `CloudClient.keychainSessionStore(...)`. The
  MDMA token (`CloudLoginSession`, `CloudUser`) is shared by every copy of the
  client and adopted from `x-mdma-session-refresh` on any response; a
  signed-in route with no session throws `AccountError.notSignedIn` unsent.
- **Owned namespaces, relays, machines**: `getMyNamespaces()`
  (`CloudNamespace`), `getNamespaceRelays(_:)` (`CloudRelay`, with
  `admission`, `admissionHint` and the relay's own `CloudRelayJoin` report),
  `findExecutingRelay(_:)`, `CloudClient.pickExecutingRelay(_:)`,
  `getMyMachines()` (`CloudMachine`, `CloudMachineNamespace`). mero-js's
  deprecated `getMyNode` is not ported.
- **HA**: `claimNamespace(_:ownershipProof:)` (or `proof:` for a typed node
  proof), `enableNamespaceHa(_:admitterAddrs:)`,
  `enableFoundedNamespaceHa(_:ownershipProof:)`, `disableNamespaceHa(_:)`,
  `admitterAddrsFromNetworkStatus(_:)`, the cloud's page URLs
  `enableHAURL(_:)` / `disableHAURL(_:)` (`HAPageOptions`), and in MeroKitUI
  `CloudClient.enableHA(_:callback:authenticator:)` /
  `disableHA(_:callback:authenticator:)`, which open them in the system auth
  sheet. `signAccountOwnershipClaim(_:)` (`AccountOwnershipClaimInput`,
  audience `AccountHaClaim.ownershipAudience` = `mdma:enable-ha-namespace`,
  bound to the login's email) and `signAccountHaClaim(_:)`
  (`AccountHaClaimInput`), each from a device secret or a `Signer`.
  `HaRefusal` (`accountNotLinked`, `accountLinkedToSeveralUsers`,
  `haRequestPending`, `unknownRelay`, `relayNotDialable` — mero-js's typed
  refusal errors) and `AccountError.haRefusal` read an `haRefused` back.
- **Account linking**: `linkAccount(_:)`, `getAccountLinkChallenge()` /
  `submitAccountLink(_:)` (`AccountLinkChallenge`, `AccountLinkProof`,
  `CloudAccountLink`), `linkAccountWithGrant(grant:rootSecret:signer:)`
  (anonymous even with a session), `getMyAccounts()` (`CloudLinkedAccounts`),
  `CloudClient.accountLinkHandoff(portalURL:accountId:callbackURL:stateStore:)`
  / `readAccountLinkCallback(_:stateStore:)` (`AccountLinkHandoff`,
  `AccountLinkCallback`), and the consent-page flow
  `CloudLink.cloudLinkUrl(accountId:returnTo:cloudWebURL:stateStore:)`,
  `readCloudLinkCallback(_:stateStore:)`, `completeCloudLink(_:root:...)`,
  `strippingCallback(_:)`, with MeroKitUI's `CloudLink.beginCloudLink(...)`
  presenting it. A grant is accepted only for a link this app began, for that
  account, within 10 minutes, once (`CloudLinkStateStore`, in memory or
  `.keychain()`, in place of `localStorage`). `callbackIsForeign(_:pageOrigin:)`.
- **`connectCloud(_:)` / `connectCloudWithAccount(_:)`** (port of mero-js
  `connect.ts`; `ConnectCloudOptions`, `ConnectCloudWithAccountOptions`): sign
  in, pick the owned namespace and the relay that can write there, and return
  a `CloudConnection` — which gains `cloud`, `namespace`, `namespaceId`,
  `relayInfo` and `execute(contextId:method:argsJson:)`. Its relay session is
  opened by the same code as `CloudSignIn.connect(_:)`.
- `AccountError.cloudNotReady` (no namespace, several, HA off, nothing
  assigned, only replicas, relays awaiting the grant — mero-js's messages;
  classified `.unavailable`) and `AccountError.linkDeclined` (`.forbidden`).
  `SseConnection(baseURL:authorize:)` and `SseClient(baseURL:authorize:)` sign
  the stream and every subscription `POST`, with the same redirect refusal.

- **Unified transport** (port of mero-js `src/transport/*`):
  `MeroTransportClient` (mero-js `MeroClient`, named apart from MeroKitUI's) over
  `MeroTransportConfig` `.node(NodeTransportConfig)` / `.relay(RelayTransportConfig)`
  with `transport`, `rpc`, `canSubscribe`, `node`, `relay`, `admin`, `auth`,
  `events`, `ws` (throws for a request proof), `ephemeral` (`EphemeralSurface`:
  `EphemeralClient` on a node, `RelayPresenceClient` on a relay), `cloud` and
  `close()`; `MeroTransportError` names what a surface lacks. `ExecuteTransport`
  (`kind`, `canSubscribe`, `execute`, `executeWithMetadata`, `migrateMyEntries`,
  `countMyPending`), which `RpcClient` conforms to as it ships, `MeroRpcTransport`
  and `RelayTransport`. `ExecuteResult` (= `RpcExecuteResult`) gains `rootHash`.
  `RelayObserver` / `RelayObserveConfig`: an `account_proof` session minted
  lazily with the pinned node key, single-flight, re-minted within 30 s of
  expiry, feeding SSE and the WebSocket.
- **Delegated accounts** (port of mero-js `src/account/session.ts`, `govern.ts`,
  `create-context.ts`, `join-as-account.ts`): `DelegatedSession`,
  `DelegatedEnvironment`, and a `DelegatedAccountStore` (`UserDefaults`,
  Keychain or memory; `UserDefaultsValueStore`) holding each account's
  `RelayMap`, its subgroups' namespaces and the relays' pinned node keys.
  `DelegatedAccounts`: `readRelayMap`, `rememberRelay`, `relayForContext`,
  `carryExecutorAccount`, `knownRelays`, `listDelegatedContexts` /
  `listDelegatedNamespaces` (the union across every known relay,
  `DelegatedListing`), `rememberGroupNamespace`, `namespaceOfGroup`,
  `markContextNonceSpent`; `attemptRelayNodeKey` (`RelayNodeKeyAttempt`:
  learned / unavailable / refused), `resolveRelayNodeKey`, `learnRelayNodeKey`
  (retry 1, 2, 5, 10, then every 30 s; cancellable; `nil` on a refusal);
  `buildDelegatedClient` (reads by `admin.queryContext` first, a `409`
  remembered as a write; admin and events by request proof, plus a session
  once the relay's key is pinned; inert events until then on a hosted relay;
  `nil` without a relay); `createDelegatedContext`,
  `createDelegatedPrivateContext`, `foundDelegatedNamespace`,
  `delegatedGovernance`, `governGroup`, `governRoot`, `latestPublishedVersion`
  and `joinAsAccount`. `CloudSignIn` gains `delegatedStore`,
  `delegatedEnvironment` and `delegatedSession()`, and records the relay a join
  or a founding used.
- **One admin surface** (port of mero-js `account-admin.ts`, `node-admin.ts`):
  `AdminSurface`, which `AdminApi` conforms to; `AdminApiForwarding`;
  `AccountAdmin` (`createAccountAdmin`, `AccountAdminDeps`, `AdminAppConfig`,
  `AccountInvitationInput`, `mintInvitation`) — reads from the relay, writes as
  governance ops, founding and device-signed invitations, node-only calls
  refused by name, an account with no relay a member of nothing — and
  `NodeAdmin` (`createNodeAdmin`), which installs the app from the registry
  before `createNamespace` and checks the installed id.
  `CreateNamespaceResponseData` gains `haEnabled` / `haError`.
  `InvitationNotClaimableReason` and `AccountError.invitationNotClaimableReason`.
  `CloudClient.haAccountNotLinkedMessage`.
- **Invitation redemption** (port of mero-js `admin-api/redeem-invitation.ts`):
  `redeemInvitation(_:redeemer:)` (`RedeemRequest`, `InviteRedeemer`),
  `RedeemOutcome` (`joined`, `alreadyMember`, `failed`), `InviteFailureReason`,
  `isSettled` / `shouldRetain`, and `AdminSurface.redeemInvitation(_:invitation:groupName:teamName:)`.
  `AccountJoin.resolveRelayFromInvitation`, `joinWithNode(nodeURL:admitURL:…)`
  (`ResolveRelayResult`, `JoinWithNodeResult`) and `InvitationRelay.peerId`.

- **TEE trust** (port of mero-js `src/sealed/verify.ts`, `release.ts`,
  `sigstore.ts`, `sigstore-trust-root.ts`, `x509.ts`, and of `@phala/dcap-qvl`'s
  `verify`): `Dcap.verify(_:collateral:nowSecs:)` (`DcapCollateral`,
  `DcapVerifiedReport`, `DcapReport`, `TdReport10`, `TdReport15`,
  `SgxEnclaveReport`, `DcapVerify`) — Intel's PCK chain, CRLs and revocation,
  TCB info, QE identity, the QE report and the quote's signatures, on
  CryptoKit's P-256/P-384 ECDSA and an internal DER/X.509 reader;
  `createQuoteVerifier(_:)` (`QuoteVerifierOptions`, `QuoteVerifier`,
  `TrustedMeasurements`), `trustedMeasurementsFromReleases(_:profile:allowDebugProfile:)`
  (`PublishedMrtds`, `PublishedProfile`, `TrustedReleaseOptions`),
  `TeeTrust.productionProfile`; the `TransportQuoteVerifier` protocol
  (`TransportQuoteArgs`, `AnyTransportQuoteVerifier`); `TeeTrustError`.
- **Signed node releases**: `Sigstore.verifyCosignBundle` with the embedded
  `Sigstore.fulcioAuthorities` / `Sigstore.rekorLogs` (`SignerIdentity`),
  `verifySignedNodeRelease`, `fetchNodeRelease`, `nodeReleaseVersionFromImage`,
  `fetchNodeReleaseVersion`, `cloudNodeReleaseUrl`, `trustSignedRelease`
  (`TrustSignedReleaseOptions`), `createSignedReleaseVerifier`
  (`SignedReleaseVerifierOptions`, `SignedReleaseVerifier`),
  `createSignedReleaseSealedFetch` (`SignedReleaseSealedFetchOptions`,
  `NodeReleaseSource`), `TeeTrust.nodeReleaseSigner`,
  `TeeTrust.defaultReleaseMirror`. Pinned to real mero-tee 2.3.86/2.3.87
  releases (`Fixtures/tee/`).
- **Sealed transport** (port of mero-js `sealed.ts` / `noise.ts`, pinned to
  core's `sealed.rs` vectors): `SealedTransport` (`handshakePath`,
  `sealedPath`, `contentType`; `send(_:)` streaming `SealedResponse`,
  `data(for:)`, `urlSession(configuration:)` over `SealedURLProtocol`, URLs
  outside the base refused, one session for concurrent requests,
  `unknown_session` reopened, `busy` retried, re-attest on restart),
  `TransportKeySource`, `SealedTransport.attested(baseURL:verifier:…)` (mero-js
  `createAttestedSealedFetch`), `fetchAttestedTransportKey(_:verifier:…)`
  (`TransportAttestation`), `transportKeyBinding(_:_:)`,
  `StaleTransportKeyError`, `SealedTransportError`.
  `ConnectCloudOptions.seal` / `ConnectCloudWithAccountOptions.seal` seal the
  relay. `DelegatedAccounts.relayTransportSession(_:env:)` /
  `forgetRelaySealing()` (mero-js `relayTransportFetch`), with the
  `sealed_route_unguarded` fallback for logged-in calls.
- **Relay node-key attestation** (port of mero-js `relay-attestation/*`):
  `attestRelayNodeKey(_:)` (`AttestRelayNodeKeyOptions`, `AttestedNodeKey`),
  `attestKeyBinding(_:)`, `reportDataOf(_:)`, `DcapRelayKeyVerifier`
  (`trustedRelayProfile`, `minRelayRelease`, `signedReleaseVerifier(relayURL:…)`),
  `RelayKeyTransportError` (an unreachable relay or mirror is `.unavailable`).
- **Private agents** (port of mero-js `src/agent-gate/*`, against
  mero-agent-gate's `protocol.rs`): `attestAgent`, `checkOwner`,
  `provisionAgent`, `releaseAgent` (`AttestAgentOptions`,
  `ProvisionAgentOptions`, `ReleaseAgentOptions`, `VerifiedAgent`,
  `ProvisionedAgent`), `generateOwnerKey` / `ownerPublicKey` (`OwnerKey`),
  `validateAgentId`, `agentKeyBinding`, `provisionInfo`,
  `AgentGate.attestDomain` / `.provisionInfoDomain`, `AgentGateError`;
  `hpkeSealAuth` / `hpkeOpenAuth` (`HpkeSealed`: RFC 9180 Auth mode,
  X25519 + HKDF-SHA256 + ChaCha20-Poly1305 on CryptoKit, opening a bundle the
  gate sealed); `verifySignedAgentPolicy`, `agentQuoteVerifier`
  (`AgentPolicy`), `TeeTrust.kmsReleaseSigner`.
- **MeroKitUI: the mero-react app layer** (port of `mero-react/src`).
  - Provider state on `MeroClient` (`MeroProvider`/`useMero`):
    `MeroProviderConfig` (`AppMode` `multiContext`/`admin`, deprecated
    `singleContext`; `packageName`, `packageVersion`, `registryURL`,
    `cloudBaseURL`, `allowedNodeURLs`), `permissions(for:)`
    (`getPermissionsForMode`), `connectToNode(_:callbackScheme:)` /
    `handleAuthCallback(_:)` (the node's login page in the system auth sheet;
    the callback's `node_url` checked by `NodeTrust.resolveTrustedNodeURL`, its
    tokens adopted only when `TokenAdoption.resolve` says so),
    `restore()` / `restoreNodeSession()` (`/auth/validate`, judged on a token
    the transport rotated), `connectWithAccount(_:)`, `isDelegated`, `can`
    (`MeroCapabilities`), `admin` (`NodeAdmin` / `AccountAdmin`), `transport`,
    `readAdmin`, `isOnline` over the node's SSE stream (logout on a revoked
    token family or `403 permission_denied`, recovery on `401`),
    `applicationId` (resolved from the registry or the account's contexts for
    an account), `contextId` / `contextIdentity`, `selectContext(_:identity:)`,
    `app`, and a `logout()` that clears everything. `MeroAppStorage` keeps the
    node URL, token node URL and ids in `UserDefaults` under mero-react's keys.
  - Hooks: `AsyncResource` (latest request wins, reset on input change,
    re-bound on session change) and `AsyncMutation` (`failure`,
    `throwOnError`); reads `contexts(applicationId:)`, `applicationContexts`,
    `contextGroup`, `groupInfo`, `groupMembers`, `groupContexts`, `subgroups`,
    `defaultCapabilities`, `subgroupVisibility`, `teeAdmissionPolicy`,
    `groupUpgradeStatus`, `groupMetadata`, `memberMetadata`, `contextMetadata`,
    `namespaces`, `namespace`, `nodeIdentity`, `namespacesForApplication`,
    `namespaceGroups`, `blobInfo` (`isBlobNotFound`); writes `ContextActions`
    (incl. `createPrivateContext`, `resyncContext`), `JoinActions` (incl.
    `redeem`), `GroupActions` (members, roles, capabilities, visibility, TEE
    admission, reparent, detach, legacy invitations, `joinGroup`),
    `MetadataActions`, `NamespaceActions`, `UpgradeActions`
    (`installFromRegistry`), `BlobActions`, `GroupCapabilities`; live
    `ContextExecutor`, `EventSubscription`, `ContextDiscovery`,
    `EphemeralPresence` (authoritative replay, self filtering, throttled
    `setPresence`, `ageOf`), `MigrationEventsWatcher`, `MigrationStatusModel`
    (SSE, `defaultMigrationPollInterval` 5 s fallback), `AppVersionModel`,
    `LatestVersionModel`, `GroupAppVersionModel`, `MyAuthoredMigrationModel`,
    `BlobFileModel` (bytes and a temporary file, removed with the model).
  - Invitations: `InvitationCapture` (`prime`, `handle(_:)` for `onOpenURL`,
    `capture(raw:)`, `onInvitation`, `peek`, `subscribe`; persisted until
    acked, deduplicated by mero-platform's FNV-1a nonce, foreign slugs acked
    and dropped, `maxAutoJoinAttempts` per app key) with
    `primeInvitationCapture`, `onInvitation`, `peekInvitation`,
    `subscribeInvitation`, `invitationFromRaw`, `buildInviteLink`,
    `urlWithoutInvitation`; `InviteRedemption`, `describeInviteFailure`,
    `JustJoinedNamespaces`, `JoinSync`, `JoinInvitation`
    (`isFinalInvitationError`), `DelegatedBootstrap`, `AccountEnrolment`,
    `isReturningFromWallet`.
  - Views: `ConnectButton`, `LoginView` tabs (`LoginModalTab`, `NodeLoginPanel`
    over `NodeLoginModel`, `AccountSignInPanel`), `MigrationPendingBanner`,
    `MigrationAdminPanel`, `InvitationDialog` (`invitationReadyDelay`),
    `InvitationGate`, `JoinSyncBanner`, `CalimeroLogo` / `CalimeroLogoShape`,
    and `MeroTheme` (`defaultMeroTheme`, `.meroTheme(_:)`).
  - Not ported: `useNamespaceIdentity` (core no longer serves the route), the
    browser-only `isDesktopWindow`, and `MigrationStatus.fleetCompletedAt`,
    which MeroKit's `MigrationStatus` does not decode yet.

### Changed

- `LoginView` offers a **Node** tab beside the Cloud one (pass `tabs: [.cloud]`
  for the Cloud sign-in alone) and draws from the environment's `MeroTheme`;
  the Cloud tab is still the default, with the same identifiers.
- `MeroRootView` restores a stored node session after the Cloud one, adopts
  SSO callbacks from `onOpenURL`, and captures invitation links.
- `MeroClient.logout()` also clears `MeroAppStorage`.

- `URLSessionHttpClient` drops a bare trailing `?` from a request URL (as
  mero-js `withoutBareQuery`), so the URL signed and the URL fetched agree.

## Unreleased: Calimero Cloud sign-in

### Breaking

- **Mobile sign-in is Calimero Cloud only.** `LoginView` is now a single
  "Continue with Calimero" button: the wallet opens in the system auth sheet,
  the person approves the device with their passkey, and the app connects to
  the relay that serves their account. The node URL, username and password
  fields are gone.
  - `LoginView(callbackScheme:)` / `LoginView(callback:)` replace
    `LoginView(defaultNodeURL:)`; likewise `MeroRootView`.
  - Accessibility ids: `cloudSignInButton` is the button; `loginButton` now
    identifies the sign-in panel; `loginTitle` and `loginError` are unchanged.
  - `MeroClient.login(nodeURL:username:password:)` remains for development
    nodes, but no shipped view offers it.

### Added

- **Account layer** (`MeroKit`, ported from mero-js 24.5.0, byte-identical with
  core's wire fixtures):
  - `Crypto/`: `domainHash`, a little-endian borsh writer, Ed25519 / X25519 over
    CryptoKit (no new dependencies), canonical JSON for warrant commitments.
  - `Account/`: `DeviceKeys` (Keychain via `AnyValueStore.keychain`),
    `DeviceCertificates` (`parse`/`verify`, port of `verifyDeviceCredential`),
    `DeviceEnrolment` (wallet URL, callback parsing, `completeEnrolment`),
    `NamespaceOps.signMemberJoinOp` (namespace-op schema 24) and `AccountJoin`
    (`bootstrapFromInvitation`).
  - `Cloud/`: `CloudClient` (`getAccountRelays` with routing-proof headers,
    `chooseRelay`, `getNamespaceRouting`, `findAdmitter`) and `CloudSignIn`,
    the end-to-end orchestrator.
  - `Relay/`: `RelayClient` (`describe`/`execute` with warrant v2 and a
    persisted per-relay nonce sequence recovered from `warrant-nonce` on a nonce
    refusal; `query` with 409 → warrant fallback;
    `describeCreation`/`createContext`; `describeGovernance`/`govern`),
    `RelayLogin` (`account_proof` login with a signed login statement, and
    `POST /auth/logout`), `RelayNodeKey` + the pluggable `RelayKeyVerifier`.
- `MeroKitUI`: `SystemWebAuthenticator` (ASWebAuthenticationSession; app scheme
  or iOS 17.4+ https callback), `MeroClient.signInWithCloud(callbackScheme:)`,
  `handleEnrolmentCallback(_:)`, `restoreCloudSession()`, and session state
  (`account`, `relayURL`, `isSignedInWithoutRelay`, `cloudNote`, `connection`).
- Sample app: Calimero light design (system font, SF Symbols, ids behind
  "Show technical details"), Cloud-only sign-in on `mero-sample://enrol`, chat
  over the relay session, and a "Cloud & Relay" explorer category.

### Known gaps

- ~~The relay's node key is taken from its TEE attestation over TLS; DCAP
  quote verification is a follow-up.~~ Done in the mero-js parity release:
  `DcapRelayKeyVerifier` verifies the quote and the signed release.
- The hosted wallet returns only to `https://` callbacks; accepting app schemes
  is a pending wallet change.

## Unreleased: core 0.11.0-rc.83

The SDK now targets core `0.11.0-rc.83` (`ci/core-version`, and the merod image
the merobox workflows pin). This is a breaking release.

### Upgrade notes

- **Log in again after upgrading the node.** rc.83 tokens carry a required
  `key_id` claim, so every access and refresh token an rc.41 node minted is
  refused (`401`). Clear the stored session and call `authenticate` again.
- `await mero.logout()` is now `async` and first calls `POST /auth/logout` to
  retire the refresh token on the node. That call is best-effort; the local clear
  always happens.

### Removed

- `AdminApi.getCertificate()`: core removed `GET /admin-api/certificate`.
- `AdminApi.teeVerifyQuote(_:)`, `TeeVerifyQuoteRequest`,
  `TeeVerifyQuoteResponseData`: the node has not served
  `POST /tee/verify-quote` since before rc.41.
- `CreateGroupRequest.groupId`: group ids are derived, and a body naming one is a
  `400`.

### Changed

- `RpcClient.execute` / `executeWithMetadata` no longer take `executorPublicKey`.
  Core's execute request is `deny_unknown_fields` with only `contextId`,
  `method` and `argsJson`, so a call that named one was refused.
- SSE: a `403` on connect or subscribe now finishes the stream with the
  `MeroError` (`.authRevoked` when `x-auth-error` names a dead token family)
  instead of reconnecting every 3 s forever.

- `createGroupInNamespace` always sends `visibility`, `"open"` when the caller
  names none (`CreateGroupInNamespaceRequest.defaultVisibility`), matching
  rc.83's new default and mero-js. Pass `"restricted"` to keep a subgroup closed.
- `SetTeeAdmissionPolicyRequest` gained the signed-release form (`signedRelease`),
  `mode` (`replica` / `relay`) and `rootProof`, with `.measurement(...)` and
  `.signedRelease(...)` factories. In the measurement form rc.83 requires
  RTMR1–3 as well as MRTD. `GetTeeAdmissionPolicyResponseData` gained `enabled`,
  `signedRelease` and `mode`.
- `TeeAttestRequest` gained `bindNodeKey`, `bindTransportKey` and
  `includeCollateral` (sent only when set); `TeeAttestResponseData` gained
  `boundPublicKey`, `transportPublicKey` and `collateral`.
- SSE sends the bearer token in the `Authorization` header instead of `?token=`.

### Added

- Admin: `queryContext`, `getIntentRelay` (`IntentRelayInfo` with `executorKey`,
  `releaseBytecodeId`, `releaseVersion`), `postPresenceIntent`,
  `getContextIntentRelay` / `createContextIntent`, `getGovernanceIntentRelay` /
  `governanceIntent`, `getWarrantNonce` / `getWarrantNonceAsAuthor` (not served
  by rc.83, which answers `404`), `signWithAccountRoot`, `linkAccountDevice`,
  `sealToAccount`, `listGroupMemberDevices`, `teeRegistrationAttest`.
- Root-guarded owner ops: `transferOwnership`, `changeNamespaceAdmin`,
  `ownerDeleteGroup`, `setTeeAuthoringPolicy`, `disableTeeAuthoringPolicy`, each
  with an optional `rootProof`.
- `Capabilities.canAuthorOnBehalf` (bit 9), with `openToDelegatedExecution` and
  `grantAuthorship`.
- Typed `createGroup(_: CreateGroupRequest)`, and typed
  `issueOwnershipProof` / `issueNamespaceOwnershipProof` returning
  `IssueOwnershipProofResponseData` (with `founding` and `credential`).
- `Namespace.founding` / `heldOps`, `CreateNamespaceResponseData.founding`,
  `GroupInfo.namespaceId` / `ownerOpCounter`.
- Auth: `AuthApi.logout(_:)`; `GenerateClientKeyRequest.applicationId` and
  `ttlSecs`.
- Errors: `HTTPError.refusal`, `MeroError.httpStatus` / `refusal` / `errorType`
  for rc.83's typed refusals (`{error, type, data}`), and
  `RpcError.isReadOnlyWriteRefused` / `refusedContextId`. `MeroError`'s cases
  are unchanged.
- RPC: `executeWithMetadata` (`RpcExecuteResult`).
- SSE: `events(contextIds:groupIds:)` for group-keyed events
  (`ContextEvent.groupId`), and `ContextEvent.presenceAccount` /
  `presenceAuthor`.
- Tests: `Rc83SurfaceTests` against core's own rc.83 wire fixtures
  (`Tests/MeroKitTests/Fixtures/rc83-*.json`).
