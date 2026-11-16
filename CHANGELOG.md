# Changelog

## Unreleased

### The sealed handshake and the directory drop `v2` from their paths (mero-core)

- `SealedTransport` opens a session at `POST /sealed/handshake` (`HANDSHAKE_PATH`). A node
  released before that path answers `404` or `405` there, and the handshake is then sent once
  more to `POST /sealed/v2/handshake` (the new `LEGACY_HANDSHAKE_PATH`), so older nodes are still
  reached. Any other refusal is reported as before.
- `DEFAULT_DIRECTORY` is `https://manager.cloud.calimero.network/api/directory` (was
  `.../api/v2/directory`, which the manager still serves).

### Consent: organization management kinds (mero-core)

- The ten consent page kinds of calimero-network/core#5018 as `ConsentRequest` cases: `LeaveOrg`,
  `RemoveMember`, `SetMemberRole`, `SetMemberTeams` (with `ConsentTeamMembership`),
  `RemoveMemberTeams`, `RevokeInvite`, `RenameOrg`, `DeleteOrg`, `SetCapabilities`, `SetAllApps`.
- Their `ConsentOutcome`s: `OrgRenamed(orgId, name)`, `OrgDeleted(apps)`, and an object for each kind
  the node answers nothing for (`LeftOrg`, `MemberRemoved`, …), so an approval of a known kind never
  reads as `Other`.
- `ConsentRequest.kind` and `ConsentRequest.subject()`: each case writes its own wire subject.
- `ConsentResult.from` (the facade's `consent.pageResult` answer) types the generated `outcome`
  (core c350a92bc's document) instead of dropping it. The request bodies stay hand-written: the codegen
  maps the spec's `oneOf` body to `ConsentRequest` (`codegen/build.gradle.kts`).

### Consent: organizations and invitations, typed outcomes (mero-core, mero-compose)

- `ConsentRequest.CreateOrg`, `ConsentRequest.CreateInvite` and `ConsentRequest.AcceptInvite`:
  the consent page kinds core adds for a package-bound session (calimero-network/core#5014).
- `ConsentResult.outcome`: a `ConsentOutcome` typed by kind (`OrgCreated(orgId)`,
  `InviteCreated(token)`, `InviteAccepted(org)`, `Release`/`Upgrade`/`Outbound`, `Other` for a kind
  this version does not know); `ConsentResult.isDeclined`. An invitation's token is left out of
  `toString`.
- `CalimeroClient.requestConsent` and `consentResult` are public; new `waitForConsent(requestId, timeout)`
  and `resumeConsent(href, requestId)`, which reads the app's own request from the node and throws
  `CONSENT_NOT_STARTED`, `CONSENT_REQUEST_MISMATCH`, `CONSENT_NOT_DECIDED` (still pending after the
  page returned) or `CONSENT_REPLACED` (`404`: a newer request replaced it) when the return is not
  that request's decision.
- `consentReturnTo(origin, version)`: a native app's `returnTo` on Calimero Cloud, the package's
  served page (`{origin}/{version}/?native-consent=1`).
- `ConsentLauncher.resume(client, uri | intent, requestId)` for the link the page returns to.
  `MeroAppSession.handle` checks a return against the request `requestConsent` started: the decision
  in `consentResult`, a refused return in the new `consentError`.

### Breaking: the `NoRelay*` aliases are gone (mero-core)

The deprecated 0.x aliases kept since Calimero 1.0 dropped relays are removed:

| Removed | Use instead |
|---|---|
| `NoRelayException` | `SignedOutException` |
| `NoRelayContextContract` | `SignedOutContextContract` |

## 1.0.0 (unreleased)

### Breaking: the deprecated one-route methods are gone (mero-core)

The `CalimeroClient` methods that 0.x deprecated in favour of the generated facade are removed;
call the facade method each one named (`client.orgs`, `client.apps`, `client.auth`,
`client.packages`, `client.deployments`, `client.consent`, `client.public`, `client.ops`).
Facade methods take named parameters and the generated enums (`com.calimero.mero.api.models.OrgRole.MEMBER`,
`TeamRole.LEAD`, `TeamJoin.EVERYONE`), approve a `412` with the call's `passkey` or the client's
(`CalimeroClient.passkey`), and answer the generated success model.

Kept, because no single operation replaces them: the passkey ceremonies (`signUp`, `signIn`,
`addPasskey`, `refresh`, `signOut`, `signOutEverywhere`), the streams (`events`, `presenceStream`),
`uploadBlob` (a caller's content type), `blobContent`, `attestation`, and the composites
(`appsReportAll`, `pendingApprovals`, `openConsentPage`, `waitForConsent`, `startRecovery`,
`connect`). New: `ConsentResult.from` reads `client.consent.pageResult`'s answer.

| Removed | Use instead |
|---|---|
| `appXcalls` | `` client.apps.xcalls(app = app) `` |
| `requestConsent` | `` client.consent.pageCreate(consentRequest = request) `` |
| `uploadFile` | none: core refuses a sealed upload (`400`); `` client.apps.blobUpload(app = app, body = data) `` for a plain one |
| `post` | the facade method for the route (raw plumbing) |
| `get` | the facade method for the route (raw plumbing) |
| `put` | the facade method for the route (raw plumbing) |
| `delete` | the facade method for the route (raw plumbing) |
| `signUpPolicy` | `` client.deployments.signUpPublic() `` |
| `me` | `` client.auth.me() `` |
| `removePasskey` | `` client.auth.passkeyRemove(cred = credentialId, passkey = passkey) `` |
| `labelPasskey` | `` client.auth.passkeyLabel(cred = credentialId, label = label) `` |
| `usage` | `` client.auth.usage() `` |
| `listApiKeys` | `` client.auth.apikeyList().propertyKeys `` |
| `revokeApiKey` | `` client.auth.apikeyRevoke(key = keyId, passkey = passkey) `` |
| `createApiKey` | `` client.auth.apikeyCreate(label = options.label, apps = options.apps, methods = options.methods, readOnly = options.readOnly, passkey = passkey) `` |
| `uploadBundle` | `` client.packages.bundlePut(body = wasm).hash `` |
| `downloadBlob` | `` client.apps.blobDownload(app = app, id = id) `` |
| `downloadBlobGranted` | `` client.apps.blobDownload(app = app, id = id, grant = grant) `` |
| `listBlobs` | `` client.apps.blobList(app = app).blobs `` |
| `blobInfo` | `` client.apps.blobInfo(app = app, id = id) `` |
| `deleteBlob` | `` client.apps.blobDelete(app = app, id = id) `` |
| `uploadPackage` | `` client.packages.upload(body = mpk) `` |
| `installPackage` | `` client.packages.install(`package` = pkg, version = version) `` |
| `packages` | `` client.packages.list().packages `` |
| `package` | `` client.packages.get(`package` = pkg) `` |
| `uninstallPackage` | `` client.packages.uninstall(`package` = pkg) `` (With a version: packages.versionUninstall) |
| `listApps` | `` client.apps.list(`package` = pkg, bundle = bundle).apps `` |
| `getApp` | `` client.apps.get(app = app) `` |
| `deleteApp` | `` client.apps.delete(app = app) `` |
| `setAccess` | `` client.apps.access(app = app, access = access) `` |
| `setAppMetadata` | `` client.apps.metadata(app = app, metadata = metadata) `` |
| `appStorage` | `` client.apps.storage(app = app) `` |
| `appAbi` | `` client.apps.abi(app = app) `` |
| `transferApp` | `` client.apps.transfer(app = app, to = to, passkey = passkey) `` |
| `upgradeApp` | `` client.apps.upgrade(app = app, bundle = target.bundle, `package` = target.`package`, version = target.version, service = target.service, migrate = target.migrate, passkey = passkey) `` |
| `postConfirmed` | the facade method for the route (raw plumbing) |
| `deleteConfirmed` | the facade method for the route (raw plumbing) |
| `createApp` | `` client.apps.create(name = name, org = org, access = access, initArgs = initArgs, metadata = metadata).appId `` (The spec has no confirmMethods) |
| `call` | `` client.apps.intents(app = app, method = method, args = args, idempotencyKey = idempotencyKey) `` |
| `callConfirmed` | `` client.apps.intents(app = app, method = method, args = args, idempotencyKey = idempotencyKey, passkey = passkey) `` |
| `view` | `` client.apps.views(app = app, method = method, args = args, consistency = if (stale) ViewsRequest.Consistency.STALE else ViewsRequest.Consistency.STRONG, minIndex = minIndex) `` |
| `publishPresence` | `` client.apps.presencePublish(app = app, seq = seq, state = Explicit(state?.let(Hex::encode))) `` |
| `presence` | `` client.apps.presenceGet(app = app).present `` |
| `setAlias` | `` client.auth.aliasSet(alias = alias, app = app) `` |
| `aliases` | `` client.auth.aliasList().aliases `` |
| `alias` | `` client.auth.aliasGet(alias = alias).appId `` |
| `deleteAlias` | `` client.auth.aliasDelete(alias = alias) `` |
| `createOrg` | `` client.orgs.create(name = name, recoveryKey = recoveryKey).orgId `` |
| `setRecoveryKey` | `` client.orgs.recoverySet(org = org, publicKey = Explicit(publicKey), passkey = passkey) `` (`Explicit(null)` removes the key) |
| `cancelRecovery` | `` client.orgs.recoveryCancel(org = org, passkey = passkey) `` |
| `finishRecovery` | `` client.orgs.recoveryFinish(org = org, passkey = passkey) `` |
| `getOrg` | `` client.orgs.get(org = org) `` |
| `deleteOrg` | `` client.orgs.delete(org = org, passkey = passkey) `` |
| `setOrgMetadata` | `` client.orgs.metadata(org = org, metadata = metadata) `` |
| `setMemberMetadata` | `` client.orgs.memberMetadata(org = org, account = account, metadata = metadata) `` |
| `upgradeOrg` | `` client.orgs.upgrade(org = org, `package` = target.`package`, version = target.version, team = target.team, cascade = target.cascade, migrate = target.migrate, passkey = passkey) `` |
| `upgradeRun` | `` client.orgs.upgradeStatus(org = org, run = run) `` |
| `retryUpgradeRun` | `` client.orgs.upgradeRetry(org = org, run = run, passkey = passkey) `` |
| `approveRelease` | `` client.orgs.releaseApproval(org = org, version = version, approved = approved, passkey = passkey) `` (With no org: auth.accountReleaseApproval) |
| `approveOutbound` | `` client.orgs.outboundApproval(org = org, `package` = pkg, prefixes = prefixes?.let { OutboundSelection.Prefixes(it) } ?: OutboundSelection.Manifest, replace = replace, passkey = passkey) `` (With no org: auth.accountOutboundApproval) |
| `appsReport` | `` client.orgs.apps(org = org, after = page.after, limit = page.limit?.toLong()) `` (With no org: auth.accountApps) |
| `releaseRules` | `` client.public.releasesPublic() `` |
| `consentResult` | `` client.consent.pageResult(request = requestId) `` |
| `publisherBinding` | `` client.public.publisherBinding(signer = signerId) `` |
| `bindPublisher` | `` client.orgs.publisher(org = org, signerId = proof.signerId, publicKey = proof.publicKey, signature = proof.signature, passkey = passkey) `` |
| `upgradePublished` | `` client.orgs.publisherUpgrade(org = org, `package` = pkg, version = version, migrate = migrate, passkey = passkey) `` |
| `opsApps` | `` client.ops.apps(after = page.after, limit = page.limit?.toLong()) `` |
| `opsOrgs` | `` client.ops.orgs(after = page.after, limit = page.limit?.toLong()) `` |
| `setReleaseDeadline` | `` client.ops.releaseDeadline(version = version, after = after.toString()) `` |
| `liftReleaseDeadline` | `` client.ops.releaseDeadlineLift(version = version) `` |
| `setMember` | `` client.orgs.setMember(org = org, account = account, role = OrgRole.decode(role.wire)!!, passkey = passkey) `` |
| `removeMember` | `` client.orgs.removeMember(org = org, account = account, passkey = passkey) `` |
| `setCapabilities` | `` client.orgs.capabilities(org = org, capabilities = capabilities.toLong(), account = account, team = team, passkey = passkey) `` |
| `createTeam` | `` client.orgs.teamCreate(org = org, name = name, join = TeamJoin.decode(join.wire)!!, parent = parent).teamId `` |
| `getTeam` | `` client.orgs.teamGet(org = org, team = team) `` |
| `updateTeam` | `` client.orgs.teamUpdate(org = org, team = team, name = name, join = join?.let { TeamJoin.decode(it.wire) }, parent = if (toTop) Explicit(null) else parent?.let(::Explicit), metadata = metadata) `` |
| `deleteTeam` | `` client.orgs.teamDelete(org = org, team = team) `` |
| `setTeamMember` | `` client.orgs.teamSetMember(org = org, team = team, account = account, lead = lead, role = role?.let { TeamRole.decode(it.wire) }, capabilities = capabilities?.toLong(), passkey = passkey) `` |
| `removeTeamMember` | `` client.orgs.teamRemoveMember(org = org, team = team, account = account, passkey = passkey) `` |
| `invite` | `` client.orgs.inviteCreate(org = org, role = OrgRole.decode(role.wire)!!, teams = teams, ttlSecs = ttlSecs, uses = uses?.toLong(), passkey = passkey).token `` |
| `redeemInvite` | `` client.orgs.inviteRedeem(token = token) `` |
| `packageOrigins` | `` client.packages.originsList(`package` = pkg).origins `` |
| `addPackageOrigin` | `` client.packages.originsAdd(`package` = pkg, origin = origin, rpId = rpId, passkey = passkey) `` |
| `removePackageOrigin` | `` client.packages.originsRemove(`package` = pkg, origin = origin, passkey = passkey) `` |
| `grantDeployer` | `` client.deployments.deployerGrant(deployer = deployer, until = until?.toString(), passkey = passkey) `` |
| `deployers` | `` client.deployments.deployerList() `` |
| `revokeDeployer` | `` client.deployments.deployerRevoke(key = deployer.key, passkey = passkey) `` |
| `createDeployment` | `` client.deployments.create(name = name, owner = owner, passkey = passkey).id `` |
| `deployments` | `` client.deployments.list().deployments `` |
| `deployment` | `` client.deployments.get(id = id) `` |
| `deleteDeployment` | `` client.deployments.delete(id = id, passkey = passkey) `` |
| `offerApp` | `` client.deployments.offer(id = id, app = app, `package` = pkg, version = version, passkey = passkey) `` |
| `withdrawApp` | `` client.deployments.withdraw(id = id, app = app, passkey = passkey) `` |
| `deploymentOrgs` | `` client.deployments.orgs(id = id) `` |


A removed method that sent an explicit `null` (`setRecoveryKey(org, null, …)`, `updateTeam(toTop = true)`,
`publishPresence(state = null)`) maps to the facade's `Explicit(null)` (`publicKey`, `parent`, `state`).
The sample app's SDK tab runs the facade methods, named `group.method`.

## 0.4.0

### Releases are cut on merge (CI)

- `.github/workflows/release-on-merge.yml` runs semantic-release (`.releaserc.json`, as in mero-js)
  on every push to `main`: the conventional commits since the last `v*` tag decide the version, it
  pushes the tag `vX.Y.Z` with a GitHub App token and creates the GitHub Release, and the tag runs
  `release.yml`. No manual tag.
- Maven Central publishes automatically (`publishingType = "AUTOMATIC"`), with no click in the Portal.
  A Central version can never be replaced. See PUBLISHING.md.

### The API client is generated from core's OpenAPI document (mero-core)

- `openapi/openapi.json` vendors core's node API (`docs/public/openapi.json` on core's master,
  commit in `openapi/SOURCE`); `scripts/refresh-openapi.sh [ref]` refreshes it and regenerates,
  `--check` (CI) fails on a stale or hand-edited client. See `openapi/README.md`.
- **Facade**: `client.orgs`, `client.apps`, `client.auth`, `client.packages`, `client.deployments`,
  `client.consent`, `client.public`, `client.ops`: one suspend method per operation, the body's fields
  as named parameters, the typed answer (`client.orgs.inviteCreate(org = org, role = OrgRole.MEMBER)`,
  `client.orgs.get(org = org)`). Generated by `codegen/facade.py`.
- Under it, OpenAPI Generator's Kotlin client (`com.calimero.mero.api`, `client.api()`), whose
  transport is the client's own: sealed to the attested node, `401` refresh, `421` failover, `503`
  retries of calls and views. Refusals throw the typed `CalimeroError`s.
- `412 CONFIRM_REQUIRED` and the new `428 OPERATOR_SIGNATURE_REQUIRED` (`OperatorSignatureRequired`)
  are answered by the transport: a call's `passkey` / `operator`, else `CalimeroClientOptions.passkey` /
  `operator` (`CalimeroClient.passkey` / `operator`).
- The `CalimeroClient` methods that one operation replaces keep their names and signatures, now thin
  calls onto the generated client, and are `@Deprecated` with a `ReplaceWith` to the facade method.
  Ceremonies, streams, binary uploads/downloads and composites (`signIn`, `events`, `uploadBlob`,
  `pendingApprovals`, …) are not deprecated.
- `AppAccess` is `@Serializable`; new `OutboundSelection` (`Manifest` / `Prefixes`).
- Fields whose `null` means something (a team's `parent`, a recovery `publicKey`, a presence
  `state`) are `Explicit<T>?` on the facade: `Explicit(null)` sends `null`; leaving it out sends nothing.
- `ConsentRequest` is `@Serializable` (the consent body), and `requestConsent` and `appXcalls` are
  deprecated in favour of `consent.pageCreate` and `apps.xcalls`. `uploadFile` is deprecated: core
  refuses sealed uploads.

### Outbound approval (mero-core, mero-compose)

Mirrors calimero-network/core#4690: an app's outbound HTTPS request needs the namespace's
approval for the app's publisher (bundle signer).

- `OutboundSelection`: `Manifest` (the string `"manifest"`) or `Prefixes(list)` (a bare JSON array),
  hand-serialized to exactly those two forms.
- `CreateNamespaceRequest.approveOutbound` and `UpgradeGroupRequest.approveOutbound`, omitted when
  `null`; `CreateNamespaceResponseData.outboundApproval` and `UpgradeGroupResponseData.outboundApproval`
  (`OutboundApproval(signer, prefixes, reviewed)`), present only when an approval was asked for.
- `AdminSurface.listOutboundApprovals(namespaceId)` (`GET /admin-api/namespaces/{id}/outbound-approvals`)
  and `setOutboundApproval(namespaceId, SetOutboundApprovalRequest(applicationId, approve))` (`PUT`, sets
  the list exactly; an empty list revokes), returning `SetOutboundApprovalResponseData` with `published`.
- An account admin refuses `setOutboundApproval` and a `createNamespace` with `approveOutbound`
  (`NotForAccountException`) rather than founding without the approval.
- mero-compose: `MeroReads.outboundApprovals` / `rememberOutboundApprovals`, and the
  `SetOutboundApproval` mutation / `rememberSetOutboundApproval`. The sample app's SDK Explorer lists both calls.

### Request proofs refuse a window a node would refuse (mero-core)

- A request-proof window is capped at `MAX_REQUEST_PROOF_TTL_SECONDS` (300), core's
  `MAX_REQUEST_LIFETIME_SECS`. `createProofSigner` (`ttlSeconds`), `signRequestProof` and
  `createProofAuthorizer` (`validForSeconds`) throw `IllegalArgumentException` for anything
  outside 1..300, instead of minting a proof every node refuses. The window is what bounds replay:
  a two-link proof names no node (SEC-14).

### CI supply chain

- Every workflow action is pinned by commit SHA (release in a trailing comment), and Dependabot
  keeps the pins current (SEC-13).
- The Gradle wrapper checks its distribution against `distributionSha256Sum`, and every
  dependency and plugin is checked against `gradle/verification-metadata.xml`. TESTING.md says
  how to regenerate it after a version bump.
- `ci.yml` runs with a read-only token.
- The release is three jobs: GitHub Packages (`packages: write`), Maven Central (the signing key
  and Central token only, no third-party action, `maven-central` environment) and the GitHub
  Release (`contents: write`). Behaviour is unchanged, including skipping Central without its
  secrets.

### Fixed — ids are one path segment, SEC-02 (mero-core)

- **Every id `AdminApi` puts in a path is percent-encoded** (`contextId`, `namespaceId`, `groupId`,
  `applicationId`, `blobId`, identities, device ids, alias and package names). A `/`, `?` or `#` in
  one can no longer reach another route: `joinNamespace("abc/leave?x=")` posts to
  `/admin-api/namespaces/abc%2Fleave%3Fx%3D/join`, which the node refuses. An empty, `.` or `..` id
  throws `IllegalArgumentException` before anything is sent (OkHttp would resolve a dot segment
  away). Real ids (hex or base58) are sent unchanged.
- **`redeemInvitation` refuses an invite whose `namespaceId` is not 64 lowercase hex** with
  `Failed(namespaceId = null, reason = INVALID, retryable = false)`, and sends nothing. The id comes
  from the invite, so a crafted one could otherwise retarget the user's authenticated join.

### Enrolment callbacks need a pending state (SEC-01, mero-core)

- **An enrolment callback is refused unless it answers the enrolment this app began.**
  `Enrolment.completeDeviceEnrolment` throws `EnrolmentException` when `expectState` is null (the
  parameter no longer defaults), so a credential someone else minted for this device's (public)
  keys and sent as a link can no longer sign the app in to their account (login CSRF).
- `CloudAccount`'s pending state is single-use and expires after 10 minutes
  (`CloudAccount.ENROLMENT_STATE_LIFETIME_MS`).
- `CloudAccount.completeEnrolment` throws the new `AccountSwitchException` (`currentAccount`,
  `incomingAccount`) rather than replacing a session for another account; pass
  `allowAccountSwitch = true` once the person confirms. `EnrolmentException` is now `open`.

### Sign-out forgets the account; encrypted stores survive a restore (mero-core, mero-compose)

- **`CloudAccount.signOut` forgets the account's relay map, subgroup namespaces and its relays'
  pinned keys** (`forgetAccount`); `signOut(forgetDevice = true)` also drops the device keys and
  every warrant, join and governance nonce spent under them (new `SecureStore.removeAll(prefix)`,
  a no-op by default for stores that cannot list their keys). `MeroClient.logout(forgetDevice)`
  passes it through, and a sign-out clears the invitation attempt ledger
  (`InvitationCapture.clearAttempts()`).
- **An `EncryptedSharedPreferences` file whose keyset no longer decrypts is reset instead of
  crashing the app** (`EncryptedPrefsTokenStore`, `EncryptedPrefsSecureStore`). This happens after
  a backup restore or device transfer, because the Keystore master key never travels; the person
  signs in again. The README shows the `dataExtractionRules` that keep the `mero_*` files out of
  backups.
- Bouncy Castle (`bcprov-jdk18on`, used for the on-device DCAP X.509/CRL checks) is 1.86, up from
  1.78.1.

### Transport and trust hardening: SEC-03, SEC-06, SEC-15 (mero-core, mero-compose)

- **Plain http only to local hosts (SEC-06).** A node is reached over `http://` / `ws://` only when
  its host is loopback, link-local, a private LAN address (RFC 1918, `fc00::/7`) or a `.local`
  name; anything else throws `InsecureTransportException` before a password, token or proof is
  attached. The Cloud manager, relays, the registry, the wallet and the release mirror must be
  `https` (loopback and the emulator's `10.0.2.2` excepted). A client sealed with
  `SealedTransport` is end-to-end encrypted and not checked. `TransportSecurity.allowInsecureHttp
  = true` lifts the rules for a development rig. `MeroClient.login` and `connectToNode` refuse such
  a node with `MeroClient.INSECURE_NODE_URL`, and `NodeLoginState.isInsecure` lets the login sheet
  warn while one is typed.
- **SSO callbacks must be https unless you opt in (SEC-03).** `parseAuthCallback` refuses a
  callback on a custom scheme or plain http unless `allowCustomScheme = true`: any app can declare
  the scheme and receive the tokens. `MeroClient` follows `MeroAppConfig.allowCustomSchemeCallbacks`
  (false by default; `connectToNode` says `CUSTOM_SCHEME_CALLBACK` up front). `SsoLauncher` and
  `WalletLauncher` pin the Custom Tab to the browser (`Intent.setPackage`).
- **Mock quotes are refused from loopback relays too (SEC-15).** `DcapRelayKeyVerifier` and
  `TlsRelayKeyVerifier` accept a mock quote only with `allowMock = true`: another app on the device
  can listen on `127.0.0.1`. `TlsRelayKeyVerifier` and `PinnedRelayKeyVerifier` need
  `@OptIn(DevOnlyMeroApi::class)`.

### Core 0.11.0: namespace ops signed at schema 27 (mero-core)

- `NamespaceOp.SCHEMA_VERSION` is 27, core 0.11.0's `SIGNED_NAMESPACE_OP_SCHEMA_VERSION`
  (calimero-network/core#4625; no layout change from 26). A 0.11.0 node refuses a join signed at
  26, so an account join (`joinAsAccount`) failed against it; Mero Chat's Android live tests on
  calimero-network/apps#622 hit this. mero-js made the same change in its #292.
- CI and the merobox scenarios are pinned to core `0.11.0` (`ci/core-version`).

### release workflow

- A tag without the Maven Central secrets (`MAVEN_CENTRAL_USERNAME`, `SIGNING_KEY`) now skips the
  Central step with a warning instead of failing, so the GitHub Release is still cut for the
  GitHub Packages build. `v0.1.0` hit this: it reached GitHub Packages, and its Release was cut by
  hand.

### sample-app

- The node sign-in screen words its errors from `MeroAuthState.errorReason` instead of matching
  `error`'s text, and now tells a lockout (`RATE_LIMITED`, "Too many failed sign-ins") and a node
  error (`NODE_ERROR`, with its HTTP status) apart from a wrong password or an unreachable node.
- A username/password session comes back on relaunch through `MeroClient.restore()`, which the
  app already calls at startup; nothing app-side persists it.

### Fixed — `MeroClient.login` sessions, errors and grants (mero-compose)

- **A password session survives a relaunch.** `login(nodeUrl, username, password)` now persists
  the node URL (and the node the tokens came from) and the username in `MeroAppStorage`, so
  `restore()` brings the session back while its tokens validate. It was not persisted, so a
  relaunch always signed out. `MeroAppStorage.username` is new; `signOut` clears it.
- **Typed login errors.** `MeroAuthState.errorReason` (`MeroErrorReason`: `INVALID_INPUT`,
  `INVALID_CREDENTIALS`, `RATE_LIMITED`, `UNREACHABLE`, `NODE_ERROR`, `PERMISSIONS_REFUSED`,
  `UNKNOWN`) and `errorStatus` (the HTTP status) sit beside `error`, so an app can tell a bad
  password (401) from a lockout (429, "Too many failed login attempts") from a node that is
  not there without matching the message. The messages are unchanged.
- **Scoped grants.** `login(…, permissions = …)` signs in with a client key holding just those
  grants, minted with the admin token through `POST /admin/client-key`; only the client key's
  tokens are kept. Without `permissions` it is the admin token, as before.
- **`MeroAppConfig.httpClient`.** Node sessions are built on this OkHttp client
  (`MeroConfig.httpClient`), for interceptors and the like. `MeroClient.create` also hands it to
  its `CloudAccount`.
- `FakeNode` (mero-testkit) mints client keys on `POST /admin/client-key` (refusing `keys:*`, as
  core does).

### Changed — Compose BOM 2024.12.01

The Compose BOM moves from 2024.09.03 to 2024.12.01 (Compose 1.7.6). On an emulator, 2024.09.03
crashed in a bottom sheet with "performMeasureAndLayout called during measure layout"; 2024.12.01
does not. An app on an older BOM resolves the newer Compose through mero-compose.

### Added — every sign-in string is overridable (mero-compose)

`LoginBranding` now carries every user-visible string on `LoginView`, not only the titles and the
Cloud button, so an app can rebrand the sign-in screen completely: `cloudHint` ("Opens the
Calimero wallet…"), `cloudCancelled`, `customWalletNote`, `manualNodeLink` ("Manually enter node
URL"), `useCloudLink` ("Use Calimero Cloud instead"), `searchingNodes`, `localNodeTag`,
`manualNodeOption`, the node URL / username / password labels and placeholders, `nodeButton` and
`rescanNodes`. The defaults are the old strings.

- `AccountSignInPanel` takes `branding` (its `buttonTitle` still overrides the button).
- `LoginSheet` takes `branding`; `title` is now nullable and overrides `branding.cloudTitle` only
  when given.
- `ConnectButton` takes `branding` (passed to its sheet; its `logo` also replaces the Calimero
  logo on the button), and `ConnectButtonLabels` gains `account`, `noRelay` and `logOut`.

### Changed — neutral error wording

`MeroAuthState.error` no longer names Calimero: "Could not open the wallet" (was "…the Calimero
wallet"), "Signed in with a cloud account: sign out before connecting to a node." `CloudAccount`'s
"sign in with Calimero Cloud first" and "not signed in with Calimero Cloud" now say "a cloud
account".

### Fixed — binary blobs were corrupted on the wire

`AdminApi.uploadBlob` sent `String(bytes, ISO_8859_1)` as the body, which OkHttp then encoded
as UTF-8, and `getBlob` read the body as UTF-8 text. Every byte `>= 0x80` was rewritten in both
directions. A round trip through the SDK hid it, because the two errors cancel, but the node
stored the wrong bytes: a 21,327-byte JPEG was stored as 31,843 bytes. Both now carry raw bytes
end to end.

- **`HttpClient.executeBytes`** (new) sends a `ByteArray` body verbatim and returns an
  **`HttpBytesResponse`** with the body as raw bytes. `OkHttpTransport` and the relay's
  session-carrying admin client implement it. The interface's default implementation throws, so
  a custom `HttpClient` keeps compiling and has to opt in to carrying bytes.
- **`FakeNode`** (mero-testkit) serves the blob routes (`PUT`/`GET /admin-api/blobs`, and
  `GET`/`HEAD`/`DELETE /admin-api/blobs/{id}`), keeps the bytes it received in `blobs`, and
  serves them back raw.

### Changed — core 0.11.0-rc.90; namespace ops signed at schema version 26

The SDK now targets core `0.11.0-rc.90` (`ci/core-version` and the merod image in every merobox
scenario). Core rc.90 raised `SIGNED_NAMESPACE_OP_SCHEMA_VERSION` to 26
(calimero-network/core#4605): a device link's scope is now judged at the link's cut. The op layout
is unchanged, but a node refuses any other version, so a join signed at 25 failed against an rc.90
relay with `schema version mismatch: expected 26, got 25`. `NamespaceOp.SCHEMA_VERSION` is 26
now, matching mero-js (its #290). Found by Mero Chat's live tests against an rc.90 relay
(calimero-network/apps#597).

No route this SDK calls changed between rc.89 and rc.90. A join refusal now carries a code
(core#4610), and `/admit` is served without a credential (core#4611), which the SDK already sends
none of.

### Added — `LoginView`, login modes and the extended theme (mero-compose)

- **`LoginView`**, redesigned like the Swift SDK's: a soft glow, the emblem on a 76dp tile, a big
  title, "Continue with Calimero Cloud", and a node card (URL, optional username and password,
  "Connect to node", rescan).
- **`LoginMode`** (`CLOUD_AND_NODE`, the default, with a "Manually enter node URL" link;
  `CLOUD_ONLY`; `NODE_ONLY`), **`NodeSignInMethod`** (`LOGIN_PAGE`, the default; `PASSWORD`,
  through `MeroClient.login`) and **`LoginBranding`** (titles, subtitles, button, logo).
- **`LoginSheet`** keeps its parameters and draws `LoginView`; `tabs` become a mode
  (`LoginMode.from`). Its test tags are now `LoginView`'s (`loginTitle`, `cloudSignInButton`, …).
- **`MeroTheme`** gains `surface`, `textTertiary`, `success`, `warning`, `cardRadius`,
  `controlHeight`, `titleFontFamily` and `showsGlow`, and presets `MeroTheme.calimeroLight` and
  `MeroTheme.calimero`. **`LocalMeroTheme` now defaults to `calimeroLight`** (it was the dark
  `defaultMeroTheme`), matching the Swift SDK; provide `MeroTheme.calimero` to keep the dark look.
- `MeroClient.clearError()`.

### Fixed — alias create and delete

`createContextAlias`, `createApplicationAlias`, `deleteContextAlias` and
`deleteApplicationAlias` threw on success: core answers its `Empty` as
`{"data": null}`, and these four treated a null `data` as an error. They return
`Empty()` now, like the other `Empty` routes and Swift's `emptyOK`.

### Fixed — a method that returns nothing

`RpcClient.executeRaw` answered `{}` when `result.output` was missing (a method
returning `()`, such as kv-store's `set`), so `execute<String?>` failed to
decode and a `JsonElement` caller saw an empty object. It answers `JsonNull`
now: optional and `JsonElement` results decode to null, and a non-nullable
type still fails to decode. Same fix as the Swift SDK.

### Changed — core 0.11.0-rc.89

The SDK now targets core `0.11.0-rc.89` (`ci/core-version`, and the merod image
in every `ci/merobox/*.yml`). Nothing on the wire was taken away between rc.83
and rc.89; what moved is already in this SDK or is additive:

- Request proofs sign the path *and query* under `calimero.auth.request.v2`
  (core#4549). `RequestSig` already signs that preimage.
- Signed namespace ops are at schema version 25 (core#4552, rc.85): the fix
  below.
- `TeeAttestRequest.eventLog` / `TeeAttestResponseData.eventLogB64` (core#4591):
  ask `POST /admin-api/tee/attest` for the TD's CCEL event log, base64. A mock
  TEE answers `400`.
- `listNamespaceRevokedDevices` (core#4589) was already added.
- A nested subgroup is created `Open` unless the caller says otherwise
  (core#4543); `createGroupInNamespace` always sends a visibility, so nothing
  changes for callers.
- A client key bound to an application acts only for that application
  (core#4564).

### Fixed — sign namespace ops at schema version 25

`NamespaceOp.signMemberJoinOp` signed at schema version 24, so every node on
core 0.11.0-rc.85 or later (calimero-network/core#4552) refused the join:
`signed_op signature does not verify: schema version mismatch: expected 25,
got 24`. A keyholder could not join a namespace from an invitation through a
current relay. `NamespaceOp.SCHEMA_VERSION` is 25 now, matching mero-js
(its #261). Found by Mero Chat's Android live tests against a real relay.

### Breaking — drop the warrant-nonce routes core does not serve

Core (`crates/server/endpoints.json`, `admin/service.rs`) mounts neither
`GET /admin-api/contexts/{ctx}/warrant-nonce/{deviceKey}` nor
`POST /admin-api/contexts/{ctx}/warrant-nonce`, so every call could only fail.
mero-js removed the same API. Removed:

- `AdminApi.getWarrantNonce` / `getWarrantNonceAsAuthor` (and on `AdminSurface`
  and the no-relay admin), `WarrantNonceState`, `WarrantNonceRouteUnavailableException`
  and the internal `parseWarrantNonceBody`.
- `RelayClient.getWarrantNonce` / `getWarrantNonceAsAuthor` / `recoverNonce`,
  `RelayWarrantNonceState` and `WarrantNonceExhaustedException` (and its
  `CONFLICT` mapping in `classifyError`).
- The SDK Explorer entries `ctx.warrantNonce`, `ctx.warrantNonceAuthor` and
  `rel.nonce`, and FakeCloud's warrant-nonce route and `nextNonce`.

Nonces come only from the device's own counter (`NonceSource`: `MemoryNonceSource`,
`PersistedNonceSource`, `advanceTo`), which was already the only path that worked
on a relay. `RelayClient.execute` is a single attempt (only a `429` is retried):
a warrant refused for its nonce surfaces as `IntentRefusedException` with
`retryable = true` instead of triggering a lookup and re-signing, and the next
call signs a fresh nonce — as in mero-js and swift-sdk.

## Unreleased — core 0.11.0-rc.83

Pinned to `0.11.0-rc.83` in `ci/core-version` (and the matching
`ghcr.io/calimero-network/merod:0.11.0-rc.83` image in every merobox scenario).

### Breaking — mero-js parity (auth hardening)

- **`parseAuthCallback` adopts tokens only for a login this app began.**
  `buildAuthLoginUrl` (and `SsoLauncher.launch`) now records a random,
  single-use, 10-minute `auth_state` in a `PendingAuthStates` store and adds it
  to the callback URL; `parseAuthCallback` returns `null` for a callback without
  a pending state, and spends it on success. A crafted deep link carrying an
  attacker's tokens is refused. The default store is in memory — set
  `PendingAuthStates.shared = PendingAuthStates(EncryptedPrefsSecureStore(context, "mero_auth_state"))`
  so a login survives the process dying while the Custom Tab is open, or pass
  `pending =` explicitly. `parseAuthCallback(url, allowUnsolicited = true)` keeps
  the old behaviour for a host that injects tokens itself. Port of mero-js
  `auth/index.ts` (same storage key and shape). Existing calls compile
  unchanged; a callback for a login not started with `buildAuthLoginUrl` in this
  process (or a persistent store) is now `null`.
- **Persisted tokens are bound to the node that issued them.** `TokenData`
  gains an optional `nodeOrigin` (`node_origin`; bundles without it still
  decode). `Mero` tags what it persists with `nodeOriginOf(baseUrl)` — origin
  plus path prefix, trailing slashes dropped — and reads a stored bundle for any
  other node, **or with no tag**, as empty. So a bundle persisted by an earlier
  SDK version is ignored once: those users sign in again. `getTokenData()`
  returns bundles without the tag; `clearToken()` leaves another node's bundle in
  a shared store alone. Port of mero-js `TokenData.node_origin`.

### Added — mero-js parity

- `Capabilities.Presets` (`NONE`, `JOIN_ONLY`, `CONTRIBUTOR` = 7,
  `COLLABORATOR` = 231), `Capabilities.PRESETS` / `BY_NAME`, `capMask(names)` /
  `capMask(base, names)` (refuses an unknown name) and `capNames(mask)` — mero-js
  `CAPABILITY_PRESETS`, `capMask`, `capNames`.
  `MeroClient.DEFAULT_SPACE_CAPABILITIES` is now `Presets.COLLABORATOR`.
- `classifyError(Throwable?)` → `ClassifiedError(kind, status, retryable,
  message)` with `ErrorKind` (`INVALID`, `UNAUTHORIZED`, `FORBIDDEN`,
  `NOT_FOUND`, `CONFLICT`, `GONE`, `TOO_LARGE`, `RATE_LIMITED`, `UNAVAILABLE`,
  `SERVER`, `UNREACHABLE`, `UNKNOWN`), plus `statusOf` / `messageOf` — mero-js
  `errors/classify.ts`, mapped over the SDK's exceptions.
- Local node discovery (`com.calimero.mero.discovery`):
  `DEFAULT_LOCAL_NODE_PORTS`, `localNodeUrl(port, host)`, `nodeEndpoint`,
  `probeNodeHealth`, `discoverLocalNodes` (parallel, per-probe timeout) and
  `ANDROID_EMULATOR_HOST` (`10.0.2.2`) — mero-js `nodeDiscovery.ts`.
- `AdminApi.listNamespaceRevokedDevices(namespaceId)` →
  `NamespaceRevokedDevices` (`GET /admin-api/namespaces/{id}/revoked-devices`)
  and `AuthApi.getLoginPage()` (`GET /auth/login`).
- `PinnedRelayKeyVerifier(nodeKey)`, `RelayLogin.logout(relayUrl,
  refreshToken)` and `RelayConnection.logout()`. `MeroClient.signOut` now
  retires the relay session on the relay (`POST /auth/logout`) instead of only
  dropping it.
- `MeroClient.login(nodeUrl, username, password)` — **development only** —
  signs the Compose client in to a node you run yourself (Swift MeroKitUI
  parity); `MeroAuthState` gains `nodeUrl`, `username` and `isDevNodeSession`.
- `FakeNode` answers `GET /admin-api/health`.

### Added — events and presence (mero-js parity)

- **`SseConnection`** (`Mero.sse`, `SseClient.connection()`): one long-lived SSE
  connection — mero-js `SseClient`. `subscribe(contextIds, groupIds)` /
  `unsubscribe(...)` change a **live** session (`POST /sse/subscription`, only the ids
  that changed, 10 s timeout); `isSubscribed` / `isSubscribedToGroup`; every
  remembered id is re-subscribed on each `connect` frame, *then* `connects` emits the
  session id. Hot `events`, `errors` and `failure` (the terminal refusal). Reconnects
  on mero-js's fixed 3 s delay — SSE has no backoff in mero-js, so none here. A failed
  subscription POST is reported on `errors` rather than thrown, as in mero-js. The
  existing `events` / `groupEvents` / `nodeEvents` flows now run on it unchanged.
- **Typed events**: `AppVersionChangedEvent`, `GroupMigrationEvent`
  (`MigrationStarted`, `MigrationProgress`, `CascadeProgress`, `MigrationCompleted`)
  and `GroupMembershipEvent` (`MemberJoined`, `MemberAdded`, `MemberRemoved`), with
  core's field names; `appVersionChanges` / `migrationEvents` / `membershipEvents`
  flows on `Mero`, `SseClient` and `SseConnection`, callback forms
  `onAppVersionChanged` / `onMigrationEvent`, and `asAppVersionChanged()` /
  `asMigrationEvent()` / `asMembershipEvent()` on a raw event.
- **`presenceSeed(contextId)`**: the latest `Ephemeral` entry per author of each
  subscribed context, `ageMs` advanced by the time since arrival; replayed to a late
  presence subscriber, cleared on reconnect and unsubscribe.
- **`WsClient`** (`Mero.ws`, `@ExperimentalMeroApi` as in mero-js): OkHttp WebSocket on
  `GET /ws` (Bearer header; core also takes `?token=`), `subscribe` / `unsubscribe`
  messages `{"id":null,"method":…,"params":{contextIds?,groupIds?}}`, re-subscribe on
  every open, reconnect after `min(1 s·2ⁿ, 30 s)`; a `403` upgrade stops it.
- **`EphemeralClient`** (`Mero.ephemeral`): `set(contextId, state, codec)` via
  JSON-RPC `set_ephemeral {contextId, state: [bytes]}`, `subscribe(contextId, codec)`
  → `Flow<EphemeralEntry>` (`author`, `account`, `state`, `removed`, `ageMs`), and
  `subscribePresence` (filter `Ephemeral`, replay the seed, connect and subscribe).
  `Codec`, `JsonCodec` / `jsonCodec<T>()` (bytes identical to mero-js) and `RawCodec`.
- **`RelayPresenceClient`** (`RelayConnection.presence`, `MeroClient.presence`): an
  account's presence signed with the device key and posted to `presence-intents`,
  resent every 2.5 s while set; `set(ctx, null)` / `clear(ctx)` retracts, `close()`
  stops; 429/5xx/network failures wait for the next resend, other refusals throw and
  stop. `RelayConnection.logout()` closes it.
- **`PresenceStatement`**: `PRESENCE_DOMAIN` (`calimero/presence/1`), `stateHash`,
  `presenceStatementBytes` (pinned to core's `statement_vector_is_stable`), `sign` and
  `signedRequest` — so `AdminApi.presenceIntent` callers no longer hand-supply a
  signature. `RelayClient.publishPresence(contextId, state, seq)` signs for you;
  `RelayClient` implements `PresenceRelay` and exposes `authorSigner`.
- SDK Explorer: a **Presence** category (`presence.set`, `presence.clear`,
  `publishPresence`, `ephemeral.set`).

### Added — signing primitives (mero-js parity)

- **Owner-op root proofs** (`com.calimero.mero.ownerop`, mero-js `owner-op`): `OwnerOpKind` /
  `OWNER_OP_KIND`, `OwnerOpPlane`, `OwnerOp`, the encoders `transferOwnershipOp`,
  `groupDeleteOp`, `adminChangedOp`, `teeAuthoringPolicyOp`, `teeAdmissionPolicyOp`,
  `teeReleaseAdmissionPolicyOp` (+ `TeeAdmissionPolicyOpInput`,
  `TeeReleaseAdmissionPolicyOpInput`, `teeAdmissionPolicyOpOf(request)`), `ownerOpDigest`,
  `ownerOpSigningPayload`, `signOwnerOpProof(OwnerOpProofInput)` — the hex borsh of core's
  `SignedOwnerOp` (`AccountProof<OwnerOpAuthorization>`) — and `rootGuardedOpBytes`, pinned to
  core's `owner_op.rs` and `root_guarded_vectors_are_stable` vectors.
- **Root-signed owner ops**: `AdminApi.transferOwnership(groupId, newOwner, root)`,
  `changeNamespaceAdmin(namespaceId, newAdmin, root)`, `ownerDeleteGroup(groupId, root)`,
  `setTeeAuthoringPolicy(groupId, allowedMrtd, root)`, `disableTeeAuthoringPolicy(groupId, root)`
  and `setTeeAdmissionPolicy(groupId, request, root)` read `namespaceId` / `ownerOpCounter`,
  encode the op the node builds and mint `rootProof` with the root `DeviceSigner` (optional
  `keyEpoch` / `chain`); `AdminApi.mintRootProof(groupId, op, root)` is the shared step. The
  `rootProof`-taking forms are unchanged.
- **Account roots** (mero-js `account/account.ts` + `wordlist.ts`): `ENGLISH_WORDLIST` (BIP39,
  checked against its canonical SHA-256), `generateAccountRoot()` (24 words; the entropy *is*
  the Ed25519 seed — no PBKDF2, no BIP32, exactly as mero-js), `accountRootFromPhrase`,
  `accountRootFromSecret`, `AccountRoot` / `RecoverableAccountRoot`, `AccountRootSigner` /
  `NewAccountRootSigner` (a `DeviceSigner` with no `secret`; the seed is kept only in a
  `SecureStore` by `AccountRootKeyStore` and read per signature — `EncryptedPrefsSecureStore`
  on a device, since AndroidKeyStore has no Ed25519 below API 33), `createAccountRootSigner`,
  `accountRootSignerFromPhrase`, `resolveRoot` / `resolveRootPair` / `ResolvedRoot`, and
  `signAccountLink` / `signAccountLogin` (`AccountLinkInput` / `AccountLoginInput`; standard
  base64, reproducing mdma's verifier vector).
- **Device scopes** (mero-js `device-cert/device-scope.ts`): `signDeviceScope(DeviceScopeInput)`
  and `deviceScopePayload`, pinned to core's `DeviceScope::sign` vectors, and
  `AdminApi.linkAccountDevice(namespaceId, credential, root, applications, scopeEpoch)`, which
  signs the scope for the device the credential names. (`rescopeDevice` takes no signature:
  core runs it on the node holding the root.)
- **Request proofs** (mero-js `request/` + `request-proof/`): `requestBodyHash`, `signRequest`
  (`RequestSigInput`), `callerProof` (`CallerProofInput`), `createProofSigner`
  (`ProofSignerOptions`, or a `ProofCredential` for the two-link chain), `signRequestProof`
  (`RequestProofInput`), `REQUEST_PROOF_HEADER` (`X-Calimero-Proof`), `createProofAuthorizer`,
  `proofAuthorizerOf`, `signedPath`, `carriesProof`, `ProofRequest` / `RequestToAuthorize`,
  `ProofSigner` and `RequestAuthorizer`, pinned to core's `request_wire_fixture` and
  `proof_auth` (`chain_for(1, key(7))`) vectors.
- **`MeroConfig.requestProof`** (mero-js `MeroJsConfig.requestProof`): a
  `RequestProofInterceptor` signs each request's method, request-line path and query (base
  path included) and exact body. With a proof configured the OkHttp clients do not follow
  redirects, and a 3xx answering a proof-carrying request throws
  `ProofRedirectRefusedException` (status `0`, `redirectStatus`, `location`) — mero-js
  `refuseProofRedirect`. A signing failure surfaces as itself, not as a `NetworkException`.
- `SseClient`, `SseConnection` and `WsClient` take an `authorize: RequestAuthorizer` hook
  (called for `GET /sse`, each subscription POST over its body, and the `GET /ws` upgrade);
  `Mero` wires it to send the session token when there is one and the proof otherwise.
- `signerFromSecret(secret, label)` and `resolveSigner(secret, signer, label)` (mero-js
  `signer.ts`); `SeedSigner` takes an error `label`, and its `toString` names its public key.
- SDK Explorer: a **Signing** category (`generateAccountRoot`, `accountRootFromPhrase`,
  `signOwnerOpProof`, root-signed `transferOwnership`, `signDeviceScope`, `signRequestProof`).
- Docs: [Account roots](docs/src/content/docs/guides/account-roots.mdx) and
  [Signing offline](docs/src/content/docs/guides/signing.mdx) guides.

### Added — Calimero Cloud sessions and account linking (mero-js parity)

- **`CloudClient` session half** (mero-js `cloud-client.ts`): `signInWithGoogle`,
  `signInWithAccount` (root secret or signer) / `getAccountLoginChallenge` /
  `submitAccountLogin`, `refreshSession`, `signOut` (clears locally even when the server call
  fails), `getSession()` / `isSignedIn`, rolling refresh from `x-mdma-session-refresh`, and
  the new constructor parameters `sessionToken`, `onSession` and `sessionStore`
  (`CloudSessionStore` over any `SecureStore`). The session type is `CloudLoginSession`
  (+ `CloudUser`), so it does not clash with `DelegatedSession`. Session routes refuse
  locally with `MeroStateException` when nobody is signed in.
- `getMyNamespaces`, `getNamespaceRelays` (`CloudRelay` with `canExecute`, `teeRole`,
  `admission`, `join`), `findExecutingRelay` / `CloudClient.pickExecutingRelay`,
  `getMyMachines` and `findAdmitter`. mero-js `getMyNode` is **not** ported: it is
  deprecated there and reads a retired ledger.
- HA: `claimNamespace`, `enableNamespaceHa(ns, admitterAddrs)`,
  `enableFoundedNamespaceHa`, `disableNamespaceHa`, `enableHaUrl` / `disableHaUrl`
  (the `/enable-ha` and `/disable-ha` pages), `readHaPageCallback`, and
  `admitterAddrsFromNetworkStatus`. `signAccountOwnershipClaim` and
  `ACCOUNT_OWNERSHIP_AUDIENCE` (`mdma:enable-ha-namespace`) sign the session route's claim.
- Account linking: `linkAccount`, `getAccountLinkChallenge`, `submitAccountLink`,
  `linkAccountWithGrant` (anonymous), `getMyAccounts`, `CloudClient.accountLinkHandoff` /
  `readAccountLinkCallback`, and `CloudLink.cloudLinkUrl` / `readCloudLinkCallback` /
  `completeCloudLink` / `withoutLinkState`, over a single-use, account-bound, 10-minute
  `PendingCloudLinks` store (mero-js `link-state.ts` key and shape). A grant this app did
  not ask for is refused (`UnexpectedCloudLinkException` / `"unexpected_callback"`).
  `callbackIsForeign(returnTo, pageOrigin)`.
- `CloudWebLauncher` (Android) opens the HA pages and the link consent page in a Custom Tab:
  `enableHA`, `disableHA`, `beginCloudLink`, `openAccountLinkHandoff`.
- **`connectCloud(ConnectCloudOptions)` / `connectCloudWithAccount(...)`** (mero-js
  `connect.ts`) return a `CloudConnection` (`cloud`, `relay`, `namespaceId`, `namespace`,
  `relayInfo`, `client`, `canSubscribe`, `execute`). `client` is the existing
  `RelayConnection`, built over the same `RelayClient`. When nothing can be built yet, they
  throw `CloudNotReadyException` with mero-js's messages. The `seal` option is not ported
  yet: the sealed transport comes separately.
- Errors: `AccountHaRefusedException` now extends `HttpException` (existing `catch` blocks
  still work) and is thrown as one of `AccountNotLinkedException`,
  `AccountLinkedToSeveralUsersException`, `HaRequestPendingException`,
  `UnknownRelayException` or `RelayNotDialableException`, readable as a typed `HaRefusal`.
  It maps by status as well as code (409 vs 422), as mero-js does. `classifyError` reads
  `CloudNotReadyException` as `UNAVAILABLE` and `CloudLinkDeclinedException` as `FORBIDDEN`.
- `FakeCloud` serves `/api/auth/*`, `/api/cloud/me/*`, claim, enable/disable-ha and
  `/api/cloud/accounts/link`, with Bearer checks and an opt-in rolling refresh.
- Docs: [Calimero Cloud](docs/src/content/docs/guides/cloud.mdx) guide and
  [CloudClient](docs/src/content/docs/reference/cloud.mdx) reference.

### Breaking — delegated accounts (mero-js parity)

- **Security fix (mero-js #264): an invitation's unsigned `admitter_addrs` is never used as a
  relay.** It sits outside the inviter's signature, so an http(s) entry there could name a host
  that proxies a genuine relay. `Join.resolveRelayFromInvitation` no longer falls back to it:
  a namespace the cloud lists no node for is `JoinStep.NO_NODES`. And
  `CloudAccount.createNamespaceInvitation` refuses a namespace the cloud routes nowhere with
  `InvitationNotClaimableException` (`reason = InvitationNotClaimableReason.NOT_HOSTED`), where
  it used to put the relay's URL in `admitter_addrs`. `InvitationNotClaimableException` takes a
  `reason` (`NOT_HOSTED` / `NO_NAMED_NODE`); `notHosted` and the boolean constructor remain.
- **`CloudAccount.bootstrapFromInvitation` returns a `BootstrapResult`** (`Success(session, relay,
  published)` / `Failure(step, reason, status)`) and never throws for an expected failure, as in
  mero-js. It persists nothing. `JoinStep` (alias `BootstrapStep`) gains `INVITED_UNREACHABLE`
  and `NO_CREDENTIAL` and a `wire` spelling. `CloudAccount.joinAsAccount(namespaceId, …)` still
  throws `JoinException`, and now also records the relay in the account's relay map.
- **`AdminApi` implements `AdminSurface`.** Every method is now an `override` and its default
  arguments are declared on the interface. Calls compile unchanged. A subclass that overrode
  an `AdminApi` method must drop the defaults it re-declared.
- **Founding attests the relay afresh** (`foundDelegatedNamespace`, which
  `CloudAccount.foundNamespace` now delegates to): a pinned key no longer stands in for the
  attestation. Founding uses the first of the account's known relays, and learns the executor
  account from the relay map when the session does not name it.
- The HA refusal advice for `account_not_linked` is mero-js's current wording
  (`HA_ACCOUNT_NOT_LINKED_MESSAGE`); `AccountHaRefusedException.ADVICE` is `HA_REFUSAL_MESSAGES`.

### Added — delegated accounts and the unified transport (mero-js parity)

- **Relay map** (mero-js `session.ts`), per account in the `CloudAccount`'s `SecureStore` under
  mero-js's keys: `RelayMap`, `readRelayMap`, `rememberRelay`, `relayForContext`, `knownRelays`,
  `listDelegatedContexts` / `listDelegatedNamespaces` (the union across every known relay, each
  recorded as served by the relay that listed it; `DelegatedContext` / `DelegatedNamespace`),
  `carryExecutorAccount`, `rememberGroupNamespace` / `namespaceOfGroup`, `persistedNonces` and
  `markContextNonceSpent`.
- **Relay node key learning**: `CloudAccount.attemptRelayNodeKey(relayUrl, fresh)` →
  `RelayNodeKeyAttempt` (`Learned` / `Unavailable` / `Refused`), `resolveRelayNodeKey`,
  `learnRelayNodeKey(relayUrl, retryMs = RELAY_NODE_KEY_RETRY_MS)` (1, 2, 5, 10, then every
  30 s, cancelled with its coroutine, `null` on a refusal), `readPinnedRelayNodeKey`.
- **`CloudAccount.buildDelegatedClient(session, contextId?)`**: a relay-transport client whose
  requests carry the device certificate's request proof, whose reads try `/query` first (a
  `409` marks the method a write and falls back to a warrant), and whose events stay inert on a
  hosted relay until its key is known. `null` without a relay. The write-method cache is shared
  per relay, context and method, as in mero-js (`forgetMethodKinds()` for tests).
- **`MeroTransportClient`** (`com.calimero.mero.transport`, mero-js `MeroClient`):
  `MeroTransportConfig.Node` / `.Relay`, with `rpc` as an `ExecuteTransport`
  (`execute` / `executeWithMetadata` → `ExecuteResult(returns, transport, rootHash?)` /
  `migrateMyEntries` / `countMyPending`, plus `executeAs<T>`), `canSubscribe`, `node`, `relay`,
  `admin`, `auth`, `events`, `ws` (refused for a proof-only relay client), `ephemeral`
  (`TransportEphemeral.Node` / `.Relay`), `cloud` and `close()`. Also `NodeTransport`,
  `RelayTransport`, `RelayObserver` / `RelayObserveConfig` (a lazily minted, single-flight
  `account_proof` session that re-mints near expiry), `createMeroClient`, and
  `RpcExecution.TRANSPORT_RELAY`.
- **Account admin** (mero-js `account-admin.ts`): `createAccountAdmin(session, read, app, deps)`
  / `CloudAccount.accountAdmin(...)` return an `AdminSurface`. Writes become governance ops,
  delegated creations, or invitations signed by this device; node-only calls throw
  `NotForAccountException`; with no relay, listings are empty and other calls throw
  `NoRelayException`. `AccountApp`, `AccountAdminDeps` / `CloudAccountAdminDeps`.
  `createNodeAdmin(admin, app)` installs the app from the registry before `createNamespace`.
  `CreateNamespaceResponseData` gains `haEnabled` / `haError` (account admin only).
  `classifyError` reads `NotForAccountException` as `FORBIDDEN`, and `NoRelayException` /
  `InvitationNotClaimableException` as `UNAVAILABLE`, as Swift does.
- **Delegated governance** (mero-js `create-context.ts`, `govern.ts`): `createDelegatedContext`
  (creation nonce `0`, the relay remembered), `createDelegatedPrivateContext` (a restricted
  subgroup, its members, the context), `delegatedGovernance` (per-(relay, group)
  clock-floored nonces), `governGroup` / `governRoot`, `foundDelegatedNamespace`,
  `latestPublishedVersion`, `HA_ACCOUNT_NOT_LINKED_MESSAGE`, `HA_REFUSAL_MESSAGES`,
  `CreateDelegatedContextRequest` / `CreatedDelegatedContext`.
- **Invitation redemption** (mero-js `redeem-invitation.ts`): `redeemInvitation(namespaceId,
  invitation, InviteRedeemer)` → `RedeemOutcome` (`Joined` / `AlreadyMember` /
  `Failed(message, reason, retryable)`), `InviteFailureReason`, `isSettled`, `shouldRetain`,
  and `AdminSurface.redeemInvitation(namespaceId, invitation, groupName, teamName)`.
  `CloudAccount.joinAsAccount(session, namespaceId, invitation, cloudBaseUrl, onJoined)`
  remembers the relay and moves the session onto it. `ResolvedRelay.peerId`.
- `FakeCloud`: proof-authenticated reads (`acceptProofs`), `listedContexts`,
  `listedNamespaces`, `contextGroups`, `groupNamespaces`, context-intents, `attestStatus` /
  `attestBindsWrongKey`, `refuseNextGovernance`, `registryBundles`. A subgroup's create
  answers the new group id.
- SDK Explorer: `redeemInvitation`.
- Docs: [Delegated accounts](docs/src/content/docs/guides/delegated-accounts.mdx) guide and
  [Transport](docs/src/content/docs/reference/transport.mdx) reference.

### Breaking — TEE trust (mero-js parity)

- **The relay's node key is verified by its TDX quote, on the device.** `DcapRelayKeyVerifier` is
  now the default `RelayKeyVerifier` of `CloudAccount` and `RelayLogin` (it was
  `TlsRelayKeyVerifier`, which trusted TLS and checked only the binding). A hosted relay's key is
  learned only if its quote verifies with DCAP against Intel's root. Its image must be the
  `locked-read-only` profile of the signed mero-tee release it names, at `MIN_RELAY_RELEASE`
  (`2.3.99`) or newer. Its report data must bind our nonce and the key. A relay that fails any of
  these is `RelayNodeKeyAttempt.Refused`. If the release mirror does not answer, the attempt is
  `Unavailable` (`RelayKeyTransportException`). Mock quotes are still accepted from loopback
  relays. Pass `relayKeyVerifier = TlsRelayKeyVerifier()` for the old check.
- **A delegated session seals every request to a hosted relay.** Intents, admin reads, queries,
  the relay login and events go through `relayTransportClient(relayUrl)`, sealed to the relay's
  attested TD (mero-js `relayTransportFetch`). A relay that cannot seal is not reached in the
  clear: its requests fail. Logged-in calls go out plain only after the TD answers
  `403 sealed_route_unguarded` from inside the envelope. `CloudAccount(sealRelays = false)` opts
  out, and `forgetRelaySealing()` resets.
- **`connectCloud` seals a hosted relay by default.** With no `seal`, the relay is sealed against
  its signed release (`DcapRelayKeyVerifier.signedReleaseVerifier`), with the same
  `sealed_route_unguarded` fallback. `ConnectCloudOptions(sealRelays = false)` (and
  `ConnectCloudWithAccountOptions`) opts out for plain test rigs. mero-js seals only with `seal`.
  This follows the Swift SDK.
- `classifyError` reads `RelayKeyTransportException` as `UNAVAILABLE`, retryable.

### Added — TEE trust: signed releases, DCAP, sealed transport, agent gate (mero-js parity)

- **Sigstore** (`com.calimero.mero.tee`, mero-js `sigstore.ts`, `sigstore-trust-root.ts`,
  `x509.ts`): `Sigstore.verifyCosignBundle` and `chainsToFulcio`, against the embedded trust root
  `FULCIO_AUTHORITIES` / `REKOR_LOGS` (copied exactly). DER, X.509 and ECDSA P-256/P-384 helpers
  are internal and built on Bouncy Castle. Nothing reads the clock: certificates are checked at
  Rekor's `integratedTime`.
- **Signed node releases** (mero-js `release.ts`): `NODE_RELEASE_SIGNER`,
  `DEFAULT_RELEASE_MIRROR`, `SignedNodeRelease`, `verifySignedNodeRelease`, `fetchNodeRelease`,
  `nodeReleaseVersionFromImage`, `fetchNodeReleaseVersion`, `cloudNodeReleaseUrl`,
  `trustSignedRelease`, `createSignedReleaseVerifier` (`SignedReleaseVerifierOptions`) and
  `createSignedReleaseSealedFetch` (`SignedReleaseSealedFetchOptions`, `NodeReleaseRequest`).
  Release versions are compared as semver.
- **DCAP quote verification** (mero-js `verify.ts`): `createQuoteVerifier(QuoteVerifierOptions)`,
  `PRODUCTION_PROFILE` (`locked-read-only`), `trustedMeasurementsFromReleases` → `TrustedImages`,
  `PublishedMrtds`, `TrustedMeasurements`, `DcapCollateral` (hex or byte arrays), `DcapVerifiedReport`,
  `DcapReport` (`Sgx` / `Td10` / `Td15`), `DcapVerify`, `VerifyTransportQuote`, `QuoteVerifier`,
  `TransportQuote`. `createQuoteVerifier` makes exactly the checks `verify.ts` makes, including
  refusing a debug TD whatever `dcapVerify` allows.
  - `Dcap` is a built-in `DcapVerify`, a port of `@phala/dcap-qvl` 0.5.3's `verify`, so quotes are
    verified on the device with no third-party library.
  - `PccsCollateral.fetch` ports dcap-qvl's `getCollateral`, for quotes that carry their PCK chain.
- **Sealed transport** (mero-js `noise.ts`, `sealed.ts`, wire-identical with core's
  `crates/server/src/sealed.rs`): `SealedTransport` is an OkHttp `Interceptor` with a `.client`. It
  - refuses URLs outside its base URL;
  - takes the transport key as bytes or as a suspend attest step;
  - re-attests and retries once when the node restarted;
  - renews and replaces sessions, and waits out `busy`;
  - streams sealed responses frame by frame;
  - fails with `StaleTransportKeyException` / `SealedTransportException`.

  Also `HANDSHAKE_PATH`, `SEALED_PATH`, `SEALED_CONTENT_TYPE`, `transportKeyBinding`,
  `fetchAttestedTransportKey` (`TransportAttestation`, or an `AdminSurface`) and
  `createAttestedSealedFetch`. The handshake, the envelope, the frames and the binding are pinned
  to core's published vectors.
- **Sealing wired in**: `ConnectCloudOptions.seal` / `ConnectCloudWithAccountOptions.seal` (the
  relay only, as in mero-js). `MeroConfig.httpClient` is the client `Mero` builds on, with its
  auth and proof interceptors in front, so a sealed client seals a node's whole surface, SSE
  included.
- **Relay node-key attestation** (mero-js `attest-node-key.ts`): `attestRelayNodeKey`
  (`AttestedNodeKey`), `attestKeyBinding`, `reportDataOf` (`QuoteReportData`), reconciled with
  `RelayAttest`. Also `DcapRelayKeyVerifier`, `AttestingRelayKeyVerifier`, `MIN_RELAY_RELEASE` and
  `RelayKeyTransportException`.
- **Agent gate** (`com.calimero.mero.agentgate`, mero-js `agent-gate/`, wire-identical with
  mero-tee's mero-agent-gate `/attest`, `/provision`, `/release`): `attestAgent`, `checkOwner`,
  `provisionAgent`, `releaseAgent`, `generateOwnerKey`, `ownerPublicKey`, `validateAgentId`,
  `agentKeyBinding`, `provisionInfo`, `AGENT_ATTEST_DOMAIN`, `AGENT_PROVISION_INFO_DOMAIN`,
  `AgentGateException`, `VerifiedAgent`, `OwnerKey`, `ProvisionedAgent`, `KMS_RELEASE_SIGNER`,
  `AgentPolicy`, `verifySignedAgentPolicy` and `agentQuoteVerifier`.
  - HPKE Auth mode (X25519, HKDF-SHA256, ChaCha20-Poly1305) is pinned to the gate's Rust-sealed
    fixture and cross-checked against Bouncy Castle's HPKE.
  - A verifier that returns `false` refuses here, where mero-js refuses only one that throws.
- `TeeTrustException`: every trust check's refusal, with the reason.
- Fixtures: mero-js's `src/sealed/fixtures/` (the real mero-tee 2.3.86 / 2.3.87 and mero-kms
  2.3.87 signed files, dcap-qvl's sample TDX quote and collateral) and the agent gate's
  `rust-sealed-bundle.json`, copied byte for byte into `mero-core/src/test/resources/fixtures/`.
- Docs: [TEE trust](docs/src/content/docs/guides/tee-trust.mdx),
  [Sealed transport](docs/src/content/docs/guides/sealed-transport.mdx) and
  [Agent gate](docs/src/content/docs/guides/agent-gate.mdx) guides.

### Changed — SSE reconnect backoff

- **Breaking:** `SseConnection(reconnectDelayMs = …)` and `SseClient(reconnectDelayMs = …)`
  are replaced by `backoff: ReconnectBackoff`, and
  `SseConnection.DEFAULT_RECONNECT_DELAY_MS` is gone. The default,
  `ReconnectBackoff.SSE`, waits 3 s first (as before, and as mero-js), then doubles
  up to 30 s while attempts keep failing, ±20% jitter, and resets once a stream
  opens — the Swift SDK's policy. `ReconnectBackoff.fixed(3_000)` is the old flat
  3 s (mero-js `reconnectDelayMs`); `ReconnectBackoff.WEB_SOCKET` names
  `WsClient`'s unchanged `1 s · 2ⁿ ≤ 30 s` schedule.

### Fixed — mero-compose

- mero-compose applies the kotlinx.serialization plugin: the invitation
  pending-intent store's `@Serializable` model did not compile in the Android build.
- `MeroClient` learns an account's application id over the relay connection it
  already holds, instead of listing contexts through a new client per relay. That
  second relay login superseded the live session on a relay keeping one session per
  device, so the next query 401'd and a read fell through to a warrant.
- mero-compose publishes a placeholder javadoc jar: AGP 8.5's Dokka cannot read
  sealed classes compiled for Java 17 (`PermittedSubclasses`), which mero-compose's
  API now names. The KDoc ships in the sources jar.
- `buildInviteLink(host = "calimero:/")` (or `"calimero:"`) gives
  `calimero://<slug>/join?invitation=…`; it used to drop a slash.
- `normalizeNodeUrl` leaves a URL that already names a scheme alone, so
  `isValidNodeUrl("ftp://node.example")` is false instead of reading it as
  `https://ftp://…`.

### Changed — mero-compose (mero-react parity)

- `ConnectButton` follows mero-react: "Connect" opens a `LoginSheet` in a dialog (or calls
  `onConnect`), "Reconnecting..." while a node's stream is down, "Connected" with a menu
  (account or node, relay note, Log out). It no longer opens the wallet directly; new
  `labels`, `logoOnly`, `tabs` and `theme` parameters.
- `LoginSheet` has **Node** and **Cloud** tabs (`tabs`, `initialTab = LoginModalTab.CLOUD`) and
  its colours default to `LoginSheetColors.from(LocalMeroTheme.current)` (mero-react's dark
  `defaultMeroTheme`). Pass `colors = LoginSheetColors()` and `tabs = listOf(LoginModalTab.CLOUD)`
  for the previous light, Cloud-only sheet (the sample does). Existing test tags are unchanged.
- `MeroClient.signOut` / `logout` also clear `MeroAppStorage` (node URL, application, context)
  and the node session; `restore()` returns whether a session came back and restores a stored
  node session when no Cloud account is signed in. `MeroAuthState.isDevNodeSession` now needs a
  `username` (an SSO node session is `isNodeSession`).

### Added — mero-compose (mero-react parity)

- **Provider**: `AppMode` (`MULTI_CONTEXT`, `ADMIN`, deprecated `SINGLE_CONTEXT`) and
  `permissionsFor(mode)`; `MeroCapabilities` (`NODE`, `account(hasPackage)`, `NONE`) as
  `MeroClient.can`; `MeroAppConfig` (mode, package, registry, cloud, `allowedNodeUrls`,
  timeout). `MeroClient.connectToNode` / `nodeLoginUrl` / `handleAuthCallback` /
  `handleDeepLink` / `restoreNodeSession` sign in through a node's own login page, adopting the
  callback only from a trusted node (`sameOrigin`, `resolveTrustedNodeUrl` — mero-react
  `auth/node-trust.ts`) and only when safe (`decodeJwtClaims`, `resolveTokenAdoption`,
  `TokenAdoptionReason` — `auth/token-adoption.ts`), then validate with `/auth/validate`.
  `MeroAuthState` gains `isOnline`, `applicationId`, `contextId`, `contextIdentity`,
  `isNodeSession`, `isDelegated`. `isOnline` follows the node's SSE (`OnlineMonitor`,
  `streamFailureVerdict`: a revoked family or a `403` with an `x-auth-error` logs out, a `401`
  recovers). `MeroClient.session` (`MeroSession`: session `admin` — `createNodeAdmin` or
  `createAccountAdmin` — `rawAdmin`, `rpc`, `events`, `presence`, `isDelegated`, `delegated`,
  `can`), `admin`, `isDelegated`, `delegatedSession`, `connectWithAccount` (in memory),
  `selectContext`; an account's `applicationId` is resolved from the registry by `packageName`,
  else from its contexts. `MeroAppStorage` (SharedPreferences, mero-react's keys) with
  `KeyValueStore`, `SharedPreferencesStore`, `MemoryKeyValueStore`.
- **State holders** (`com.calimero.mero.compose.state`): `AsyncResource` / `ResourceState`,
  `AsyncMutation` / `AdminMutation` / `MutationState`, `NotConnectedException`; `MeroReads`
  (contexts, groups, members, subgroups, capabilities, visibility, upgrade status, metadata,
  namespaces, node identity, blob info, `isBlobNotFound`); a write holder per write hook
  (`CreateContext`, `CreatePrivateContext`, `JoinContext`, `JoinNamespace`, `RedeemInvitation`,
  `UpgradeGroup`, `InstallFromRegistry`, `UploadBlob`, …); `ExecuteState`,
  `SubscriptionState`, `EphemeralState`, `ContextDiscoveryState`, `GroupCapabilitiesState`,
  `MigrationEventsState`, `MigrationStatusState` (SSE, 5 s poll fallback), `AppVersionState`,
  `LatestVersionState`, `GroupAppVersionState`, `MyAuthoredMigrationState`, `BlobFileState`;
  and `remember*` composables for each.
- **Invitations** (`com.calimero.mero.compose.invitations`): `InvitationCapture`,
  `primeInvitationCapture`, `onInvitation`, `peekInvitation`, `subscribeInvitation`,
  `CapturedInvitation`, `buildInviteLink`, `invitationFromRaw`, `urlWithoutInvitation`,
  `MAX_AUTO_JOIN_ATTEMPTS`; `InviteRedemption` / `InviteState` / `ParsedInvitation`,
  `describeInviteFailure`, `JustJoinedNamespaces`, `JoinSync` (+ `JOIN_SYNC_*`),
  `InvitationDialogModel` / `invitationDialogCopy` / `INVITATION_READY_MS`,
  `decideInvitationGate` / `invitationGateCopy`, `JoinInvitation` / `JoinInvitationStep` /
  `isFinalInvitationError`, `DelegatedBootstrap`; `AccountEnrolment` and `isReturningFromWallet`.
- **Composables**: `MeroTheme` / `defaultMeroTheme` / `lightMeroTheme` / `LocalMeroTheme` /
  `ProvideMeroTheme`, `CalimeroLogo` / `CalimeroLogoVector`, `AccountSignInPanel`,
  `LoginModalTab` (`LoginTab`) with `NodeLoginState`, `isValidNodeUrl`, `localDiscoveryHost`,
  `MigrationPendingBanner`, `MigrationAdminPanel`, `InvitationDialog`, `InvitationGate`,
  `JoinSyncBanner`, `rememberInviteRedemption`, `rememberJoinSync`, `rememberPendingInvitation`.
- Not ported: mero-react's `useNamespaceIdentity` (core serves no namespace-identity route),
  `MigrationStatus.fleetCompletedAt` (not in the Kotlin type yet).
- Docs: [Compose frontend](docs/src/content/docs/guides/compose-frontend.mdx),
  [Compose state holders](docs/src/content/docs/guides/compose-state.mdx) and
  [Invitations in Compose](docs/src/content/docs/guides/compose-invitations.mdx).

### Changed — events and presence

- An SSE `401` whose `x-auth-error` is `token_reuse` / `token_revoked` now ends the
  stream like a `403` (`AuthRevokedException`); a `401 token_expired` still reconnects.
- A group-only SSE subscription no longer sends an empty `contextIds` (both lists are
  sent only when non-empty, as in mero-js; core defaults either).
- `RelayClient.presenceIntent` is no longer retried on `429` (presence resends every
  2.5 s anyway, as in mero-js).
- The re-subscribe after a `connect` frame runs beside the frame reader instead of
  blocking it, so a frame that follows `connect` closely may be delivered before the
  subscription POST returns.

### Breaking

- `RpcClient.execute` / `executeRaw` / `executeWithMetadata` no longer take
  `executorPublicKey`. Core's execute request is `deny_unknown_fields` with only
  `contextId`, `method` and `argsJson`, so a call that named one was refused.
- SSE sends the access token as `Authorization: Bearer` instead of `?token=` in
  the URL, matching the Swift SDK and keeping tokens out of proxy logs.

- **Re-login after the node upgrade.** core rc.83 added a *required* `key_id`
  claim to its JWTs, so every access and refresh token minted by an older node
  fails verification. Stored sessions are dead: apps must authenticate again.
  The SDK now drops the stored bundle when `/auth/refresh` answers `401` (an
  unrefreshable pair never becomes refreshable), so `isAuthenticated` turns
  `false` instead of every call failing the same way. Refresh always sends the
  access/refresh pair from **one** token response — rc.83 refuses a pair whose
  `key_id`s differ ("Access and refresh tokens do not belong to the same key").
- **`AdminApi.getCertificate()` removed.** core removed `GET
  /admin-api/certificate` (a928b5ed4).
- **`AdminApi.teeVerifyQuote()` and `TeeVerifyQuoteRequest` /
  `TeeVerifyQuoteResponseData` removed.** `POST /admin-api/tee/verify-quote` was
  removed from core before rc.41 and has answered 404 since.
- **`CreateGroupRequest.groupId` removed.** Group ids are derived now (a
  namespace root's from its founder and a salt, a subgroup's from its create),
  and the body is `deny_unknown_fields`, so naming one is a `400`. A new typed
  `createGroup(CreateGroupRequest)` overload sits beside the map one; do not put
  `groupId` in the map either.
- **Subgroup visibility is always sent.** rc.83 flipped what an absent
  `visibility` means on `POST /namespaces/{id}/groups` — *restricted* before,
  **open** now. `createGroupInNamespace` therefore always names one, filling
  `"open"` (`CreateGroupInNamespaceRequest.VISIBILITY_OPEN`) when the request
  leaves it `null`, so the same call creates the same subgroup on every node.
  Ask for `VISIBILITY_RESTRICTED` for a membership boundary. A `null` request
  used to send `{}`; it now sends `{"visibility":"open"}`.
- **TEE admission policy validation is stricter (node-side).** In the
  measurement form `allowedMrtd`, `allowedRtmr1`, `allowedRtmr2` and
  `allowedRtmr3` must all be non-empty, even with `acceptMock`. Changing the
  policy is a root-guarded owner op (see below).

### Added — auth

- `AuthApi.logout(LogoutRequest)` — `POST /auth/logout {refresh_token}` retires
  the refresh token server-side. `Mero.logout()` calls it first, best-effort,
  then revokes by `clientId` when given, then always clears the local store.
- `GenerateClientKeyRequest.applicationId` / `ttlSecs`. Client keys now expire
  (30 days by default, no non-expiring key), cannot be granted `keys:*`, and get
  `admin` only when bound to no context.

### Added — admin surface

- Reads as the account: `queryContext(contextId, QueryContextRequest)`.
- Delegated execution (types and raw calls; minting warrants is the account
  layer's job): `getIntentRelay` → `IntentRelayInfo` (`executorKey`,
  `releaseBytecodeId`, `releaseVersion`, `grantedOnGroupId`),
  `createContextIntent` / `getContextIntentRelay`, `governanceIntent` /
  `getGovernanceIntentRelay`, `presenceIntent`.
- `getWarrantNonce` / `getWarrantNonceAsAuthor` → `WarrantNonceState`
  (`Open(nextNonce: ULong)` / `Exhausted`), u64-exact. **Not served by rc.83**:
  a `404` throws `WarrantNonceRouteUnavailableException`.
- Account root and devices: `signWithAccountRoot` (`/account/sign-with-root`,
  `AccountSignDomains`), `linkAccountDevice`, `sealToAccount` → `SealedEnvelope`.
- Root-guarded owner ops, each with an optional `rootProof`:
  `transferOwnership`, `changeNamespaceAdmin`, `ownerDeleteGroup`,
  `setTeeAuthoringPolicy` / `disableTeeAuthoringPolicy`. A stale owner-op
  counter is a `409` — re-read `GroupInfo.ownerOpCounter` and re-sign.
- TEE admission policy, union form: `SetTeeAdmissionPolicyRequest.measurements(…)`
  and `.signedRelease(…)`, plus `mode` (`TeeAdmissionMode.REPLICA` / `RELAY`) and
  `rootProof`. The rc.41 positional constructor still compiles. The GET adds
  `enabled`, `signedRelease`, `mode`.
- `teeAttest` flags `bindNodeKey` / `bindTransportKey` / `includeCollateral`;
  the response adds `boundPublicKey`, `transportPublicKey`, `collateral`.
  `teeRegistrationAttest` (`POST /tee/registration-attest`).
- `openToDelegatedExecution(groupId)` / `grantAuthorship(groupId, account)` —
  read-modify-write helpers for the new `Capabilities.CAN_AUTHOR_ON_BEHALF`
  (`1 shl 9`).
- `listGroupMemberDevices`, and `offset` / `limit` paging on `listMemberDevices`.
- `getBlob` / `getBlobInfo` take an optional `contextId` (`?context_id=`).
- Typed ownership proofs: `issueOwnershipProof(groupId, IssueOwnershipProofRequest)`
  and `issueNamespaceOwnershipProof(groupId, IssueNamespaceOwnershipProofRequest)`
  → `IssueOwnershipProofResponseData` (with `founding` and `credential`). The
  map-based overloads remain.
- Response fields: `Namespace.founding` / `heldOps`,
  `CreateNamespaceResponseData.founding`, `GroupInfo.namespaceId` /
  `ownerOpCounter`. `GroupRoles` names the role strings, including the new
  `RelayTee`.

### Added — errors, RPC, events

- `HttpException.errorMessage` / `errorType` / `errorData` / `authError`. core
  now answers refusals with typed statuses (400/403/404/409/503 instead of a
  blanket 500), and `/intents` / `/query` method errors carry JSON-RPC's
  `type` / `data` beside `error`.
- `ReadOnlyWriteRefusedException` (a `RpcException`) for JSON-RPC's new
  `ReadOnlyWriteRefused`: this node's role in the context is read-only, so the
  write was discarded.
- `RpcClient.executeWithMetadata` → `RpcExecution(returns, transport = "node")`,
  mero-js's shape.
- SSE group subscriptions: `Mero.groupEvents(groupIds)` / `nodeEvents(contextIds,
  groupIds)` deliver `GroupEvent`s (`result.groupId` frames). `groupIds` is sent
  only when non-empty. `ContextEvent.presenceAccount` reads the relayed account
  on presence events. The stream still authenticates with `?token=`, which rc.83
  keeps accepting on `/sse` (and only there, `/ws` aside).

### Tests

`Rc83WireShapeTest` asserts exact request key sets for every new body and
decodes core's own rc.83 wire fixtures (`crates/server/primitives/fixtures/wire/`,
copied verbatim to `src/test/resources/fixtures/rc83/`) in both directions.
`SseGroupEventsTest` covers group subscriptions.

### Breaking — mobile sign-in is Calimero Cloud only

`mero-compose` no longer signs in to a node. `LoginSheet` is one **Continue with Calimero** button:
the person approves this device in the Calimero wallet (a passkey, in a Chrome Custom Tab), the
wallet redirects back with a device certificate, and the app talks to the account's hosted relay.

- `MeroClient` is rebuilt around `CloudAccount`: `signInWithCloud(context, callbackUrl)`,
  `handleEnrolmentCallback(url)`, `restore()`, `joinWithInvitation(...)`, `signOut()`, and a
  `MeroAuthState` with `account`, `device`, `relayUrl`, `sessionReady`, `relayNote` and
  `signedInWithoutRelay`. `relay` (warrants + query) and `mero` (admin/SSE on the relay) appear once
  connected. `MeroClient.create(context)` no longer takes a node URL.
- Removed from `mero-compose`: `MeroClient.login(username, password)`, `startSsoLogin`,
  `handleAuthCallback`, and the node-URL / credential fields of `LoginSheet`. `ConnectButton` now
  takes the `callbackUrl`. Core's `Mero.authenticate(...)` and the SSO helpers stay for servers,
  tools and tests that talk to a self-hosted node.

### Added — founding spaces and inviting as a Cloud account

- `relay.GovernanceOps`: every delegable governance op encoder mero-js exports (`memberAddedOp`,
  `memberRemovedOp`, `targetApplicationSetOp`, `defaultCapabilitiesSetOp`, `groupMetadataSetOp`,
  `subgroupCreation`, `namespaceCreatedOp`, `foundedNamespaceId`, ...), pinned by core's vectors.
- `RelayClient.foundNamespace` (the `NamespaceCreatedV2` genesis, then application and mask);
  `CloudAccount.foundNamespace` (executor from the attested relay key, name, best-effort HA via the
  new `CloudClient.enableHaAsAccount` and its signed founder claim, `AccountHaRefusedException`).
- `Invitations.signGroupInvitation` and `CloudAccount.createNamespaceInvitation`: invitations
  signed by the device key, with relay admitters and the not-hosted `admitter_addrs` fallback.
- `ApplicationIds` (`applicationIdForBundle`, `resolveFromRegistry`).
- `MeroClient.foundNamespace` / `createNamespaceInvitation`; the sample chat creates a space and
  shares an invite.
- Renamed `relay.WarrantNonceState` to `RelayWarrantNonceState` (no clash with `admin.WarrantNonceState`).

### Added — the Cloud account layer (`mero-core`)

A port of mero-js 24.5.0's delegated-account layer, byte-for-byte with core 0.11.0-rc.83 and pinned
by mero-js's golden vectors (which are core's own fixtures):

- `crypto`: `domainHash`, little-endian borsh writer, Ed25519 sign/verify and X25519 keygen over
  Bouncy Castle (`org.bouncycastle:bcprov-jdk18on` is a new dependency: the JCA has neither below
  API 33). `DeviceSigner` / `SeedSigner`.
- `account`: `DeviceKeys` (Ed25519 signing + X25519 delivery pair) in a `SecureStore`
  (`EncryptedPrefsSecureStore` / `MemorySecureStore`); `Enrolment.deviceEnrolmentUrl`,
  `readEnrolmentCallback`, `completeDeviceEnrolment`; `DeviceCert.verify` / `parse` (a port of
  `verifyDeviceCredential`); `CloudAccount` running the whole flow; `NamespaceOp.signMemberJoinOp`
  (schema 24) and `joinAsAccount` / `bootstrapFromInvitation` through `POST …/namespaces/{ns}/admit`.
  Any absolute callback URL is accepted (an https App Link or an app scheme).
- `cloud`: `CloudClient` (`getAccountRelaysChallenge`, `getAccountRelays`, `getRoutingChallenge`,
  `getNamespaceRouting`) with `X-Calimero-Credential/Nonce/Signature` routing proofs, and
  `chooseRelay`.
- `relay`: `RelayClient` — `describe` / `execute` (warrant v2 with `executorKey`,
  `releaseBytecodeId`, `releaseVersion`), `query` and `call` (query, falling back to a warrant on
  409), `describeCreation` / `createContext`, `describeGovernance` / `govern`, `presenceIntent`,
  `getWarrantNonce` / `getWarrantNonceAsAuthor`. Nonces come from a per-relay `PersistedNonceSource`;
  a warrant refused for its nonce is recovered from the relay's warrant-nonce route and re-signed
  once. `RelayLogin` learns the relay's node key from `POST /admin-api/tee/attest` behind a
  pluggable `RelayKeyVerifier`, then logs in (`/auth/challenge` → `/auth/token`,
  `auth_method: account_proof`, signed `LoginStatement`) for Bearer tokens the existing admin and
  SSE clients use.

### Known gaps

- ~~**Relay node key.** The default `TlsRelayKeyVerifier` does not verify the TDX DCAP quote.~~
  Closed in Unreleased: `DcapRelayKeyVerifier` verifies the quote on the device.
- **App-scheme callbacks** work in the SDK, but the hosted wallet only redirects to `https://`
  today. Until mero-wallet accepts app schemes, use a verified https App Link as the callback.

## Superseded — core 0.11.0-rc.44

Pinned to `0.11.0-rc.44` in `ci/core-version`, so every node lane — the two
merobox sync scenarios, the chat-sync scenario and the `merod`-booting e2e jobs —
boots that release.

### rc.41 → rc.44: no wire change

`calimero-server-primitives` (every admin request and response type) is
byte-identical between rc.41 and rc.44, and no route this SDK calls moved. The
rest is additive or server-side: delegated sessions may read the namespaces,
groups and contexts they are in (core#4038/#4039/#4042/#4045), not-found /
not-a-member errors answer 404/403 instead of 500 (core#4007), and
`account/link-proof` became `account/sign-with-root` (core#3992) — never called
here. `merod --public-intents` is now `--delegated-access` (core#4048); no script
in this repo passes it.

## Superseded — core 0.11.0-rc.41

### Breaking — the event transports now need a permission

core rc.41 (core#3942) mapped `/sse`, `/sse/subscription`, `/sse/session/{id}`
and `/ws` to **`context:subscribe`**. They required *no* permission before, so a
token minted for any purpose at all could open a stream.

- **`context:subscribe` is not implied by `context`.** core's
  `ContextPermission::All` arm matches only another `All`. `admin` does cover it,
  which is what `mero.authenticate()` asks for — so credential login is
  unaffected. A **scoped** SSO login (`AuthLoginOptions.permissions`) must name
  `context:subscribe` or every stream is refused.

- **A `403` on the stream is now terminal.** `SseClient` treated every failure
  alike and reconnected after three seconds, forever. Both of the things that
  produce a 403 here — a token without the grant, and a revoked refresh family
  (which core answers with `x-auth-error` and an EMPTY body) — are permanent, so
  that loop was a silent outage: a live-looking subscription delivering nothing,
  with no exception ever reaching the collector. A revoked family now throws
  `AuthRevokedException`, any other 403 a plain `HttpException`. Everything that
  is not a 403 still reconnects.

  The subscription POST is covered too: the stream can open and the *subscribe*
  still be refused, which used to read as "this context is quiet".

### Added — rc.39 → rc.41 surface

The admin request surface is otherwise **additive** across rc.38 → rc.41: no
existing request struct lost, renamed or closed a field. What is new:

- `rescopeDevice(deviceId, RescopeDeviceRequest)` — `PUT
  /admin-api/account/devices/{deviceId}/scope`. The direction `relinkDevice`
  cannot go: relink is add-only, this replaces. Body `{scope}`, where a
  `DeviceScope` is the bare string `"all"` or `{"only":[…]}` — serde's
  externally-tagged enum. An empty `only` is a `400`, deliberately, rather than
  the narrowest slip becoming the widest grant. Must run on the node holding the
  account root.
- `labelDevice(deviceId, LabelDeviceRequest)` — `PUT
  /admin-api/account/devices/{deviceId}/label`. Body `{label}`. The name
  replicates and comes back on `AccountDevice.label`; `labelEpoch` orders it
  against a rename made elsewhere at the same moment.
- `AccountDevice.label` — the name above, `null` while a device has none.
- `NodeIdentity.revokedFrom` — which account withdrew this node's device, `null`
  on a node no revocation has reached. A node reading this non-null still speaks
  locally, but nothing it publishes is accepted.
- `GetContextIdentitiesResponseData.identitiesOf` — `members` / `node` /
  `caller`. rc.41 made `identities-owned` mean *"the identities **you** can act
  as here"* for a delegated caller while keeping the node-wide reading for a
  node-owner session, so the same call on the same context answers differently
  per token. Kept a `String` rather than an enum so a reading a later core adds
  decodes instead of throwing; `null` from a node predating the field.

### Tests

`Rc41WireShapeTest` asserts the **exact key set** of both new bodies, from their
first commit rather than after the first outage — the two routes arrived
`deny_unknown_fields`. A `contains` assertion would pass with a fatal key beside
it, which is how `requester` rode eighteen bodies undetected until rc.38.

It also pins both directions of each added response field: the field decodes, and
a body from a node predating it — which **omits** the key, because core skips
rather than nulls it — still decodes, to `null`.

`SseForbiddenTest` covers the three outcomes that used to be one: a bare 403
fails the flow after exactly one attempt, a revoked family surfaces as
`AuthRevokedException`, and a 500 still reconnects.

## Superseded — core 0.11.0-rc.38

Verified against a real `merod 0.11.0-rc.38`.

### Breaking — keys core now refuses

core rc.32 → rc.38 added `deny_unknown_fields` to **37** admin request structs
that had been permissive. Nothing about this SDK changed; what changed is that
an extra key it had been sending — tolerated and ignored for releases — became
a 400 or 422 for the whole call.

- **`CreateGroupInNamespaceRequest` is `{groupName, visibility}`.**
  `POST /admin-api/namespaces/{id}/groups` is not the group-create body; it
  reads `groupName` and `visibility`. This request said `name` and `groupId`,
  and **both** are refused — `422 unknown field \`name\`, expected \`groupName\`
  or \`visibility\``. So the only call that ever worked was the one that named
  nothing: naming a subgroup, the reason to pass a request at all, has been
  failing outright. The sample app's "create channel" hit exactly this.
  `visibility` is new here and saves the follow-up
  `setSubgroupVisibility` call.

- **`requester` is gone from every request type (18 of them).** core has never
  had such a field — not at rc.32, not at rc.38. It was omitted whenever null
  (`explicitNulls = false`), which is why it never showed: harmless until a
  caller set it, then a 400 for the whole call. Six types held nothing else and
  are now empty `@Serializable class`es, so `SyncGroupRequest()` still compiles.

### Tests

`Rc38RequestShapeTest` asserts the **exact key set** of each body. That is the
only assertion that catches this class of break — checking that the keys you
care about are present passes just as happily with a fatal one beside them,
which is how `requester` rode along on eighteen requests unnoticed.

The live-node suite gained the provisioning chain: create a namespace, create a
**named** subgroup, list it back. Everything it did before either read, or
asserted a request shape against a mock — and a mock cannot notice that the node
stopped accepting what the SDK sends.

## Superseded — core 0.11.0-rc.32

Verified against a real `merod 0.11.0-rc.32`, and against a second one for the join.

### Breaking — the SDK could not do these against a current node

- **`InstallApplicationRequest` is `{package, version}`.** core#3652 (rc.31) made
  distribution registry-only: the node fetches from its own `[registry]`, and the
  request carries `deny_unknown_fields`, so the old body naming `url` is
  **refused** — `400 unknown field \`url\`, expected \`package\` or \`version\``.
  `installFromRegistry(registryUrl, package, version)` loses its first argument.
  ⚠️ Discover versions with **`getRegistryVersions`** (the registry read), not with
  `listPackageVersions` / `getLatestPackageVersion` — those ask the *node*, which
  reports what it has **installed**, so for a package it has never seen they answer
  `{"versions":[]}` / `{"version":null}`. That reads like "unpublished" and is a
  different question.
  `InstallDevApplicationRequest` is `{path}` alone, and the path must be an
  `.mpk` — a raw `.wasm` is refused with *"not a signed application bundle"*.

- **Invitations are carried verbatim.** `SignedGroupOpenInvitation` and
  `GroupInvitationFromAdmin` now hold the object core sent and re-encode it key
  for key, with typed accessors over it. Naming only the fields the model knew
  dropped the rest on re-encode — including **`admitters`**, which is inside the
  *signed* body and which core#3714 (rc.29) made non-empty on every invitation.
  The node re-encodes to borsh and checks the signature, so on one rc.32 pair,
  same namespace, two freshly minted invitations:
  `stripped → 500 invalid invitation signature` · `verbatim → 200, joined`.

- **`upgradePolicy` is gone** from `Namespace`, `GroupInfo`, `CreateNamespaceRequest`
  and `CreateGroupRequest`, along with the `UpgradePolicy` enum (core#3485 — it
  always held `"LazyOnAccess"`). Declared required, it threw
  `MissingFieldException` on every `listNamespaces` / `getNamespace` /
  `getGroupInfo`; `ignoreUnknownKeys` cannot help with a *missing* field.
  `Namespace` gains `appVersion`, `GroupInfo` gains `groupStateHash`.

- **Timestamps and sizes are `Long`.** They are `u64` in core.
  `MetadataRecord.updatedAt` is wall-clock **milliseconds** and overflowed a
  32-bit `Int` on the first real body carrying a metadata record
  (`Failed to parse int for input '1788952411519'`), taking `getGroupInfo` and
  all three metadata getters with it. Also `size` / `sizeInBytes` (a blob may
  exceed 2 GB), `createdAt`, `initiatedAt`, `completedAt`, `reportedAt`,
  `expirationTimestamp`.

- **`syncContext(null)` posted to a path that 404s.** It built
  `"/contexts/sync/${contextId ?: ""}"`, and axum 0.8 (core rc.30, #3744) stopped
  matching a trailing slash to its route — `POST /contexts/sync` is 200,
  `POST /contexts/sync/` is **404**. So "sync every context" broke at rc.30 on a
  call that had worked for every release before it. The two paths are built
  separately now.

- **Four methods removed — their routes do not exist.** Probed live:
  `getNamespaceIdentity` (404, deleted rc.23 core#3522 — use `getNodeIdentity`),
  `registerGroupSigningKey` (404, rc.21 core#3439),
  `inviteSpecializedNode` (405, rc.17 core#3267 — the path now matches
  `/contexts/:id`, so it resolves to a *different* route), and
  `updateGroupSettings` (405; its only field was `upgradePolicy`).

- **The alias list routes answer a map, and there is no `identity` scope.**
  `GET /alias/list/{context,application,device}` return
  `{"data": {"<alias>": "<value>"}}` (`{}` when empty), not `{aliases: […]}` —
  modeled as a list, all three list methods threw
  `MissingFieldException: Field 'aliases' is required` on **every** call, so they
  had never worked. And the four `alias/*/identity/{contextId}` methods are gone:
  that scope answers 404. `device` — the scope core actually serves — is added in
  its place (`createDeviceAlias` / `lookupDeviceAlias` / `deleteDeviceAlias` /
  `listDeviceAliases`).

- **`getCertificate()` returns `String?`.** A node with no TLS certificate answers
  `404 Certificate not found`, which is an absence, not a failure — it used to
  `ensureSuccessful()` and throw on any plain `merod init` node.

- **`Credentials.bootstrapSecret` removed**, with `MeroClient.login`'s third
  parameter and `LoginSheet`'s `showBootstrapSecret`. `merod init` creates the
  admin account, and rc.29 parses `bootstrap_secret` only to discard it.

### Added — surface from rc.23 to rc.32

`getNodeIdentity` (incl. rc.32's `holdsAccountRoot`) · `isReady` ·
`getApplicationAbi` · `performIntent` · `listMemberDevices` ·
`listAccountDevices` · `listAccountApplications` · `pairInit` · `pairComplete` ·
`relinkDevice` · `revokeAccountDevice` · `admitJoin`, and **`admitters` +
`admitterAddrs`** on both invite requests.

⚠️ `admitterAddrs` is silent if missed: rc.32 renamed it from `admitterHints`, and
the request is not `deny_unknown_fields`, so a node ignores the old key and
answers 200 — an unfixed client quietly mints invitations no joiner can dial,
just as core#3804 made the `admitters` those addresses point at an authorization
boundary rather than a hint.

⚠️ `listAccountDevices`, `listAccountApplications` and `listMemberDevices` answer
**flat** — `{devices:[…]}` / `{applications:[…]}` / `{members:[…]}`, no `data`
envelope — unlike every route around them.

### sample-app

- Chat installs **`com.calimero.chat`**: `com.calimero.curb` is no longer
  published (the registry answers `[]`). Same `init` params, but `send_message`
  has **no `sender_username`** argument — passing curb's panics the guest at
  argument deserialization. "Not published" is now reported as itself instead of
  leaving the UI on the install gate.
- SDK Explorer gains an **Accounts & devices** category and entries for every new
  method, so `check-registry-parity.sh` stays green.

### Docs

- **The SSE event kind is `StateMutation`, not `ExecutionEvent`.** The events guide
  showed a `when (event.kind)` branching on `"ExecutionEvent"`, which no node sends,
  so that branch never fired. A live rc.32 node sends `StateMutation` (the context
  state moved) and `SyncStatus`; the contract's own events sit under
  `data.events[]`, each with its own `kind`. Captured from the wire. The chat sample
  was unaffected because it reloads on any event rather than branching.

### Testing

Response bodies captured verbatim from the live node live in
`mero-core/src/test/resources/fixtures/`. A fixture written to match the model can
only confirm the model agrees with itself — which is how both this and the rc.25
`groupId`→`namespaceId` rename got past a green suite. 78 tests, and
`RealNodeE2ETest` now drives the new surface against a real node, and
**decode-sweeps every read that needs no id** — which is what caught the alias
map and the certificate 404 after the hand-picked checks had all passed.

`TwoNodeInviteJoinE2ETest` drives the whole invitation journey through the SDK's
own types across two real nodes: mint on A, carry as Kotlin objects, join on B.
Simulating the old model at both levels turns that join into an HTTP 500 and
reddens three unit tests, so it is load-bearing rather than decorative — checked,
not assumed. (The first attempt at that check filtered the wrong serializer and
passed; the envelope carries the nested object verbatim, so only filtering there
reproduces the bug.)

## Unreleased — swift-sdk parity pass

Brings the Android SDK and sample level with `calimero-network/swift-sdk` as of its
`syncGroupContexts` / clean-landing work.

### mero-core
- `AuthApi`: `createRootKey`, `deleteRootKey`, `generateClientKey`, `deleteClientKey`,
  `getKeyPermissions`, `updateKeyPermissions` (core's `{ add, remove }` delta, not a replacement),
  and `generateMockTokens` — with their wire types.
- `AdminApi.syncGroupContexts(groupId)`: sync the group, then join + state-pull each of its contexts,
  so a freshly joined group initializes instead of staying on the all-ones uninitialized hash.
  Clients no longer reimplement this (and hit the trap).

### sample-app
- **Landing screen**: Open Chat Example + Explore SDK entries (with method/category counts), replacing
  the flat op list; the SDK surface now lives behind a **searchable list of collapsible categories**
  (search auto-expands matches).
- **Diagnostics screen** for the session log (clear / copy), reachable from the landing and the login
  screen; login screen gets the brand mark and a spaced-out form.
- **Chat**: joined spaces are listed (no more filtering by our own app id, which hid invited spaces);
  uninitialized contexts are joined + synced on load; joining retries `syncGroupContexts` six times
  with visible progress and registers the display name per context; "Sync now" + a diagnostic empty
  state that distinguishes "curb not installed" from "0 peers" from "still syncing"; recursive
  invitations are accepted; blocking busy overlay; single-line invite field with paste; invite sheet
  with Copy/Share; deterministic avatar colors; auto-scroll to the newest message.
- Registry: 12 more operations (root/client keys + permissions, `generateTokens`, `generateMockTokens`,
  `removeGroupMembers`, `setContextMetadata`, `syncGroup`, `syncGroupContexts`) — 127 total.
- e2e hooks as launch extras: `chatUser` (chat display name) and `invite` (auto-join on open) —
  the Android analogs of the Swift harness's `E2E_USERNAME` / `E2E_JOIN`.

### Tests / CI
- `AdminSyncContextsTest` asserts the sync/join/state-pull call sequence.
- Merobox two-node checks ported from swift-sdk: `merobox-sync.yml` (kv_store forward + bidirectional,
  gating) and `merobox-chat-sync.yml` (real published curb app, informational), with `ci/merobox/`
  scenarios and the node-matched `kv_store.wasm`.
- CI additionally compiles the instrumented tests, validates the merobox scenarios, and runs
  `ci/check-registry-parity.sh` so no SDK method can be added without an SDK Explorer entry.
- ktlint is now **enforcing** (`ignoreFailures = false`) after a full `ktlintFormat` pass; detekt's
  `LargeClass` is off for the deliberately-large `AdminApi`. Actions bumped to Node-24 runtimes.

## 0.1.0 (unreleased)

Initial drop — M1 (transport + auth core) plus a Compose login UI kit. Ports the mero-js v7.0.1 wire
contract and mirrors `calimero-network/swift-sdk`.

### mero-core
- `Mero` top-level client: `authenticate`, `setTokenData`, `getTokenData`, `clearToken`, `logout`,
  `isAuthenticated`, plus `auth`/`rpc` sub-clients and static SSO helpers.
- OkHttp transport with Bearer injection, reactive `401 token_expired` → refresh via `Authenticator`,
  and terminal `x-auth-error` → `AuthRevokedException` + store clear.
- `RefreshCoordinator`: single-use-refresh-token safety (single-flight + cross-process `FileLock` +
  store re-read inside the lock).
- `AuthApi` (`generateTokens`, `refreshToken`, `validateToken`, providers/health/identity, key lists),
  `RpcClient` (`execute` with `executorPublicKey`, `migrateMyEntries`, `countMyPending`), `Capabilities`.
- `AdminApi` — full ~106-method port of the node's admin surface: contexts, context identities,
  groups & subgroups, members/capabilities, namespaces, invitations (single + recursive), registry
  install/versions, blobs, aliases, metadata, TEE, upgrades/migrations. Faithful 1:1 with the Swift SDK.
- `SseClient` + `Mero.events(contextIds)` — auto-reconnecting Server-Sent-Events stream as a `Flow`,
  subscribing on `connect` and reconnecting on drop/`close` (powers live chat updates without polling).
- `TokenStore` with `MemoryTokenStore` and Keystore-backed `EncryptedPrefsTokenStore`.
- SSO: `parseAuthCallback`, `buildAuthLoginUrl`, `SsoLauncher` (Chrome Custom Tabs).

### mero-compose
- `MeroClient` (observable auth `StateFlow`), `MeroProvider`/`useMero`, `LoginSheet`, `ConnectButton`.

### mero-testkit
- `FakeNode` — a stateful in-memory Calimero node on OkHttp MockWebServer (rotating/single-use tokens,
  expire/revoke controls, call counters, canned RPC outputs) for node-free tests and the app's mock mode.

### sample-app
- Mock mode (deterministic login → home → RPC → logout against an in-app `FakeNode`) driving the
  instrumented UI test, plus an **SDK Explorer + native chat client** (install curb from the registry,
  spaces/channels, send/read messages, invite & join, live SSE) against a real node, with hosted SSO.

### Tests / CI
- Unit + mock suites (`EndToEndMockTest`, `HttpAndAuthTest`, `AdminApiRequestTest`) on `FakeNode`;
  a live-node `RealNodeE2ETest` (self-skips without `MERO_E2E_NODE_URL`); instrumented UI + chat e2e.
- Workflows: `e2e.yml` (live merod), `instrumented.yml` (emulator UI test, PR gate),
  `android-e2e.yml` (single- & multi-node app e2e); dev/test scripts (`run-app.sh`, `android-e2e.sh`,
  `chat-multi-e2e.sh`, `test-all.sh`) and `TESTING.md`.

### Tooling
- Gradle 8.7, AGP 8.5, Kotlin 2.0; ktlint + detekt; GitHub Actions CI (lint, unit tests, lint, assemble).
