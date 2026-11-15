## [35.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v35.3.0...mero-js-v35.4.0) (2026-11-15)

### Features

* **client:** verify a pipeline run's receipt against the cluster's TDX attestation ([#390](https://github.com/calimero-network/mero-js/issues/390)) ([b77067c](https://github.com/calimero-network/mero-js/commit/b77067c56e3f679b13b0bb3e0052ce58b292709c))

## [35.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v35.2.1...mero-js-v35.3.0) (2026-11-15)

### Features

* **client:** expose what the client verified about its node ([#396](https://github.com/calimero-network/mero-js/issues/396)) ([14565de](https://github.com/calimero-network/mero-js/commit/14565dedb8efb2f9e99110b7f3d9b1c0143d33f5))

## [35.2.1](https://github.com/calimero-network/mero-js/compare/mero-js-v35.2.0...mero-js-v35.2.1) (2026-11-13)

### Bug Fixes

* **docs:** open the README with what Calimero is and what mero-js does ([#393](https://github.com/calimero-network/mero-js/issues/393)) ([ac42866](https://github.com/calimero-network/mero-js/commit/ac428664c80554a46c90d0ab3ccbf3bd804e3027))

## [35.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v35.1.0...mero-js-v35.2.0) (2026-11-13)

### Features

* **api:** sync with core f0b0a6b9d ([#387](https://github.com/calimero-network/mero-js/issues/387)) ([a0069d6](https://github.com/calimero-network/mero-js/commit/a0069d616d7880577e86d6982367cc19c51ee539))

### Bug Fixes

* **docs:** drop the removed account approvals and fix the cluster fixture path ([#388](https://github.com/calimero-network/mero-js/issues/388)) ([44a0838](https://github.com/calimero-network/mero-js/commit/44a083875a5b8c15756fdcfc17309139948dd0a5))

## [35.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v35.0.0...mero-js-v35.1.0) (2026-11-13)

### Features

* **packages:** build and sign a compute package with a publisher key ([#379](https://github.com/calimero-network/mero-js/issues/379)) ([6e341b0](https://github.com/calimero-network/mero-js/commit/6e341b0c0d0c025b8dc02aeb8e209ffd0f76191d))

## [35.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v34.1.0...mero-js-v35.0.0) (2026-11-12)

### ⚠ BREAKING CHANGES

* **api:** sync with core 3da5b6edb (#386)

### Features

* **api:** sync with core 3da5b6edb ([#386](https://github.com/calimero-network/mero-js/issues/386)) ([0567174](https://github.com/calimero-network/mero-js/commit/05671744b3fe924d8df417a3290bea59d97688fa))

## [34.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v34.0.0...mero-js-v34.1.0) (2026-11-12)

### Features

* **client:** type the organization management consent kinds ([#383](https://github.com/calimero-network/mero-js/issues/383)) ([c73326d](https://github.com/calimero-network/mero-js/commit/c73326dbffe4f81eba4c6783ea75fcc66925f8ff))

## [34.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v33.3.2...mero-js-v34.0.0) (2026-11-12)

### ⚠ BREAKING CHANGES

* **api:** core's node API changed in a way callers of the generated client see:
- `GET /consent/{request}/result`: added the new `acceptInvite` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `createInvite` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `createOrg` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `deleteOrg` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `leaveOrg` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `removeMemberTeams` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `removeMember` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `renameOrg` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `revokeInvite` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `setAllApps` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `setCapabilities` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `setMemberRole` enum value to the `kind` response property for the response status `200`
- `GET /consent/{request}/result`: added the new `setMemberTeams` enum value to the `kind` response property for the response status `200`

Co-authored-by: gh-actions-calimero[bot] <168463340+gh-actions-calimero[bot]@users.noreply.github.com>

### Features

* **api:** sync with core c350a92bc ([#375](https://github.com/calimero-network/mero-js/issues/375)) ([095dfa8](https://github.com/calimero-network/mero-js/commit/095dfa8552a6f3869bff75beae475b6c0834dbb3))

## [33.3.2](https://github.com/calimero-network/mero-js/compare/mero-js-v33.3.1...mero-js-v33.3.2) (2026-11-12)

### Bug Fixes

* **consent:** type each consent helper as the request kind it builds ([#380](https://github.com/calimero-network/mero-js/issues/380)) ([73d3d8c](https://github.com/calimero-network/mero-js/commit/73d3d8ce11841b36563cbc708ca9fee692b51d6e))

## [33.3.1](https://github.com/calimero-network/mero-js/compare/mero-js-v33.3.0...mero-js-v33.3.1) (2026-11-12)

### Bug Fixes

* **recovery:** check recovery files' attestation with the v3 receipt-key binding ([#378](https://github.com/calimero-network/mero-js/issues/378)) ([7b37528](https://github.com/calimero-network/mero-js/commit/7b37528211c268e4a462c70195aa5d47b669c479))

## [33.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v33.2.0...mero-js-v33.3.0) (2026-11-12)

### Features

* **client:** organization consent kinds and a checked consent return ([#377](https://github.com/calimero-network/mero-js/issues/377)) ([218f07d](https://github.com/calimero-network/mero-js/commit/218f07d5f1cc01f7517ed923da2ba094163db5f6))
* **files:** download a sealed file, checked against its id and hash ([#374](https://github.com/calimero-network/mero-js/issues/374)) ([87d018d](https://github.com/calimero-network/mero-js/commit/87d018db59f8bebfaa840571c857aa6dce44233e))

## [33.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v33.1.0...mero-js-v33.2.0) (2026-11-12)

### Features

* **files:** upload sealed files through the files API ([#349](https://github.com/calimero-network/mero-js/issues/349)) ([e375f6c](https://github.com/calimero-network/mero-js/commit/e375f6c458e815f81b5658005341e1a5ceea88e8))

## [33.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v33.0.0...mero-js-v33.1.0) (2026-11-12)

### Features

* **recovery:** open the recovery disk's files on the holder's device ([#376](https://github.com/calimero-network/mero-js/issues/376)) ([5a39035](https://github.com/calimero-network/mero-js/commit/5a39035915b20de5b8d46311343c1083848af5f4))

## [33.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v32.0.0...mero-js-v33.0.0) (2026-11-12)

### ⚠ BREAKING CHANGES

* **api:** core's node API changed in a way callers of the generated client see:
- removed the schema `Written`
- `GET /apps/{app}/attestation`: deleted the `query` request parameter `appHash`
- `GET /apps/{app}/attestation`: deleted the `query` request parameter `app`
- `GET /apps/{app}/attestation`: deleted the `query` request parameter `purpose`
- `GET /ops/status`: added `#/components/schemas/RollbackStatus, subschema #2` to the `rollback` response property `anyOf` list for the response status `200`
- `POST /apps/{app}/transfer`: the response's body became nullable
- `POST /apps/{app}/transfer`: the response's body `type` changed from `object` to `null` for status `200`
- `POST /ops/apps/{app}/restore`: the `status` response's property `type` changed from `string` to `object` for status `200`
- `POST /orgs/{org}/publishers`: removed the required property `approval` from the response with the `200` status
- `POST /orgs/{org}/publishers`: removed the required property `signer` from the response with the `200` status

* test(routes): follow core 92ac297a1's app attestation query and publisher answer

`GET /apps/{app}/attestation` documents only `nonce` now (the node never
read `app`, `appHash` or `purpose` there), so the generated method flattens
it beside the path's `app`. The method test that used it to exercise a query
named like a path parameter now does so through `route()` with a synthetic
shape, and a new case holds the flattened call. The publisher binding's fake
answers `{ generation }`, as the node does, and the client reference drops the
removed parameters.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01RYxobRQaXBRFPdkTkQmgj8

### Features

* **api:** sync with core 92ac297a1 ([#372](https://github.com/calimero-network/mero-js/issues/372)) ([e0c4aaa](https://github.com/calimero-network/mero-js/commit/e0c4aaadd330a48fa7edad800769ecf7df76c79d))

## [32.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v31.0.3...mero-js-v32.0.0) (2026-11-12)

### ⚠ BREAKING CHANGES

* **api:** core's node API changed in a way callers of the generated client see:
- `DELETE /ops/nodes/{id}`: the `status` response's property `type` changed from `string` to `object` for status `200`

Co-authored-by: gh-actions-calimero[bot] <168463340+gh-actions-calimero[bot]@users.noreply.github.com>

### Features

* **api:** sync with core fb0ed2e0c ([#365](https://github.com/calimero-network/mero-js/issues/365)) ([a2d1372](https://github.com/calimero-network/mero-js/commit/a2d1372c0323dcef3062d5ae8e1d830a624073ae))

## [31.0.3](https://github.com/calimero-network/mero-js/compare/mero-js-v31.0.2...mero-js-v31.0.3) (2026-11-12)

### Bug Fixes

* **release:** only feat, fix and perf cut a release ([#369](https://github.com/calimero-network/mero-js/issues/369)) ([9e57a45](https://github.com/calimero-network/mero-js/commit/9e57a4533211217f4d68027d09455efcae59466e))

## [31.0.2](https://github.com/calimero-network/mero-js/compare/mero-js-v31.0.1...mero-js-v31.0.2) (2026-11-12)

## [31.0.1](https://github.com/calimero-network/mero-js/compare/mero-js-v31.0.0...mero-js-v31.0.1) (2026-11-11)

## [31.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v30.1.0...mero-js-v31.0.0) (2026-11-11)

### ⚠ BREAKING CHANGES

* lay the client out as runtime, routes and client, and drop the cloud-v2 leftovers (#362)

### Code Refactoring

* lay the client out as runtime, routes and client, and drop the cloud-v2 leftovers ([#362](https://github.com/calimero-network/mero-js/issues/362)) ([899552f](https://github.com/calimero-network/mero-js/commit/899552f0aa75bc660ee72c82d8bf7cab7fa6a322))

## [30.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v30.0.0...mero-js-v30.1.0) (2026-11-11)

### Features

* **cloud-v2:** type 409 FEATURE_NOT_ACTIVE as FeatureNotActive ([#354](https://github.com/calimero-network/mero-js/issues/354)) ([90fa7b8](https://github.com/calimero-network/mero-js/commit/90fa7b84e5ab5dc008728e5a9c121ab619b93383))

## [30.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.17.0...mero-js-v30.0.0) (2026-11-11)

### ⚠ BREAKING CHANGES

* **client:** remove the deprecated route wrappers (30.0) (#359)

### Features

* **client:** remove the deprecated route wrappers (30.0) ([#359](https://github.com/calimero-network/mero-js/issues/359)) ([ad2a152](https://github.com/calimero-network/mero-js/commit/ad2a152cffeb760d5b6ea821b5b1a8ec634df16a))

## [29.17.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.16.0...mero-js-v29.17.0) (2026-11-11)

### Features

* **api:** a client generated from core's OpenAPI document, over the sealed transport ([#358](https://github.com/calimero-network/mero-js/issues/358)) ([3d949f6](https://github.com/calimero-network/mero-js/commit/3d949f679d79c9bdc5e6f06fac2cabf3a20384b6)), closes [#4937](https://github.com/calimero-network/mero-js/issues/4937) [#127](https://github.com/calimero-network/mero-js/issues/127) [#4937](https://github.com/calimero-network/mero-js/issues/4937)

## [29.16.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.15.0...mero-js-v29.16.0) (2026-11-11)

### Features

* **runtime:** split the transport runtime from the route methods ([#356](https://github.com/calimero-network/mero-js/issues/356)) ([ef12441](https://github.com/calimero-network/mero-js/commit/ef12441d16201670edde7f5ca82908c330590c37))

## [29.15.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.14.0...mero-js-v29.15.0) (2026-11-11)

### Features

* **cloud-v2:** offer the newest release an app can reach, and type RUNTIME_TOO_OLD ([#355](https://github.com/calimero-network/mero-js/issues/355)) ([57bd0d0](https://github.com/calimero-network/mero-js/commit/57bd0d0e7912450eba67ad1c1288da9940e71f87))

## [29.14.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.13.0...mero-js-v29.14.0) (2026-11-10)

### Features

* **cloud-v2:** setAllApps and invite allApps, every app added later ([#351](https://github.com/calimero-network/mero-js/issues/351)) ([18d5979](https://github.com/calimero-network/mero-js/commit/18d59791a52a05e156437498e224304c74dc1e6b))

## [29.13.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.12.0...mero-js-v29.13.0) (2026-11-10)

### Features

* **cloud-v2:** moveApp, an app instance to another organization ([#348](https://github.com/calimero-network/mero-js/issues/348)) ([cfe08f6](https://github.com/calimero-network/mero-js/commit/cfe08f63a91f0aff6ba9b82edc8b7a4dbb753a2e))

## [29.12.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.11.0...mero-js-v29.12.0) (2026-11-10)

### Features

* **orgs:** orgMembersSecurity(), members' lockout risk for an org's admins ([#346](https://github.com/calimero-network/mero-js/issues/346)) ([3c92890](https://github.com/calimero-network/mero-js/commit/3c9289039ddf47a93310684872f1c95710c32b29))

## [29.11.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.10.0...mero-js-v29.11.0) (2026-11-10)

### Features

* **auth:** listActivity(), what the account's passkeys signed in to and approved ([#345](https://github.com/calimero-network/mero-js/issues/345)) ([e836a92](https://github.com/calimero-network/mero-js/commit/e836a9247392e1240f33c0063175dbe2e9d1817b))

## [29.10.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.9.0...mero-js-v29.10.0) (2026-11-10)

### Features

* **cloud-v2:** scopeApps closes or opens one app team, keeping whom it names ([#347](https://github.com/calimero-network/mero-js/issues/347)) ([ff119e2](https://github.com/calimero-network/mero-js/commit/ff119e2cf3a817329fc9c6ad74adb02d821a68a0))

## [29.9.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.8.1...mero-js-v29.9.0) (2026-11-10)

### Features

* **auth:** list and end the account's sessions, and type /auth/me's passkeys ([#344](https://github.com/calimero-network/mero-js/issues/344)) ([373213a](https://github.com/calimero-network/mero-js/commit/373213a285b189cc4ac912c5407d9115965517f1))

## [29.8.1](https://github.com/calimero-network/mero-js/compare/mero-js-v29.8.0...mero-js-v29.8.1) (2026-11-10)

### Bug Fixes

* **orgs:** delete a team holding apps with the deleter's passkey ([#343](https://github.com/calimero-network/mero-js/issues/343)) ([b14f052](https://github.com/calimero-network/mero-js/commit/b14f05273bbbd4443d6b5b94d312b24bfb78d250))

## [29.8.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.7...mero-js-v29.8.0) (2026-11-10)

### Features

* **cloud-v2:** wrap core's org app-teams APIs ([#342](https://github.com/calimero-network/mero-js/issues/342)) ([929ebe6](https://github.com/calimero-network/mero-js/commit/929ebe63a3f1bce58b454e703c2193f735afb7b8)), closes [#4890](https://github.com/calimero-network/mero-js/issues/4890)

## [29.7.7](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.6...mero-js-v29.7.7) (2026-11-09)

### Bug Fixes

* **passkey:** sign out locally when logout fails; let a signal cancel a passkey prompt ([#341](https://github.com/calimero-network/mero-js/issues/341)) ([363f371](https://github.com/calimero-network/mero-js/commit/363f37156658d46d35fa499865b4a45cbba9c4df))

## [29.7.6](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.5...mero-js-v29.7.6) (2026-11-09)

### Bug Fixes

* **passkey:** ask the sign-up confirmation for user verification 'preferred', not 'required' ([#340](https://github.com/calimero-network/mero-js/issues/340)) ([1eecdb0](https://github.com/calimero-network/mero-js/commit/1eecdb0121e3efbafeb0c6e7bbe52b63aa2fb0ec))

## [29.7.5](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.4...mero-js-v29.7.5) (2026-11-09)

### Bug Fixes

* **passkey:** always use the browser's own passkey prompt, and create ES256 passkeys ([#339](https://github.com/calimero-network/mero-js/issues/339)) ([23ac97e](https://github.com/calimero-network/mero-js/commit/23ac97e844dd7b24dbed47093f92ba96dc3591cb))

## [29.7.4](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.3...mero-js-v29.7.4) (2026-11-09)

### Bug Fixes

* **passkey:** require user verification on confirm prompts; tell a blocked prompt from a cancel ([#338](https://github.com/calimero-network/mero-js/issues/338)) ([e56d135](https://github.com/calimero-network/mero-js/commit/e56d1355da6e28c4ae16aa92be46c5287a541e02))

## [29.7.3](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.2...mero-js-v29.7.3) (2026-11-09)

### Bug Fixes

* **passkey:** ask the browser directly when an extension refuses a prompt as already pending ([#337](https://github.com/calimero-network/mero-js/issues/337)) ([51513fd](https://github.com/calimero-network/mero-js/commit/51513fdff177518d73ce55e0d634ae5363be123f))

## [29.7.2](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.1...mero-js-v29.7.2) (2026-11-09)

### Bug Fixes

* **fleet:** expand workspace member globs in the consumer, whatever the current directory ([#336](https://github.com/calimero-network/mero-js/issues/336)) ([fdf02ba](https://github.com/calimero-network/mero-js/commit/fdf02bace8a4949e1e94775200304f612cf1f311))

## [29.7.1](https://github.com/calimero-network/mero-js/compare/mero-js-v29.7.0...mero-js-v29.7.1) (2026-11-09)

### Bug Fixes

* **fleet:** judge each pnpm catalog on its own version line ([#335](https://github.com/calimero-network/mero-js/issues/335)) ([2ba3961](https://github.com/calimero-network/mero-js/commit/2ba3961baf358e0f7bd6faa404dfc3a9e80bdeb3)), closes [apps#765](https://github.com/calimero-network/apps/issues/765)

## [29.7.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.6.0...mero-js-v29.7.0) (2026-11-06)

### Features

* **cloud-v2:** type a passkey from another cluster as PasskeyUnknownHere ([#334](https://github.com/calimero-network/mero-js/issues/334)) ([8292763](https://github.com/calimero-network/mero-js/commit/82927638909ce158179bceed1921edc827fc47cc))

## [29.6.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.5.0...mero-js-v29.6.0) (2026-11-05)

### Features

* **cloud-v2:** register served origins (custom domains) ([#333](https://github.com/calimero-network/mero-js/issues/333)) ([e840c0a](https://github.com/calimero-network/mero-js/commit/e840c0a909b78edc6076dc61ac5eed050f2f1b17))

## [29.5.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.4.0...mero-js-v29.5.0) (2026-11-05)

### Features

* **cloud-v2:** account recovery with a recovery code ([#330](https://github.com/calimero-network/mero-js/issues/330)) ([afa158d](https://github.com/calimero-network/mero-js/commit/afa158d8c6841df47c7019519200ec376b1239ec))

## [29.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.3.0...mero-js-v29.4.0) (2026-11-05)

### Features

* **cloud-v2:** recover an organization with its recovery seed ([#331](https://github.com/calimero-network/mero-js/issues/331)) ([fd87ae7](https://github.com/calimero-network/mero-js/commit/fd87ae7974686d352269a2998fea1f1b7030971d))

## [29.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.2.0...mero-js-v29.3.0) (2026-11-05)

### Features

* **cloud-v2:** sign up with an invite and read the sign-up policy ([#329](https://github.com/calimero-network/mero-js/issues/329)) ([f7dbf7f](https://github.com/calimero-network/mero-js/commit/f7dbf7f681483fad95805bb2612e47b2d176ea4e))

## [29.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.1.0...mero-js-v29.2.0) (2026-11-04)

### Features

* **cloud-v2:** create upgrade keys for ci deploys ([#328](https://github.com/calimero-network/mero-js/issues/328)) ([4ef759d](https://github.com/calimero-network/mero-js/commit/4ef759db683b5600eeb03fc6d187877ee8971993))

## [29.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v29.0.0...mero-js-v29.1.0) (2026-11-04)

### Features

* **cloud-v2:** package origins and origin-aware passkey sign-in ([#326](https://github.com/calimero-network/mero-js/issues/326)) ([4ebf6b6](https://github.com/calimero-network/mero-js/commit/4ebf6b6ba004e6aa330b05ca4e3b7eef224cb1ec))

## [29.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v28.1.2...mero-js-v29.0.0) (2026-11-04)

### ⚠ BREAKING CHANGES

* **cloud-v2:** answer the sign-up's and the key revocation's passkey proofs (#327)

### Features

* **cloud-v2:** answer the sign-up's and the key revocation's passkey proofs ([#327](https://github.com/calimero-network/mero-js/issues/327)) ([1381479](https://github.com/calimero-network/mero-js/commit/1381479c6e3dd2daffe719110da9d1ff3da7542e))

## [28.1.2](https://github.com/calimero-network/mero-js/compare/mero-js-v28.1.1...mero-js-v28.1.2) (2026-11-02)

### Bug Fixes

* **deps:** patch critical and high security alerts ([#325](https://github.com/calimero-network/mero-js/issues/325)) ([9391960](https://github.com/calimero-network/mero-js/commit/93919601cc9d89766aad911f88620f61f2397c66))

## [28.1.1](https://github.com/calimero-network/mero-js/compare/mero-js-v28.1.0...mero-js-v28.1.1) (2026-11-02)

## [28.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v28.0.0...mero-js-v28.1.0) (2026-11-02)

### Features

* **cloud-v2:** blob content types and grants, xcalls, deployments and deployers (core rc.7+) ([#323](https://github.com/calimero-network/mero-js/issues/323)) ([c35fbe3](https://github.com/calimero-network/mero-js/commit/c35fbe31489e13cd61bc6562821b92c827eded8c)), closes [#4815](https://github.com/calimero-network/mero-js/issues/4815) [4817/#4820](https://github.com/4817/mero-js/issues/4820) [#4810](https://github.com/calimero-network/mero-js/issues/4810) [#4807](https://github.com/calimero-network/mero-js/issues/4807)

## [28.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v27.5.0...mero-js-v28.0.0) (2026-10-31)

### ⚠ BREAKING CHANGES

* **cloud-v2:** answer passkey step-ups on credentials, memberships and invites (#320)

### Features

* **cloud-v2:** answer passkey step-ups on credentials, memberships and invites ([#320](https://github.com/calimero-network/mero-js/issues/320)) ([1953b55](https://github.com/calimero-network/mero-js/commit/1953b5573b1d97be89bac0c99ae3ab65fe4961ac))

## [27.5.0](https://github.com/calimero-network/mero-js/compare/mero-js-v27.4.0...mero-js-v27.5.0) (2026-10-31)

### Features

* **cloud-v2:** connect to a cluster by name and fail over between its nodes ([#322](https://github.com/calimero-network/mero-js/issues/322)) ([400e893](https://github.com/calimero-network/mero-js/commit/400e893cb818c9570f3c5330e8ce313cef7860f8))

## [27.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v27.3.0...mero-js-v27.4.0) (2026-10-30)

### Features

* **cloud-v2:** take Intel's collateral from the cloud's mirror, which browsers can read ([#321](https://github.com/calimero-network/mero-js/issues/321)) ([431a4e8](https://github.com/calimero-network/mero-js/commit/431a4e86926deb36471dc2761274d07e1dfea7fe))

## [27.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v27.2.0...mero-js-v27.3.0) (2026-10-30)

### Features

* **approvals:** expose the consent page's confirmation code ([#319](https://github.com/calimero-network/mero-js/issues/319)) ([0735ec5](https://github.com/calimero-network/mero-js/commit/0735ec509d56cd1481d2191365c1edfe3841b5ba))

## [27.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v27.1.0...mero-js-v27.2.0) (2026-10-30)

### Features

* **cloud-v2:** refresh after losing the issuing node and move on 421 WRONG_NODE ([#318](https://github.com/calimero-network/mero-js/issues/318)) ([03993b9](https://github.com/calimero-network/mero-js/commit/03993b923774a7f23e3e7c740e8c9c4d4a1b50bf))

## [27.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v27.0.2...mero-js-v27.1.0) (2026-10-30)

### Features

* **approvals:** approval reports, publishers, deadlines and the node's consent page ([#317](https://github.com/calimero-network/mero-js/issues/317)) ([eebdd2a](https://github.com/calimero-network/mero-js/commit/eebdd2af585c66cca7e135b69c570013c3f05483))

## [27.0.2](https://github.com/calimero-network/mero-js/compare/mero-js-v27.0.1...mero-js-v27.0.2) (2026-10-29)

## [27.0.1](https://github.com/calimero-network/mero-js/compare/mero-js-v27.0.0...mero-js-v27.0.1) (2026-10-29)

## [27.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.11.0...mero-js-v27.0.0) (2026-10-28)

### ⚠ BREAKING CHANGES

* ship only the calimero 1.0 client (#314)

### Features

* ship only the calimero 1.0 client ([#314](https://github.com/calimero-network/mero-js/issues/314)) ([aad31bc](https://github.com/calimero-network/mero-js/commit/aad31bc69ed250b037fe9a8e9fea1f574cbe4924))

### Calimero 1.0 (27.0.0)

The release that supports Calimero 1.0 (core `1.0.0-rc.1` and later). semantic-release writes the version heading and the commit list above this note when it publishes; this note is the migration summary.

* **The package is the Calimero 1.0 client only.** `CalimeroClient`, its errors and its types are the package's main exports. Code that talks to a v1 (merod 0.x) node stays on 26.x; npm does not move a `^26` range to 27.
* **Removed (v1):** `MeroJs` and `createMeroJs`; `AdminApiClient` and `AuthApiClient` (mero-auth) with their types; the HTTP client (`createBrowserHttpClient`, `createNodeHttpClient`, `withRetry`, …); `RpcClient`, `SseClient`, `WsClient`, `EphemeralClient` and their event types; `CloudClient` (namespaces, relays, HA); account roots, device certificates and scopes, group invitations, namespace and owner ops, login statements and signers; token stores and auth-callback helpers; node discovery; v1's `CAPABILITIES` helpers and error classifier (`HTTPError`, `AuthRevokedError`, `classifyError`); and the v1 attestation helpers of the sealed transport (`fetchAttestedTransportKey`, `createAttestedSealedFetch`, `createSignedReleaseSealedFetch`, `fetchNodeReleaseVersion`, `nodeReleaseVersionFromImage`).
* **Added to `CalimeroClient`:** `upgradeOrg`, `upgradeRun`, `retryUpgradeRun`, `approveOutbound`, `approveRelease`, `transferApp`, `setAppMetadata`, `setOrgMetadata`, `setMemberMetadata`, `labelPasskey`, `usage`, `alias`, `appStorage`, `appAbi`, `attestation`, `listBlobs`, `blobInfo`, `deleteBlob`, `publishPresence`, `presence`, `presenceStream`; `ORG_CAPABILITIES` and `orgCapabilities` with core 1.0's names.
* **Kept:** the sealed transport, quote and signed-release verification, and the private agent gate.
* **CI** runs the cluster suite against the newest released merod 1.0.

See [Migrating from v1](https://calimero-network.github.io/mero-js/reference/migrating-from-v1/).

## [26.11.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.10.0...mero-js-v26.11.0) (2026-10-27)

### Features

* **admin-api:** expose missingReleases from group upgrade status ([#309](https://github.com/calimero-network/mero-js/issues/309)) ([55bc59d](https://github.com/calimero-network/mero-js/commit/55bc59d432a93cf8f85ae00a75720f49b6411806))

## [26.10.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.9.1...mero-js-v26.10.0) (2026-10-26)

### Features

* **admin:** approve a publisher's outbound requests per namespace ([#307](https://github.com/calimero-network/mero-js/issues/307)) ([bf2d55c](https://github.com/calimero-network/mero-js/commit/bf2d55c45bcf42b98b2bd4436045e26665722721))

## [26.9.1](https://github.com/calimero-network/mero-js/compare/mero-js-v26.9.0...mero-js-v26.9.1) (2026-10-24)

### Bug Fixes

* **namespace-op:** sign at schema 27 ([#292](https://github.com/calimero-network/mero-js/issues/292)) ([1536a0a](https://github.com/calimero-network/mero-js/commit/1536a0afaab3b16cf310a54407417732e7f3ac88))

## [26.9.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.8.0...mero-js-v26.9.0) (2026-10-23)

### Features

* **admin:** access grant client methods and e2e ([#303](https://github.com/calimero-network/mero-js/issues/303)) ([3c4eeca](https://github.com/calimero-network/mero-js/commit/3c4eecaefc3f5c4a208e8f7cd6cc82eeda9d5ba4))

## [26.8.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.7.0...mero-js-v26.8.0) (2026-10-21)

### Features

* **agent-gate:** verify a release's signed inference allowlist ([#302](https://github.com/calimero-network/mero-js/issues/302)) ([a1c4691](https://github.com/calimero-network/mero-js/commit/a1c4691631f02ba5185c2734a2dc5ae95847101b))

## [26.7.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.6.0...mero-js-v26.7.0) (2026-10-21)

### Features

* **admin:** leave a namespace with purge, restore a forgotten one ([#295](https://github.com/calimero-network/mero-js/issues/295)) ([c382ad1](https://github.com/calimero-network/mero-js/commit/c382ad1032494d18400c4af8ddfe69fe5911910d))

## [26.6.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.5.0...mero-js-v26.6.0) (2026-10-21)

### Features

* **account:** surface relay login failures as RelayLoginError ([#300](https://github.com/calimero-network/mero-js/issues/300)) ([7245bec](https://github.com/calimero-network/mero-js/commit/7245bec74aad00eeca045dad22067a18a59b992e))

## [26.5.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.4.1...mero-js-v26.5.0) (2026-10-21)

### Features

* **account:** listing an account across relays says when none answered, and why ([#299](https://github.com/calimero-network/mero-js/issues/299)) ([066b43d](https://github.com/calimero-network/mero-js/commit/066b43da6293398e2311289b4da13bc7ce85c94f))

## [26.4.1](https://github.com/calimero-network/mero-js/compare/mero-js-v26.4.0...mero-js-v26.4.1) (2026-10-21)

## [26.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.3.0...mero-js-v26.4.0) (2026-10-20)

### Features

* **admin-api:** activate a namespace's upgrade on this node ([#293](https://github.com/calimero-network/mero-js/issues/293)) ([d31a8ee](https://github.com/calimero-network/mero-js/commit/d31a8ee1b4d2634c5644ca72b842da56419d7d60))

## [26.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.2.2...mero-js-v26.3.0) (2026-10-20)

### Features

* **admin:** type the upgrade trial's skipTrial and its context counts ([#291](https://github.com/calimero-network/mero-js/issues/291)) ([644434c](https://github.com/calimero-network/mero-js/commit/644434ce79bf0d3ba5fc941b9e3e78249e40291d))

## [26.2.2](https://github.com/calimero-network/mero-js/compare/mero-js-v26.2.1...mero-js-v26.2.2) (2026-10-19)

### Bug Fixes

* **cloud:** bind link grants to the browser that began the link ([#284](https://github.com/calimero-network/mero-js/issues/284)) ([5b25dd4](https://github.com/calimero-network/mero-js/commit/5b25dd410fc3c0d5c5232dfb3023761f1dbc6140))

## [26.2.1](https://github.com/calimero-network/mero-js/compare/mero-js-v26.2.0...mero-js-v26.2.1) (2026-10-19)

### Bug Fixes

* **namespace-op:** sign at namespace op schema version 26 ([#290](https://github.com/calimero-network/mero-js/issues/290)) ([b6be1fe](https://github.com/calimero-network/mero-js/commit/b6be1fe905a45b31f6dc08f77199831994a8b2db))

## [26.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.1.0...mero-js-v26.2.0) (2026-10-19)

### Features

* **admin-api:** type the target release package on namespaces and groups ([#285](https://github.com/calimero-network/mero-js/issues/285)) ([4df00f2](https://github.com/calimero-network/mero-js/commit/4df00f2c2a22906a2ad45b97731a86292eeeeba8))

## [26.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v26.0.1...mero-js-v26.1.0) (2026-10-18)

### Features

* **invitations:** tell an already-used link and a removal apart from other refusals ([#287](https://github.com/calimero-network/mero-js/issues/287)) ([c4562ea](https://github.com/calimero-network/mero-js/commit/c4562ea257f31a93a3734f8a21323852081d3333))

## [26.0.1](https://github.com/calimero-network/mero-js/compare/mero-js-v26.0.0...mero-js-v26.0.1) (2026-10-18)

## [26.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v25.3.3...mero-js-v26.0.0) (2026-10-17)

### ⚠ BREAKING CHANGES

* **admin-api:** drop calls to routes core does not serve (#286)

### Code Refactoring

* **admin-api:** drop calls to routes core does not serve ([#286](https://github.com/calimero-network/mero-js/issues/286)) ([65f188c](https://github.com/calimero-network/mero-js/commit/65f188c3053dad999a3824cd82978125f5b8335b))

## [25.3.3](https://github.com/calimero-network/mero-js/compare/mero-js-v25.3.2...mero-js-v25.3.3) (2026-10-16)

### Bug Fixes

* **sse:** a superseded stream's abort no longer tears down the live one ([#283](https://github.com/calimero-network/mero-js/issues/283)) ([f2ef5ae](https://github.com/calimero-network/mero-js/commit/f2ef5ae515871739d9379b2c35fc0650b9fea72e))

## [25.3.2](https://github.com/calimero-network/mero-js/compare/mero-js-v25.3.1...mero-js-v25.3.2) (2026-10-15)

### Bug Fixes

* **auth:** match login and enrolment callbacks to a request this browser began ([#271](https://github.com/calimero-network/mero-js/issues/271)) ([07951f5](https://github.com/calimero-network/mero-js/commit/07951f51df451bb97160dfaeb24dbbe2ad07774d))

## [25.3.1](https://github.com/calimero-network/mero-js/compare/mero-js-v25.3.0...mero-js-v25.3.1) (2026-10-15)

### Bug Fixes

* **http-client:** do not follow redirects on requests carrying a proof ([#268](https://github.com/calimero-network/mero-js/issues/268)) ([c5f57fe](https://github.com/calimero-network/mero-js/commit/c5f57fed2ad571de32c9aa94bbc5cd51569d913c))
* **request:** sign the path and query in request proofs ([#262](https://github.com/calimero-network/mero-js/issues/262)) ([f323b65](https://github.com/calimero-network/mero-js/commit/f323b65822b5e2af08e4aeff0eadc47f3038752f))

## [25.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v25.2.0...mero-js-v25.3.0) (2026-10-15)

### Features

* **client:** add listNamespaceRevokedDevices ([#281](https://github.com/calimero-network/mero-js/issues/281)) ([aef5d7f](https://github.com/calimero-network/mero-js/commit/aef5d7fd165080cd3dd579ba3337d1919b25c3fa))

## [25.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v25.1.1...mero-js-v25.2.0) (2026-10-14)

### Features

* **auth:** type trusted_callback_origins on the providers response ([#277](https://github.com/calimero-network/mero-js/issues/277)) ([412f8cf](https://github.com/calimero-network/mero-js/commit/412f8cf202f4505e2c2e044da83ef2d25de285bb))

## [25.1.1](https://github.com/calimero-network/mero-js/compare/mero-js-v25.1.0...mero-js-v25.1.1) (2026-10-14)

### Bug Fixes

* **namespace-op:** sign at namespace op schema version 25 ([#261](https://github.com/calimero-network/mero-js/issues/261)) ([f3bdfba](https://github.com/calimero-network/mero-js/commit/f3bdfba6f7af973f46fc5f8d163c8b3d952022f2))

## [25.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.7...mero-js-v25.1.0) (2026-10-14)

### Features

* **agent-gate:** attest a private agent, claim it, and seal its secrets to its TD ([#276](https://github.com/calimero-network/mero-js/issues/276)) ([830498d](https://github.com/calimero-network/mero-js/commit/830498d71509b6f586e720115e628b57e247e01f))

## [25.0.7](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.6...mero-js-v25.0.7) (2026-10-14)

### Bug Fixes

* **token-store:** bind persisted tokens to the node that issued them ([#269](https://github.com/calimero-network/mero-js/issues/269)) ([b005b43](https://github.com/calimero-network/mero-js/commit/b005b43ec7db6e9d62c0a4bbbef841bea0a2a547))

## [25.0.6](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.5...mero-js-v25.0.6) (2026-10-14)

## [25.0.5](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.4...mero-js-v25.0.5) (2026-10-13)

### Bug Fixes

* encode ids in admin, auth and relay request paths ([#267](https://github.com/calimero-network/mero-js/issues/267)) ([c53443f](https://github.com/calimero-network/mero-js/commit/c53443f2d7f45c5016794d9763af2fe3db6f28ae))

## [25.0.4](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.3...mero-js-v25.0.4) (2026-10-13)

### Bug Fixes

* **cloud:** match a link callback to the link this tab began ([#263](https://github.com/calimero-network/mero-js/issues/263)) ([a29be8c](https://github.com/calimero-network/mero-js/commit/a29be8c1c699685429454629fe8b17038a0c6f81))

## [25.0.3](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.2...mero-js-v25.0.3) (2026-10-13)

### Bug Fixes

* **account:** only trust relays the cloud lists ([#264](https://github.com/calimero-network/mero-js/issues/264)) ([98d6e6e](https://github.com/calimero-network/mero-js/commit/98d6e6ec788f58ef989384d81d1cf86a0bc1f3b9))

## [25.0.2](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.1...mero-js-v25.0.2) (2026-10-13)

### Bug Fixes

* **auth:** keep a logout from another tab signed out ([#270](https://github.com/calimero-network/mero-js/issues/270)) ([b4651b1](https://github.com/calimero-network/mero-js/commit/b4651b1420cf4c5b3e30bd55c369780e525d67c5))

## [25.0.1](https://github.com/calimero-network/mero-js/compare/mero-js-v25.0.0...mero-js-v25.0.1) (2026-10-13)

### Bug Fixes

* **groups:** let createGroup name a subgroup's visibility; restrict the invitation-join test's subgroup ([#266](https://github.com/calimero-network/mero-js/issues/266)) ([683a770](https://github.com/calimero-network/mero-js/commit/683a770fdd1067328a90ecd290a68184e11bee63))

## [25.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.7.0...mero-js-v25.0.0) (2026-10-13)

### ⚠ BREAKING CHANGES

* **sealed:** refuse debug TDs and trust only the production image profile by default (#265)

### Features

* **sealed:** refuse debug TDs and trust only the production image profile by default ([#265](https://github.com/calimero-network/mero-js/issues/265)) ([a94c1a8](https://github.com/calimero-network/mero-js/commit/a94c1a8e56a83c66dee3fb58cd60616f6bc34d36))

## [24.7.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.6.0...mero-js-v24.7.0) (2026-10-13)

### Features

* **account:** seal a delegated session's relay traffic to the attested TD ([#259](https://github.com/calimero-network/mero-js/issues/259)) ([891fbc4](https://github.com/calimero-network/mero-js/commit/891fbc49b6e7cfc7d70608722951bab1da06b55e))

## [24.6.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.5.0...mero-js-v24.6.0) (2026-10-10)

### Features

* **token-store:** let a token store lock refreshes across processes ([#260](https://github.com/calimero-network/mero-js/issues/260)) ([979bee1](https://github.com/calimero-network/mero-js/commit/979bee1dcb3da9b75f9c7828a272b85a37988d88))

## [24.5.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.4.1...mero-js-v24.5.0) (2026-10-07)

### Features

* **namespace-op:** sign at schema 24 (only a subgroup's creator may open it) ([#257](https://github.com/calimero-network/mero-js/issues/257)) ([76d03a3](https://github.com/calimero-network/mero-js/commit/76d03a3f75e5ca0c0ec01059906368ad96db8517))

## [24.4.1](https://github.com/calimero-network/mero-js/compare/mero-js-v24.4.0...mero-js-v24.4.1) (2026-10-07)

### Bug Fixes

* **device-cert:** refuse a device id minted for another account ([#254](https://github.com/calimero-network/mero-js/issues/254)) ([bcb6520](https://github.com/calimero-network/mero-js/commit/bcb65206f70702aa2fc06f04b31dd4c612b38927))

## [24.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.3.0...mero-js-v24.4.0) (2026-10-07)

### Features

* **admin:** heldOps on a namespace ([#256](https://github.com/calimero-network/mero-js/issues/256)) ([607d4a1](https://github.com/calimero-network/mero-js/commit/607d4a103ed52ee4e8189e47247a917d3bb6e3bb))

## [24.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.2.0...mero-js-v24.3.0) (2026-10-06)

### Features

* **warrant:** pin each data warrant to the release it is signed against ([#255](https://github.com/calimero-network/mero-js/issues/255)) ([b2abb95](https://github.com/calimero-network/mero-js/commit/b2abb955abaf52265a9b65a220cfc3fff79defc2))

## [24.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.1.3...mero-js-v24.2.0) (2026-10-06)

### Features

* **warrant:** name and sign the executor device key in every warrant ([#250](https://github.com/calimero-network/mero-js/issues/250)) ([5a8b742](https://github.com/calimero-network/mero-js/commit/5a8b742f8ca37ae326e4e97fcd5ba735b523debc))

## [24.1.3](https://github.com/calimero-network/mero-js/compare/mero-js-v24.1.2...mero-js-v24.1.3) (2026-10-06)

### Bug Fixes

* **namespace-op:** sign namespace ops at schema version 23 ([#220](https://github.com/calimero-network/mero-js/issues/220)) ([efcc79d](https://github.com/calimero-network/mero-js/commit/efcc79d7e77108ccfe918228d0ae54ff8e536298))

## [24.1.2](https://github.com/calimero-network/mero-js/compare/mero-js-v24.1.1...mero-js-v24.1.2) (2026-10-06)

## [24.1.1](https://github.com/calimero-network/mero-js/compare/mero-js-v24.1.0...mero-js-v24.1.1) (2026-10-06)

### Bug Fixes

* **device-cert:** mint device ids bound to their account ([#249](https://github.com/calimero-network/mero-js/issues/249)) ([60c13f7](https://github.com/calimero-network/mero-js/commit/60c13f703631d8c8850113b57f24e21d9d9097e6))
* **sse:** announce a reconnect only once the new session is re-subscribed ([#237](https://github.com/calimero-network/mero-js/issues/237)) ([e1c8fd1](https://github.com/calimero-network/mero-js/commit/e1c8fd1f45f18737f33421a6ec4a9c27693ce806))

## [24.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v24.0.0...mero-js-v24.1.0) (2026-10-06)

### Features

* **cloud:** carry a relay's admission state and join report on CloudRelay ([#248](https://github.com/calimero-network/mero-js/issues/248)) ([462f07a](https://github.com/calimero-network/mero-js/commit/462f07a53ada9b1dc89ef14717f3ec03a9a4fa0a))

## [24.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.9.0...mero-js-v24.0.0) (2026-10-06)

### ⚠ BREAKING CHANGES

* **account:** create a subgroup Open unless Restricted is asked for (#251)

### Bug Fixes

* **account:** create a subgroup Open unless Restricted is asked for ([#251](https://github.com/calimero-network/mero-js/issues/251)) ([9d0bf49](https://github.com/calimero-network/mero-js/commit/9d0bf49f07d021c0abb651dd35417cd10a7db869))

## [23.9.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.8.2...mero-js-v23.9.0) (2026-10-05)

### Features

* **account:** the delegated account layer lives in mero-js ([#247](https://github.com/calimero-network/mero-js/issues/247)) ([9ff5a7b](https://github.com/calimero-network/mero-js/commit/9ff5a7b2c3ce4cc7cbb9a916449509856cef5cff))

## [23.8.2](https://github.com/calimero-network/mero-js/compare/mero-js-v23.8.1...mero-js-v23.8.2) (2026-10-05)

### Bug Fixes

* **namespace-op:** sign namespace ops at schema version 22 ([#244](https://github.com/calimero-network/mero-js/issues/244)) ([fa1c93a](https://github.com/calimero-network/mero-js/commit/fa1c93a58fa3334964850e44132cd43a94c1aeb9)), closes [calimero-network/core#4263](https://github.com/calimero-network/core/issues/4263) [calimero-network/core#4453](https://github.com/calimero-network/core/issues/4453)

## [23.8.1](https://github.com/calimero-network/mero-js/compare/mero-js-v23.8.0...mero-js-v23.8.1) (2026-10-04)

### Bug Fixes

* **presence:** step the resend test one heartbeat at a time so its seq order is deterministic ([#246](https://github.com/calimero-network/mero-js/issues/246)) ([6979277](https://github.com/calimero-network/mero-js/commit/6979277be483f8f9c93d16e436c931bf6aa0d80d))

## [23.8.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.7.0...mero-js-v23.8.0) (2026-10-04)

### Features

* **cloud:** carry the relay's executor account and assignment in getAccountRelays ([#245](https://github.com/calimero-network/mero-js/issues/245)) ([9de9100](https://github.com/calimero-network/mero-js/commit/9de91002cc0826e342196cb581a8717e62fe83a3))

## [23.7.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.6.0...mero-js-v23.7.0) (2026-10-03)

### Features

* **admin:** type the founder attachments of a namespace ownership proof ([#243](https://github.com/calimero-network/mero-js/issues/243)) ([af00194](https://github.com/calimero-network/mero-js/commit/af00194f28b9b345f6a99bb92e4afb9059bd0a58))

## [23.6.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.5.0...mero-js-v23.6.0) (2026-10-03)

### Features

* **cloud:** sign the founding relay into the account HA claim and type its refusals ([#242](https://github.com/calimero-network/mero-js/issues/242)) ([ab3b5fe](https://github.com/calimero-network/mero-js/commit/ab3b5fea20e910ca8313d266fb4344a9579ce3d7))

## [23.5.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.4.3...mero-js-v23.5.0) (2026-10-03)

### Features

* **cloud:** enable HA as an account with no cloud session ([#241](https://github.com/calimero-network/mero-js/issues/241)) ([c738357](https://github.com/calimero-network/mero-js/commit/c738357944d408aa059bb3526249d1fb12faa9a9))

## [23.4.3](https://github.com/calimero-network/mero-js/compare/mero-js-v23.4.2...mero-js-v23.4.3) (2026-10-02)

### Bug Fixes

* **ephemeral:** a presence listener added after the context is subscribed still gets its current presence ([#240](https://github.com/calimero-network/mero-js/issues/240)) ([ce618de](https://github.com/calimero-network/mero-js/commit/ce618de9533af1060fa186fd1609eb3d9f9da99f))

## [23.4.2](https://github.com/calimero-network/mero-js/compare/mero-js-v23.4.1...mero-js-v23.4.2) (2026-10-02)

### Bug Fixes

* **namespace-op:** release signing at schema 21, the version core 0.11.0-rc.75 verifies ([#239](https://github.com/calimero-network/mero-js/issues/239)) ([53f480f](https://github.com/calimero-network/mero-js/commit/53f480f4a25081295d6ea49dbe3118425e37ec21))

## [23.4.1](https://github.com/calimero-network/mero-js/compare/mero-js-v23.4.0...mero-js-v23.4.1) (2026-10-02)

## [23.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.3.0...mero-js-v23.4.0) (2026-10-02)

### Features

* **relay:** found a namespace with its application ([#222](https://github.com/calimero-network/mero-js/issues/222)) ([626ea1a](https://github.com/calimero-network/mero-js/commit/626ea1adfe16c5527fa1661f4d83b568c410d2ba))

## [23.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.2.0...mero-js-v23.3.0) (2026-10-02)

### Features

* **presence:** an account publishes presence through its relay ([#230](https://github.com/calimero-network/mero-js/issues/230)) ([4aa5778](https://github.com/calimero-network/mero-js/commit/4aa57780b9dba1de33c79d2c1886288decd9c2e6))

## [23.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.1.0...mero-js-v23.2.0) (2026-10-02)

### Features

* **cloud:** enable HA for a namespace an account founded ([#225](https://github.com/calimero-network/mero-js/issues/225)) ([73b69af](https://github.com/calimero-network/mero-js/commit/73b69af58d6ea709ddd112ef9db519ad1b16097c)), closes [mdma#415](https://github.com/calimero-network/mdma/issues/415)

## [23.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v23.0.0...mero-js-v23.1.0) (2026-10-02)

### Features

* **warrant:** encode the rest of the delegable governance ops ([#224](https://github.com/calimero-network/mero-js/issues/224)) ([4f67ce0](https://github.com/calimero-network/mero-js/commit/4f67ce01459a829304191a1645ac29c05c1b82be))

## [23.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.9.0...mero-js-v23.0.0) (2026-10-01)

### ⚠ BREAKING CHANGES

* **warrant:** sign at schema 20, derive subgroup ids, and sign root proofs for owner ops (#219)

### Features

* **warrant:** sign at schema 20, derive subgroup ids, and sign root proofs for owner ops ([#219](https://github.com/calimero-network/mero-js/issues/219)) ([79187cf](https://github.com/calimero-network/mero-js/commit/79187cfa43e73961656fe65f7bf5583c0243a350))

## [22.9.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.8.0...mero-js-v22.9.0) (2026-10-01)

### Features

* **invitation:** mint invitations and carry a device link for a nodeless account ([#221](https://github.com/calimero-network/mero-js/issues/221)) ([8fea916](https://github.com/calimero-network/mero-js/commit/8fea916018e188c006d446db1d14e3cc762c1635))

## [22.8.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.7.0...mero-js-v22.8.0) (2026-09-30)

### Features

* signer interface, account root, cloud enrolment and per-request device proofs ([#203](https://github.com/calimero-network/mero-js/issues/203)) ([d31a48c](https://github.com/calimero-network/mero-js/commit/d31a48ce567583ad723ea1d954fe7eb91b2426fc)), closes [#312](https://github.com/calimero-network/mero-js/issues/312) [#4018](https://github.com/calimero-network/mero-js/issues/4018) [#193](https://github.com/calimero-network/mero-js/issues/193)

## [22.7.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.6.0...mero-js-v22.7.0) (2026-09-30)

### Features

* **relay:** found a namespace through a relay ([#218](https://github.com/calimero-network/mero-js/issues/218)) ([c730bca](https://github.com/calimero-network/mero-js/commit/c730bca5270ef55b16657199f002eab2c1c785c7))

## [22.6.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.5.1...mero-js-v22.6.0) (2026-09-30)

### Features

* **warrant:** sign creation and governance warrants with a Signer ([#217](https://github.com/calimero-network/mero-js/issues/217)) ([1c619e4](https://github.com/calimero-network/mero-js/commit/1c619e4bcf4b64cf2e15d05d898ff3102d5e1864)), closes [#203](https://github.com/calimero-network/mero-js/issues/203)

## [22.5.1](https://github.com/calimero-network/mero-js/compare/mero-js-v22.5.0...mero-js-v22.5.1) (2026-09-29)

### Bug Fixes

* **namespace-op:** sign namespace ops at schema version 16 ([#216](https://github.com/calimero-network/mero-js/issues/216)) ([766d3d5](https://github.com/calimero-network/mero-js/commit/766d3d594d4ff732ad56a6650d56d743906d7a67))

## [22.5.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.4.0...mero-js-v22.5.0) (2026-09-29)

### Features

* **relay:** schema 15 and delegated governance through a relay ([#215](https://github.com/calimero-network/mero-js/issues/215)) ([d185aec](https://github.com/calimero-network/mero-js/commit/d185aec862d62d02b93e8f95de511f2fcad289dd))

## [22.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.3.0...mero-js-v22.4.0) (2026-09-29)

### Features

* **relay:** create contexts through a relay with a creation warrant ([#214](https://github.com/calimero-network/mero-js/issues/214)) ([292fdc5](https://github.com/calimero-network/mero-js/commit/292fdc5a7682130aaf0c664fb5359633814c3b77))

## [22.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.2.0...mero-js-v22.3.0) (2026-09-29)

### Features

* **sealed:** name the release for nodes that refuse /tee/info unsealed ([#212](https://github.com/calimero-network/mero-js/issues/212)) ([361ad19](https://github.com/calimero-network/mero-js/commit/361ad1972a2c2c4b418c781a9b6490b08a1dc603))

## [22.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.1.0...mero-js-v22.2.0) (2026-09-29)

### Features

* **cloud:** pick relays on mdma's can_execute, not authorship_ready ([#211](https://github.com/calimero-network/mero-js/issues/211)) ([f4c16f3](https://github.com/calimero-network/mero-js/commit/f4c16f3d9d05a28dfeab1fa800791c144cad9a55))

## [22.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v22.0.0...mero-js-v22.1.0) (2026-09-29)

### Features

* **admin:** add the RelayTee role and the TEE admission policy mode ([#210](https://github.com/calimero-network/mero-js/issues/210)) ([592a7e0](https://github.com/calimero-network/mero-js/commit/592a7e0f962b21203e3e7c0b85407881c3538d39))

## [22.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v21.4.1...mero-js-v22.0.0) (2026-09-28)

### ⚠ BREAKING CHANGES

* **admin:** drop the chosen group id from CreateGroupRequest (#202)

### Features

* **admin:** drop the chosen group id from CreateGroupRequest ([#202](https://github.com/calimero-network/mero-js/issues/202)) ([34bbef4](https://github.com/calimero-network/mero-js/commit/34bbef4a83ebc583c8759a4914cd3f334444b246))

## [21.4.1](https://github.com/calimero-network/mero-js/compare/mero-js-v21.4.0...mero-js-v21.4.1) (2026-09-28)

### Bug Fixes

* **namespace-op:** sign namespace ops at schema version 11 ([#208](https://github.com/calimero-network/mero-js/issues/208)) ([8ed336b](https://github.com/calimero-network/mero-js/commit/8ed336b7e06668ee065a357f9f13d38502f63fa7)), closes [core#4172](https://github.com/calimero-network/core/issues/4172)

## [21.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v21.3.0...mero-js-v21.4.0) (2026-09-28)

### Features

* **sealed:** verify signed mero-tee releases at run time; sign namespace ops at schema 10 ([#206](https://github.com/calimero-network/mero-js/issues/206)) ([7f653ae](https://github.com/calimero-network/mero-js/commit/7f653aedd2524fae879013353a63dd2a812f3e09))

## [21.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v21.2.2...mero-js-v21.3.0) (2026-09-28)

### Features

* **admin:** classifyError, and redeemInvitation that settles a join by membership ([#207](https://github.com/calimero-network/mero-js/issues/207)) ([12ad2a4](https://github.com/calimero-network/mero-js/commit/12ad2a416d6372ee5aee488e6bce98fb407568fc))

## [21.2.2](https://github.com/calimero-network/mero-js/compare/mero-js-v21.2.1...mero-js-v21.2.2) (2026-09-28)

### Bug Fixes

* **namespace-op:** sign namespace ops at schema version 9 ([#201](https://github.com/calimero-network/mero-js/issues/201)) ([5b721dd](https://github.com/calimero-network/mero-js/commit/5b721dd3c1733b5925ae1214e308a4ad7aceb159)), closes [core#4163](https://github.com/calimero-network/core/issues/4163)

## [21.2.1](https://github.com/calimero-network/mero-js/compare/mero-js-v21.2.0...mero-js-v21.2.1) (2026-09-28)

### Bug Fixes

* **relay:** keep the reason when the fetch itself fails ([#205](https://github.com/calimero-network/mero-js/issues/205)) ([d20ece7](https://github.com/calimero-network/mero-js/commit/d20ece788b0338552bd01abe6b82dea14c60629f))

## [21.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v21.1.0...mero-js-v21.2.0) (2026-09-28)

### Features

* **admin:** declare the namespace legacy founding field ([#199](https://github.com/calimero-network/mero-js/issues/199)) ([64f98a9](https://github.com/calimero-network/mero-js/commit/64f98a9c989e66c9735fbe15faa807a830ad3cd1))

## [21.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v21.0.0...mero-js-v21.1.0) (2026-09-28)

### Features

* **admin:** declare the namespace founding field ([#198](https://github.com/calimero-network/mero-js/issues/198)) ([ffd3f59](https://github.com/calimero-network/mero-js/commit/ffd3f5968996abd6be1597f4b29a620ba5ae7c5e))

## [21.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v20.0.0...mero-js-v21.0.0) (2026-09-27)

### ⚠ BREAKING CHANGES

* **admin:** remove getCertificate; test(e2e): cover upgrade retry (#196)

### Code Refactoring

* **admin:** remove getCertificate; test(e2e): cover upgrade retry ([#196](https://github.com/calimero-network/mero-js/issues/196)) ([19bd45f](https://github.com/calimero-network/mero-js/commit/19bd45f098e74f7f539462a37bf7ed72efba72ac))

## [20.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.27.1...mero-js-v20.0.0) (2026-09-27)

### ⚠ BREAKING CHANGES

* **sealed:** trust a TEE image by all of its measurements, and build them from releases (#195)

### Bug Fixes

* **sealed:** trust a TEE image by all of its measurements, and build them from releases ([#195](https://github.com/calimero-network/mero-js/issues/195)) ([a631b5f](https://github.com/calimero-network/mero-js/commit/a631b5f19a50e6f092f7a41c713f73e5cad2cf8a))

## [19.27.1](https://github.com/calimero-network/mero-js/compare/mero-js-v19.27.0...mero-js-v19.27.1) (2026-09-27)

### Bug Fixes

* **login:** keep the global fetch receiver on the default path ([#193](https://github.com/calimero-network/mero-js/issues/193)) ([fefbd07](https://github.com/calimero-network/mero-js/commit/fefbd0725e6bad0213aad6b3b25851b65edc916f))

## [19.27.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.26.0...mero-js-v19.27.0) (2026-09-27)

### Features

* **sealed:** seal delegated execution to a TEE relay end to end ([#189](https://github.com/calimero-network/mero-js/issues/189)) ([8b8c7b4](https://github.com/calimero-network/mero-js/commit/8b8c7b49b80bbd50861654cd4b75df2e5aa14cfa))

## [19.26.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.25.0...mero-js-v19.26.0) (2026-09-27)

### Features

* **sealed:** verify a TEE node's quote in the page, and name what React Native needs ([#186](https://github.com/calimero-network/mero-js/issues/186)) ([516b000](https://github.com/calimero-network/mero-js/commit/516b00003124a0317e6d67179a6a70bd75f162cb))

## [19.25.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.24.1...mero-js-v19.25.0) (2026-09-26)

### Features

* **admin-api:** add readinessCheck, listMemberDevices, sealToAccount, queryContext ([#181](https://github.com/calimero-network/mero-js/issues/181)) ([aabf7ce](https://github.com/calimero-network/mero-js/commit/aabf7cef93349507cff1bd149601d38db440aab8))

## [19.24.1](https://github.com/calimero-network/mero-js/compare/mero-js-v19.24.0...mero-js-v19.24.1) (2026-09-26)

### Bug Fixes

* **sealed:** bound what a proxy can make the client buffer, and retry a busy handshake ([#180](https://github.com/calimero-network/mero-js/issues/180)) ([eb9ce20](https://github.com/calimero-network/mero-js/commit/eb9ce205171491c5ecc39e94b918ebabbb4bfad6))

## [19.24.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.23.0...mero-js-v19.24.0) (2026-09-26)

### Features

* **admin-api:** add disableTeeAuthoringPolicy for DELETE tee-authoring-policy ([#179](https://github.com/calimero-network/mero-js/issues/179)) ([f833d1e](https://github.com/calimero-network/mero-js/commit/f833d1e954f7c99b1133744ac31c5d03332779be))

## [19.23.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.22.0...mero-js-v19.23.0) (2026-09-26)

### Features

* **sealed:** open forward-secret sessions and stream sealed responses ([#178](https://github.com/calimero-network/mero-js/issues/178)) ([f46cc27](https://github.com/calimero-network/mero-js/commit/f46cc273beae5d97466ca1e5b90ee31e289084c6))

## [19.22.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.21.0...mero-js-v19.22.0) (2026-09-26)

### Features

* **admin-api:** type the signed-release TEE admission policy ([#174](https://github.com/calimero-network/mero-js/issues/174)) ([5564435](https://github.com/calimero-network/mero-js/commit/5564435199a7706eeaeb18d60007056c57aaa09f))

## [19.21.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.20.0...mero-js-v19.21.0) (2026-09-26)

### Features

* **sealed:** seal requests to a TEE node's attested transport key ([#177](https://github.com/calimero-network/mero-js/issues/177)) ([820fdbd](https://github.com/calimero-network/mero-js/commit/820fdbd55bef290bcc61f0179d04b40a063f0f18))

## [19.20.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.19.0...mero-js-v19.20.0) (2026-09-26)

### Features

* **admin-api:** add setTeeAuthoringPolicy for the tee-authoring-policy route ([#175](https://github.com/calimero-network/mero-js/issues/175)) ([3156824](https://github.com/calimero-network/mero-js/commit/3156824b849e863b1e95424e5287fcd38fb5dcdd)), closes [#4059](https://github.com/calimero-network/mero-js/issues/4059) [core#4059](https://github.com/calimero-network/core/issues/4059)

## [19.19.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.18.0...mero-js-v19.19.0) (2026-09-26)

### Features

* **cloud:** let enable-ha carry the owner node's admitter addresses ([#173](https://github.com/calimero-network/mero-js/issues/173)) ([3770457](https://github.com/calimero-network/mero-js/commit/3770457b0cddc1a5c728e44225ca5da338017c79))

## [19.18.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.17.0...mero-js-v19.18.0) (2026-09-26)

### Features

* **http-client:** support a custom fetch implementation in MeroJsConfig ([#171](https://github.com/calimero-network/mero-js/issues/171)) ([34b71e0](https://github.com/calimero-network/mero-js/commit/34b71e0053c6af94f4b6b0567dc386813562b3ac))

## [19.17.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.16.0...mero-js-v19.17.0) (2026-09-24)

### Features

* **mero-js:** let a MeroJs client sign its requests ([#169](https://github.com/calimero-network/mero-js/issues/169)) ([02b56c5](https://github.com/calimero-network/mero-js/commit/02b56c51302fbab5ecc858df0b8b03aa88e9a9a4))

## [19.16.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.15.0...mero-js-v19.16.0) (2026-09-24)

### Features

* **request:** carry a signed proof on every request ([#168](https://github.com/calimero-network/mero-js/issues/168)) ([ae9c808](https://github.com/calimero-network/mero-js/commit/ae9c8085886eb89cb5ae562dfa485049a0acf6a9))

## [19.15.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.14.1...mero-js-v19.15.0) (2026-09-24)

### Features

* **request:** sign one request, matching core's byte contract ([#167](https://github.com/calimero-network/mero-js/issues/167)) ([90d8f82](https://github.com/calimero-network/mero-js/commit/90d8f82c430b85d586b872d8f1bb03692ae04c70))

## [19.14.1](https://github.com/calimero-network/mero-js/compare/mero-js-v19.14.0...mero-js-v19.14.1) (2026-09-22)

### Bug Fixes

* **events:** a revoked token family arrives on SSE as 403, and the check only ever looked at 401 ([#166](https://github.com/calimero-network/mero-js/issues/166)) ([938c3e4](https://github.com/calimero-network/mero-js/commit/938c3e40690caa2c7d98cbd10f335f5f778bc25a))

## [19.14.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.13.1...mero-js-v19.14.0) (2026-09-20)

### Features

* **admin:** sign a verifier's payload with the account root ([#164](https://github.com/calimero-network/mero-js/issues/164)) ([5812286](https://github.com/calimero-network/mero-js/commit/5812286db1a68fef66e57c4e5ce4b9f60ba1f256))

## [19.13.1](https://github.com/calimero-network/mero-js/compare/mero-js-v19.13.0...mero-js-v19.13.1) (2026-09-18)

## [19.13.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.12.0...mero-js-v19.13.0) (2026-09-18)

### Features

* **cloud:** find the relays that serve an account, with a device certificate ([#163](https://github.com/calimero-network/mero-js/issues/163)) ([6451505](https://github.com/calimero-network/mero-js/commit/64515059b69dd81c5a466dca6c41d1fead7844f4))

## [19.12.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.11.0...mero-js-v19.12.0) (2026-09-18)

### Features

* **admin-api:** name a device, and surface what revoked this node ([#161](https://github.com/calimero-network/mero-js/issues/161)) ([c030838](https://github.com/calimero-network/mero-js/commit/c030838bae3062c617c2e8405c6c4e7f1f4023cc))

## [19.11.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.10.0...mero-js-v19.11.0) (2026-09-18)

### Features

* **admin-api:** replace a device's scope ([#160](https://github.com/calimero-network/mero-js/issues/160)) ([5be5f1f](https://github.com/calimero-network/mero-js/commit/5be5f1f4fb3f14443fa614b419377b44bc75fda3))

## [19.10.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.9.0...mero-js-v19.10.0) (2026-09-17)

### Features

* **cloud:** link an account with a grant, for apps that hold no session ([#158](https://github.com/calimero-network/mero-js/issues/158)) ([d330ffc](https://github.com/calimero-network/mero-js/commit/d330ffcb8e4a83761af8c5bcf120bc2d920578bb))

## [19.9.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.8.1...mero-js-v19.9.0) (2026-09-17)

### Features

* **cloud:** sign in as an account with its root key ([#157](https://github.com/calimero-network/mero-js/issues/157)) ([3743d6a](https://github.com/calimero-network/mero-js/commit/3743d6a8b88f0c246cb30d42ea94e1e800c75319))

## [19.8.1](https://github.com/calimero-network/mero-js/compare/mero-js-v19.8.0...mero-js-v19.8.1) (2026-09-17)

## [19.8.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.7.2...mero-js-v19.8.0) (2026-09-17)

### Features

* **cloud:** prove an account on the routing read ([#156](https://github.com/calimero-network/mero-js/issues/156)) ([1778c75](https://github.com/calimero-network/mero-js/commit/1778c757df5b37e40b29789a56b2b6d2995a9712)), closes [mdma#248](https://github.com/calimero-network/mdma/issues/248)

## [19.7.2](https://github.com/calimero-network/mero-js/compare/mero-js-v19.7.1...mero-js-v19.7.2) (2026-09-17)

## [19.7.1](https://github.com/calimero-network/mero-js/compare/mero-js-v19.7.0...mero-js-v19.7.1) (2026-09-17)

## [19.7.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.6.0...mero-js-v19.7.0) (2026-09-17)

### Features

* **cloud, admin:** namespace routing without a cloud account, and one call to open a namespace to relays ([#154](https://github.com/calimero-network/mero-js/issues/154)) ([e4f7623](https://github.com/calimero-network/mero-js/commit/e4f76237124454b2d174cae573d766b7a22a29ad))

## [19.6.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.5.0...mero-js-v19.6.0) (2026-09-17)

### Features

* **account:** mint an account root with a recovery phrase, and link it to the cloud ([#153](https://github.com/calimero-network/mero-js/issues/153)) ([903eab1](https://github.com/calimero-network/mero-js/commit/903eab18accfc8b33c564fde10a5c88c898c4f54))

## [19.5.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.4.0...mero-js-v19.5.0) (2026-09-16)

### Features

* **admin:** device alias methods ([#152](https://github.com/calimero-network/mero-js/issues/152)) ([46141d9](https://github.com/calimero-network/mero-js/commit/46141d9ee61f6f8a4a62fcfeb8b073dca7f5e51c))

## [19.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.3.0...mero-js-v19.4.0) (2026-09-16)

### Features

* **login:** the handshake around the signer, not just the signature ([#151](https://github.com/calimero-network/mero-js/issues/151)) ([ff31a7c](https://github.com/calimero-network/mero-js/commit/ff31a7ce184069bb14d04a4e0359934f800d953d)), closes [#84](https://github.com/calimero-network/mero-js/issues/84) [#87](https://github.com/calimero-network/mero-js/issues/87) [#88](https://github.com/calimero-network/mero-js/issues/88)

## [19.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.2.0...mero-js-v19.3.0) (2026-09-16)

### Features

* **login:** sign the statement a device presents for a session ([#150](https://github.com/calimero-network/mero-js/issues/150)) ([5dde61c](https://github.com/calimero-network/mero-js/commit/5dde61c0134d1100b8117f7ca7a081394c99cded))

## [19.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.1.0...mero-js-v19.2.0) (2026-09-16)

### Features

* **admin:** identity and pair-init carry the account namespace ([#149](https://github.com/calimero-network/mero-js/issues/149)) ([d93b26d](https://github.com/calimero-network/mero-js/commit/d93b26d88bc11aa87e5e12cff51f0fe86ea5ac59))

## [19.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v19.0.0...mero-js-v19.1.0) (2026-09-16)

### Features

* **admin-api:** accept a context on blob reads for network discovery ([#142](https://github.com/calimero-network/mero-js/issues/142)) ([013d9e2](https://github.com/calimero-network/mero-js/commit/013d9e232a7807bc9e1b4396b12ab07a14c404f5))

## [19.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v18.3.0...mero-js-v19.0.0) (2026-09-16)

### ⚠ BREAKING CHANGES

* **warrant:** sign the v2 layout core now verifies (#148)

### Features

* **warrant:** sign the v2 layout core now verifies ([#148](https://github.com/calimero-network/mero-js/issues/148)) ([e0e8fc7](https://github.com/calimero-network/mero-js/commit/e0e8fc758ba061b010ee03345166fbea9a319774)), closes [core#3933](https://github.com/calimero-network/core/issues/3933)

## [18.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v18.2.0...mero-js-v18.3.0) (2026-09-10)

### Features

* **cloud,relay:** write through a relay with only a signing key ([#135](https://github.com/calimero-network/mero-js/issues/135)) ([a670bfc](https://github.com/calimero-network/mero-js/commit/a670bfc68dcb3c8cb373c860d027dc6bd14096e4))

## [18.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v18.1.1...mero-js-v18.2.0) (2026-09-04)

### Features

* **admin:** surface the account each migration-status row speaks for ([#105](https://github.com/calimero-network/mero-js/issues/105)) ([005597c](https://github.com/calimero-network/mero-js/commit/005597cd61055ee4d3db1709fd7e44851dd9a890)), closes [#3558](https://github.com/calimero-network/mero-js/issues/3558)

## [18.1.1](https://github.com/calimero-network/mero-js/compare/mero-js-v18.1.0...mero-js-v18.1.1) (2026-09-04)

### Bug Fixes

* **admin:** send only the fields core declares ([#136](https://github.com/calimero-network/mero-js/issues/136)) ([fc95c0d](https://github.com/calimero-network/mero-js/commit/fc95c0d6190d94a16b73d187cb26f1fed8be5014))

## [18.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v18.0.0...mero-js-v18.1.0) (2026-09-03)

### Features

* **admin:** account-level device pairing, relink and listings ([#115](https://github.com/calimero-network/mero-js/issues/115)) ([2cb206e](https://github.com/calimero-network/mero-js/commit/2cb206ee14215a7be711f63332b84d16a50034bb)), closes [core#3774](https://github.com/calimero-network/core/issues/3774)

## [18.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v17.0.0...mero-js-v18.0.0) (2026-09-03)

### ⚠ BREAKING CHANGES

* **admin:** installApplication takes { package, version } instead of
{ url, hash, metadata }; installFromRegistry is removed - call
installApplication with the coordinates directly. installDevApplication takes
{ path } only, following core dropping metadata from the dev request.

* test(e2e): assert the node accepts the install coordinate shape

The route-coverage sweep fires install-application through cover(), which
swallows every failure. That is right for a state 4xx, but it also swallows a
400 from a rejected request BODY - and the coverage recorder logs a path when
the request fires, before the response, so the route reads as covered while
every call to it fails.

That combination is why core's paired SDK e2e passed on the registry-only
distribution change while this SDK still sent the old url-shaped body: the one
place that exercises the route could not tell "nothing published there" from
"your request shape is wrong".

install-application now asserts instead. These coordinates have nothing
published at them, so the install cannot succeed; what is checked is that the
node got past deserialization. Deliberately not pinned to 502: install_by_coords
propagates a fetch fault as an error, so an unreachable registry is a 500, and
requiring 502 would make this depend on CI egress to the node's configured
public registry. A status is required though, so a transport fault cannot pass
the assertion vacuously.

Verified against merod 0.11.0-rc.30, which predates the change: the coordinate
body earns `400 missing field \`url\``, which fails the new assertion.

The file header claimed a covered 4xx "proves the SDK builds and sends a correct
request". It does not prove that about the body, and that sentence is what made
the hole look intentional - corrected, with a note to assert the shape on any
route where core rejects unknown fields.

* ci: take the newest release that actually has a merod asset

Both e2e jobs resolved merod with `gh release list --limit 1` and hard-failed
when that tag carried no linux binary. GitHub publishes a release object before
its binary matrix finishes uploading, so the newest tag legitimately has no
merod for a while - 0.11.0-rc.31 sat published with only its test fixture for
several minutes, and any PR whose e2e ran in that window failed on
"no merod linux asset", indistinguishable from a genuinely broken release.

Walk back through recent releases and take the first that has the asset. Both
jobs had their own copy of the download, so it moves to one script they share
rather than being fixed twice.

Checked both directions against the live release state: it skips rc.31 and
downloads rc.30, and it exits 1 when no release in the lookback window carries
the asset.

### Features

* **admin:** install applications by package@version ([#133](https://github.com/calimero-network/mero-js/issues/133)) ([dfd2b9e](https://github.com/calimero-network/mero-js/commit/dfd2b9e9c5b1deab94b3c7a2ba2dabed6ff6d0ff))

## [17.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v16.1.0...mero-js-v17.0.0) (2026-09-03)

### ⚠ BREAKING CHANGES

* **namespace-op:** write the endorsement slot on the envelope (#134)

### Bug Fixes

* **namespace-op:** write the endorsement slot on the envelope ([#134](https://github.com/calimero-network/mero-js/issues/134)) ([d6f99ca](https://github.com/calimero-network/mero-js/commit/d6f99ca2ea2219cd82e1287c4de31a95d7e458ec)), closes [core#3804](https://github.com/calimero-network/core/issues/3804) [core#3819](https://github.com/calimero-network/core/issues/3819) [#3804](https://github.com/calimero-network/mero-js/issues/3804) [calimero-network/core#3819](https://github.com/calimero-network/core/issues/3819)

## [16.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v16.0.0...mero-js-v16.1.0) (2026-09-01)

### Features

* **invitation:** follow core's rename to admitter_addrs ([#132](https://github.com/calimero-network/mero-js/issues/132)) ([a1e6949](https://github.com/calimero-network/mero-js/commit/a1e6949b6c89a5150c7ec6d4c35602da1d4574d5))

## [16.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v15.3.4...mero-js-v16.0.0) (2026-09-01)

### ⚠ BREAKING CHANGES

* **release:** <text>     -> major
  fix: ...                    -> patch
  feat: ...                   -> minor

Co-authored-by: Claude Opus 5 (1M context) <noreply@anthropic.com>

### Bug Fixes

* **release:** stop prose from being read as a breaking change ([#131](https://github.com/calimero-network/mero-js/issues/131)) ([1673df8](https://github.com/calimero-network/mero-js/commit/1673df8d72f1eb4fba67e497e9065cc28e89cc62))

## [15.3.4](https://github.com/calimero-network/mero-js/compare/mero-js-v15.3.3...mero-js-v15.3.4) (2026-08-31)

### Bug Fixes

* **namespace-op:** emit joined_at, and pick the variant from the invitation ([#129](https://github.com/calimero-network/mero-js/issues/129)) ([80cf1cb](https://github.com/calimero-network/mero-js/commit/80cf1cb077c40469d9e6dd3c523e0811845c7e2d))

## [15.3.3](https://github.com/calimero-network/mero-js/compare/mero-js-v15.3.2...mero-js-v15.3.3) (2026-08-31)

### Bug Fixes

* **device-cert:** write genesis version 2, the one core accepts ([#128](https://github.com/calimero-network/mero-js/issues/128)) ([5a2c647](https://github.com/calimero-network/mero-js/commit/5a2c647a1c4f7197d7e31e560eb1c89aefe357fc))

## [15.3.2](https://github.com/calimero-network/mero-js/compare/mero-js-v15.3.1...mero-js-v15.3.2) (2026-08-31)

### Bug Fixes

* **e2e:** assert the substitution property where it is actually enforced ([#127](https://github.com/calimero-network/mero-js/issues/127)) ([134fe8e](https://github.com/calimero-network/mero-js/commit/134fe8e3fa60969dd5fc91487ab0541ad8db5faf))

## [15.3.1](https://github.com/calimero-network/mero-js/compare/mero-js-v15.3.0...mero-js-v15.3.1) (2026-08-31)

### Bug Fixes

* **contract:** declare admitters, which every current node now sends ([#126](https://github.com/calimero-network/mero-js/issues/126)) ([f7de4f8](https://github.com/calimero-network/mero-js/commit/f7de4f8279cbd623b78b4618c7968fdb30ee4049)), closes [#124](https://github.com/calimero-network/mero-js/issues/124) [#124](https://github.com/calimero-network/mero-js/issues/124)

## [15.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v15.2.0...mero-js-v15.3.0) (2026-08-31)

### Features

* **namespace-op:** sign a membership op from the SDK, so a keyholder can join ([#125](https://github.com/calimero-network/mero-js/issues/125)) ([a15dd6e](https://github.com/calimero-network/mero-js/commit/a15dd6e2b2b34ed2ce8b7697fff0364db833c131))

## [15.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v15.1.0...mero-js-v15.2.0) (2026-08-30)

### Features

* **admin-api:** present a signed join to a designated admitter ([#124](https://github.com/calimero-network/mero-js/issues/124)) ([459f394](https://github.com/calimero-network/mero-js/commit/459f3944cb255497a0f907bedeeb528c51ffce53))

## [15.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v15.0.0...mero-js-v15.1.0) (2026-08-29)

### Features

* **device-cert:** certify a device offline from the SDK ([#123](https://github.com/calimero-network/mero-js/issues/123)) ([b20e1d6](https://github.com/calimero-network/mero-js/commit/b20e1d6466e57fd21641a260018199133ef425f4))

## [15.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v14.0.0...mero-js-v15.0.0) (2026-08-28)

### ⚠ BREAKING CHANGES

* **account:** `toAccountHex`, `toAccountBase58` and `sameAccount` are removed.
Every id core emits is 64 lowercase hex, so comparing two ids is now `===`. A
caller holding base58 account ids has data predating the migration; decode it
once rather than converting on every comparison.

Co-authored-by: Claude Opus 5 (1M context) <noreply@anthropic.com>

### Features

* **account:** drop the account encoding bridge ([#119](https://github.com/calimero-network/mero-js/issues/119)) ([0090ac7](https://github.com/calimero-network/mero-js/commit/0090ac7459db6c5826e66a68657c58574809d865)), closes [calimero-network/core#3696](https://github.com/calimero-network/core/issues/3696)

## [14.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v13.4.0...mero-js-v14.0.0) (2026-08-28)

### ⚠ BREAKING CHANGES

* **warrant:** the context is hex, and base58 is gone from this package (#118)

### Bug Fixes

* **warrant:** the context is hex, and base58 is gone from this package ([#118](https://github.com/calimero-network/mero-js/issues/118)) ([9298bea](https://github.com/calimero-network/mero-js/commit/9298beaeadaeb04d2eee3d12641e44e0da6914f0))

## [13.4.0](https://github.com/calimero-network/mero-js/compare/mero-js-v13.3.0...mero-js-v13.4.0) (2026-08-28)

### Features

* **warrant:** mint warrants in JS, with no dependencies ([#116](https://github.com/calimero-network/mero-js/issues/116)) ([8b9ad6a](https://github.com/calimero-network/mero-js/commit/8b9ad6a9aabaea82351fdb9777972b474b721443))

## [13.3.0](https://github.com/calimero-network/mero-js/compare/mero-js-v13.2.5...mero-js-v13.3.0) (2026-08-26)

### Features

* **admin:** bind performIntent for delegated authorship ([#113](https://github.com/calimero-network/mero-js/issues/113)) ([f51e831](https://github.com/calimero-network/mero-js/commit/f51e831e7dc3b03accf320e49febbbd9b78c1f3e)), closes [core#3636](https://github.com/calimero-network/core/issues/3636) [core#3640](https://github.com/calimero-network/core/issues/3640)

## [13.2.5](https://github.com/calimero-network/mero-js/compare/mero-js-v13.2.4...mero-js-v13.2.5) (2026-08-26)

### Bug Fixes

* **fleet:** let the pnpm probe find nothing without killing the step ([#114](https://github.com/calimero-network/mero-js/issues/114)) ([be7aa50](https://github.com/calimero-network/mero-js/commit/be7aa508e315cb336f584578916f66ef5b2e2b9f))

## [13.2.4](https://github.com/calimero-network/mero-js/compare/mero-js-v13.2.3...mero-js-v13.2.4) (2026-08-25)

### Bug Fixes

* **fleet:** record a root lockfile at its repository-relative path ([#112](https://github.com/calimero-network/mero-js/issues/112)) ([95fc7fb](https://github.com/calimero-network/mero-js/commit/95fc7fb60098141f5ef5c52e73f791d9b9665b1f))

## [13.2.3](https://github.com/calimero-network/mero-js/compare/mero-js-v13.2.2...mero-js-v13.2.3) (2026-08-25)

### Bug Fixes

* **fleet:** bump with the pnpm the repository actually uses ([#111](https://github.com/calimero-network/mero-js/issues/111)) ([cf2442d](https://github.com/calimero-network/mero-js/commit/cf2442d624b1536ab9dc4f2dea279ea5a9e215c1))

## [13.2.2](https://github.com/calimero-network/mero-js/compare/mero-js-v13.2.1...mero-js-v13.2.2) (2026-08-24)

### Bug Fixes

* **fleet:** commit only the files the bump claims ([#110](https://github.com/calimero-network/mero-js/issues/110)) ([1ac802b](https://github.com/calimero-network/mero-js/commit/1ac802bd0498aad4065c922727b44bbee751332a)), closes [#190](https://github.com/calimero-network/mero-js/issues/190) [#194](https://github.com/calimero-network/mero-js/issues/194)

## [13.2.1](https://github.com/calimero-network/mero-js/compare/mero-js-v13.2.0...mero-js-v13.2.1) (2026-08-24)

### Bug Fixes

* **admin:** read the namespace-join id under either spelling ([#108](https://github.com/calimero-network/mero-js/issues/108)) ([5d040ab](https://github.com/calimero-network/mero-js/commit/5d040ab974f5fe4beb70649c180eabd9a724a385)), closes [core#3598](https://github.com/calimero-network/core/issues/3598)

## [13.2.0](https://github.com/calimero-network/mero-js/compare/mero-js-v13.1.0...mero-js-v13.2.0) (2026-08-21)

### Features

* **account:** canonicalise account ids, and give SSE failures their reason ([#106](https://github.com/calimero-network/mero-js/issues/106)) ([82e58a5](https://github.com/calimero-network/mero-js/commit/82e58a58cfacab9971aaf667d6a41d5d653b3123))

## [13.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v13.0.0...mero-js-v13.1.0) (2026-08-18)

### Features

* **ephemeral:** add mero.ephemeral presence client (set/get/subscribe) ([#97](https://github.com/calimero-network/mero-js/issues/97)) ([d77affd](https://github.com/calimero-network/mero-js/commit/d77affdb5a5ebfe314a18a3f81cfcb0acaae8198))

## [13.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v12.1.0...mero-js-v13.0.0) (2026-08-18)

### ⚠ BREAKING CHANGES

* **admin:** `upgradePolicy` is gone from the `Namespace` and `GroupInfo`
response types. It only ever held `"LazyOnAccess"`, so no working code can
depend on its value; code that destructures it needs the name removed.

### Features

* **admin:** drop upgradePolicy from the Namespace and GroupInfo responses ([#104](https://github.com/calimero-network/mero-js/issues/104)) ([c742269](https://github.com/calimero-network/mero-js/commit/c74226950eff67f344b691ecd7dcd328acb29c1b)), closes [calimero-network/core#3485](https://github.com/calimero-network/core/issues/3485) [core#3393](https://github.com/calimero-network/core/issues/3393) [calimero-network/core#3485](https://github.com/calimero-network/core/issues/3485)

## [12.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v12.0.0...mero-js-v12.1.0) (2026-08-18)

### Features

* **events:** model the GroupMigration event family and add onMigrationEvent ([#94](https://github.com/calimero-network/mero-js/issues/94)) ([091957e](https://github.com/calimero-network/mero-js/commit/091957e239e4ee5825be16848b79c38d69c95e08))

### Bug Fixes

* **admin-api:** rename GroupUpgradeStatus/UpgradeGroupResponseData counters to localContexts* ([#93](https://github.com/calimero-network/mero-js/issues/93)) ([e76c316](https://github.com/calimero-network/mero-js/commit/e76c31612b7948b2144c0a87626d4b5657b51f2f))
* **events:** rename GroupMembershipEventData.member to memberAccount ([#95](https://github.com/calimero-network/mero-js/issues/95)) ([03dfb55](https://github.com/calimero-network/mero-js/commit/03dfb55cf35fd1373260fe63dc88384b732763a9))

## [12.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v11.1.0...mero-js-v12.0.0) (2026-08-18)

### ⚠ BREAKING CHANGES

* **admin:** `governanceOp` is gone from `JoinGroupResponseData` and
`JoinNamespaceResponseData`. It only ever held `""`, so no working code can
depend on its value; code that destructures it needs the name removed.

Co-authored-by: Claude Opus 5 (1M context) <noreply@anthropic.com>

### Bug Fixes

* **admin:** replace governanceOp with memberAccount on join responses ([#103](https://github.com/calimero-network/mero-js/issues/103)) ([5418ae6](https://github.com/calimero-network/mero-js/commit/5418ae648baaeb44dc72502bd63cf7b07d6f9fa3))

## [11.1.0](https://github.com/calimero-network/mero-js/compare/mero-js-v11.0.0...mero-js-v11.1.0) (2026-08-17)

### Features

* **admin:** add getNodeIdentity, deprecate getNamespaceIdentity ([#101](https://github.com/calimero-network/mero-js/issues/101)) ([bc134df](https://github.com/calimero-network/mero-js/commit/bc134dfad66c9bdf52e60c74bad26d1fb0b90ad4))

## [11.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v10.0.0...mero-js-v11.0.0) (2026-08-14)

### ⚠ BREAKING CHANGES

* **admin:** the four *ContextIdentityAlias methods on AdminApiClient are
removed, along with CreateContextIdentityAliasRequest and the
List/Create/Lookup/DeleteContextIdentityAliasResponseData types. Manage
identities without per-context aliases.

### Features

* **admin:** drop context identity alias methods ([#100](https://github.com/calimero-network/mero-js/issues/100)) ([162259c](https://github.com/calimero-network/mero-js/commit/162259cd410301dec526abd37a4757d890d79644))

## [10.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v9.0.2...mero-js-v10.0.0) (2026-08-14)

### ⚠ BREAKING CHANGES

* **admin:** drop registerGroupSigningKey (#99)

### Bug Fixes

* **admin:** drop registerGroupSigningKey ([#99](https://github.com/calimero-network/mero-js/issues/99)) ([fa76803](https://github.com/calimero-network/mero-js/commit/fa76803a5e37aa71f9f8a55f9a9840b19fc59191)), closes [calimero-network/core#3439](https://github.com/calimero-network/core/issues/3439)

## [9.0.2](https://github.com/calimero-network/mero-js/compare/mero-js-v9.0.1...mero-js-v9.0.2) (2026-08-14)

### Bug Fixes

* drop selfIdentity from the group member listing ([#98](https://github.com/calimero-network/mero-js/issues/98)) ([bf61155](https://github.com/calimero-network/mero-js/commit/bf611555544e1ead5461554aac054936ce6a83d0))

## [9.0.1](https://github.com/calimero-network/mero-js/compare/mero-js-v9.0.0...mero-js-v9.0.1) (2026-08-12)

### Bug Fixes

* **admin-api:** declare inviter_account, and address members by account in e2e ([#81](https://github.com/calimero-network/mero-js/issues/81)) ([4503845](https://github.com/calimero-network/mero-js/commit/45038451ea05ce92624af99858326bed6e6cfc74)), closes [core#3391](https://github.com/calimero-network/core/issues/3391) [core#3393](https://github.com/calimero-network/core/issues/3393)

## [9.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v8.0.0...mero-js-v9.0.0) (2026-08-11)

### ⚠ BREAKING CHANGES

* **admin-api:** CreateNamespaceRequest.upgradePolicy,
CreateGroupRequest.upgradePolicy and the UpgradePolicy type are removed.
Namespace.upgradePolicy and GroupInfo.upgradePolicy are deprecated.

### Features

* **admin-api:** drop upgradePolicy from the create requests ([#83](https://github.com/calimero-network/mero-js/issues/83)) ([2c3c835](https://github.com/calimero-network/mero-js/commit/2c3c835d16e853c4fdf543d43f626b37be792831))

## [8.0.0](https://github.com/calimero-network/mero-js/compare/mero-js-v7.3.2...mero-js-v8.0.0) (2026-08-11)

### ⚠ BREAKING CHANGES

* **admin-api:** UpdateGroupSettingsRequest, UpdateGroupSettingsResponseData,
and AdminClient.updateGroupSettings() are removed. The only field this
endpoint ever set was upgradePolicy, which no longer exists.

* feat(admin-api)!: drop residueIdentity from MemberMigrationReport
* **admin-api:** MemberMigrationReport.residueIdentity is removed. It was
hardwired to 0 in production because the wasm host exposes no committed-state
key iteration to compute it for real. authoredRemaining measures the same
fact, is self-reported, and works.

* docs(admin-api): correct toSchemaVersion doc comment

It documents the ABI state version from #[app::state(version = N)], not a
CRDT concept. The wrong wording is how the core rollup defect (comparing
bundle-semver majors instead of state versions) went unnoticed.

### Features

* **admin-api:** drop UpgradePolicy, updateGroupSettings, and residueIdentity ([#82](https://github.com/calimero-network/mero-js/issues/82)) ([a9a74e8](https://github.com/calimero-network/mero-js/commit/a9a74e8bdbe8c5fa2ab43d471c14119a885679b1))

## <small>7.3.2 (2026-07-30)</small>

* fix(types): match core's wire format and surface node errors (#79) ([ab535b1](https://github.com/calimero-network/mero-js/commit/ab535b1)), closes [#79](https://github.com/calimero-network/mero-js/issues/79)


### BREAKING CHANGE

* `AliasEntry` is removed and `ListAliasesResponseData` is now
`Record<string, string>`; `SignedGroupOpenInvitation` /
`GroupInvitationFromAdmin` use core's snake_case keys and are opaque; the
optional-on-the-wire fields of `Application`, `Context` and `Namespace` are
declared optional. Code written against the old declarations did not work
against a real node.

* fix(admin): build a real HTTP client in the admin factories

`createAdminApiClient`, `createNodeAdminApiClient` and
`createBrowserAdminApiClient` wrapped a stub whose every method threw, so the
three obvious entry points only worked for a caller who already knew to reach
for `createAdminApiClientFromHttpClient`. The transports they needed already
exist in `http-client`, so wire each to its counterpart rather than removing
public API that consumers may already import.

* feat(config): let apps react to a revoked credential via onAuthRevoked

The transport has always had the hook and `MeroJs` used it to clear its tokens,
but `MeroJsConfig` did not expose it — so an app could not tell the difference
between "logged out" and "still logged in" after the node revokes a token
family, and had no place to start a re-login. Clear first, then notify, and
swallow whatever the callback throws so a UI failure cannot mask the auth error.

`refreshToken` / `onTokenRefresh` are deliberately not exposed: MeroJs owns the
single-flight, cross-tab-locked rotation that single-use refresh tokens require,
and a `tokenStore` already observes every rotation.

* test(contract): check a live node's responses against the declared types

Items in this branch's first two commits shared one root cause: the
hand-written types drift from core's wire and nothing notices until a consumer
hits it. This closes that loop for the methods most exposed to it.

The suite provisions real resources on a booted merod and, per covered method,
asserts that every key the node sends is declared and every key the SDK
requires is present. Covered: listApplications, getContexts, listNamespaces,
createGroupInvitation (response, signed blob, and signed inner payload), and
the three alias listings. Adding one is a single SPECS entry.

Two gates, verified to fail independently: the runtime run catches a wire key
the SDK does not model (confirmed by dropping `Namespace.appVersion`), while
`typecheck:contract` — now in CI and prepublish, and extended to cover
tests/ — catches a declaration that drifts from its spec (confirmed by
reverting the alias map to a list of entries, and the invitation keys to
camelCase). Neither alone is enough: the field names in a spec are typed
`keyof T`, so a rename has to break one or the other.

* chore: stop emitting dangling sourcemaps and drop the prettier scripts

`declarationMap`/`sourceMap` made tsc write 8 maps whose `sources` point at
`../src`, which the published package (`files: ["dist"]`) never contains — every
one of them dead on arrival. The esbuild bundles that `exports` actually points
at inline `sourcesContent`, so they keep working.

`pnpm prettier` could never run: prettier is not a dependency and the repo has
no prettier config. Removing the two scripts is honest; adding the tool would
reformat a codebase that has never been prettier-formatted.

## <small>7.3.1 (2026-07-29)</small>

* fix(types): emit NodeNext-resolvable specifiers in published declarations (#78) ([dc4e735](https://github.com/calimero-network/mero-js/commit/dc4e735)), closes [#78](https://github.com/calimero-network/mero-js/issues/78)

## 7.3.0 (2026-07-29)

* feat(utils): add local node discovery helpers (#77) ([4ca6a95](https://github.com/calimero-network/mero-js/commit/4ca6a95)), closes [#77](https://github.com/calimero-network/mero-js/issues/77)

## 7.2.0 (2026-07-29)

* feat(admin): add getApplicationAbi (#76) ([c05ee81](https://github.com/calimero-network/mero-js/commit/c05ee81)), closes [#76](https://github.com/calimero-network/mero-js/issues/76)

## <small>7.1.1 (2026-07-28)</small>

* fix(e2e): give the kv-store fixture real artifact hashes (#75) ([cd460a4](https://github.com/calimero-network/mero-js/commit/cd460a4)), closes [#75](https://github.com/calimero-network/mero-js/issues/75)

## 7.1.0 (2026-07-24)

* feat(events): add group-membership event support (#73) ([606dfbf](https://github.com/calimero-network/mero-js/commit/606dfbf)), closes [#73](https://github.com/calimero-network/mero-js/issues/73)

## <small>7.0.5 (2026-07-24)</small>

* fix(admin): SemVer 2.0 compareSemver, hardened registry fetches, forceCodeOnly passthrough (#72) ([0924ea6](https://github.com/calimero-network/mero-js/commit/0924ea6)), closes [#72](https://github.com/calimero-network/mero-js/issues/72)
* ci: bump actions to Node 24 runtimes (#71) ([6b695fc](https://github.com/calimero-network/mero-js/commit/6b695fc)), closes [#71](https://github.com/calimero-network/mero-js/issues/71)

## <small>7.0.4 (2026-07-23)</small>

* docs: fix factual bugs, add missing reference pages, close depth gaps (#70) ([fd5d5a3](https://github.com/calimero-network/mero-js/commit/fd5d5a3)), closes [#70](https://github.com/calimero-network/mero-js/issues/70)

## <small>7.0.3 (2026-07-22)</small>

* docs: migrate to Astro Starlight docs site with animated diagrams (#69) ([8d2bf86](https://github.com/calimero-network/mero-js/commit/8d2bf86)), closes [#69](https://github.com/calimero-network/mero-js/issues/69) [#a5ff11](https://github.com/calimero-network/mero-js/issues/a5ff11)

## <small>7.0.2 (2026-07-20)</small>

* fix(auth): adapt e2e/CI to core rc.17 admin-creds-at-init (#68) ([4c9f619](https://github.com/calimero-network/mero-js/commit/4c9f619)), closes [#68](https://github.com/calimero-network/mero-js/issues/68) [core#3221](https://github.com/core/issues/3221) [core#3081](https://github.com/core/issues/3081) [calimero-network/core#3276](https://github.com/calimero-network/core/issues/3276)

## <small>7.0.1 (2026-07-15)</small>

* fix(auth): make token refresh single-use safe (rotation, single-flight, token_reuse) (#67) ([bdf4609](https://github.com/calimero-network/mero-js/commit/bdf4609)), closes [#67](https://github.com/calimero-network/mero-js/issues/67) [calimero-network/core#3083](https://github.com/calimero-network/core/issues/3083) [calimero-network/core#3229](https://github.com/calimero-network/core/issues/3229)

## 7.0.0 (2026-06-28)

* fix!: metadata getters return the full MetadataRecord (#65) ([6daa627](https://github.com/calimero-network/mero-js/commit/6daa627)), closes [#65](https://github.com/calimero-network/mero-js/issues/65)
* test(e2e): true round-trip + multi-node assertions (Tiers 1–3) (#63) ([2a351e1](https://github.com/calimero-network/mero-js/commit/2a351e1)), closes [#63](https://github.com/calimero-network/mero-js/issues/63)


### BREAKING CHANGE

* getGroupMetadata/getMemberMetadata/getContextMetadata now
resolve to the full MetadataRecord ({ name, data, updatedAt, updatedBy })
instead of the bare data map. Read the values under `.data`.

Co-authored-by: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

## 6.1.0 (2026-06-27)

* feat(sdk): add getBlobInfo (HEAD /blobs/:id) (#62) ([7d440ee](https://github.com/calimero-network/mero-js/commit/7d440ee)), closes [#62](https://github.com/calimero-network/mero-js/issues/62)
* test(e2e): method-aware coverage recorder + getTeeAdmissionPolicy (pairs core #2960) (#61) ([ccae44b](https://github.com/calimero-network/mero-js/commit/ccae44b)), closes [#2960](https://github.com/calimero-network/mero-js/issues/2960) [#61](https://github.com/calimero-network/mero-js/issues/61)

## <small>6.0.4 (2026-06-26)</small>

* fix(sdk): WsClient type parity + getBlob raw bytes + resync empty-body (#60) ([b112aca](https://github.com/calimero-network/mero-js/commit/b112aca)), closes [#60](https://github.com/calimero-network/mero-js/issues/60)
* ci: run e2e against released merod on every PR (replaces #25) (#59) ([22b298e](https://github.com/calimero-network/mero-js/commit/22b298e)), closes [#25](https://github.com/calimero-network/mero-js/issues/25) [#59](https://github.com/calimero-network/mero-js/issues/59)

## <small>6.0.3 (2026-06-25)</small>

* chore(deps-dev): bump vitest from 2.1.9 to 3.2.6 (#43) ([e3a0b01](https://github.com/calimero-network/mero-js/commit/e3a0b01)), closes [#43](https://github.com/calimero-network/mero-js/issues/43)

## <small>6.0.2 (2026-06-25)</small>

* chore(deps-dev): bump esbuild from 0.25.12 to 0.28.1 (#48) ([9f29edd](https://github.com/calimero-network/mero-js/commit/9f29edd)), closes [#48](https://github.com/calimero-network/mero-js/issues/48)

## <small>6.0.1 (2026-06-25)</small>

* docs(pr): add wire-contract (core gate) PR template (#58) ([1cd4c18](https://github.com/calimero-network/mero-js/commit/1cd4c18)), closes [#58](https://github.com/calimero-network/mero-js/issues/58) [#56](https://github.com/calimero-network/mero-js/issues/56)
* test(e2e): full-flow e2e coverage + recorder; fix 8 SDK wire bugs found live (#57) ([dbe404b](https://github.com/calimero-network/mero-js/commit/dbe404b)), closes [#57](https://github.com/calimero-network/mero-js/issues/57)

## 6.0.0 (2026-06-23)

* fix(admin)!: align group reparent + createContext label with core wire contract (#53) ([269c412](https://github.com/calimero-network/mero-js/commit/269c412)), closes [#53](https://github.com/calimero-network/mero-js/issues/53)
* test(auth): cover success:false payloads in revokeTokens/createRootKey (#54) ([100828a](https://github.com/calimero-network/mero-js/commit/100828a)), closes [#54](https://github.com/calimero-network/mero-js/issues/54)


### BREAKING CHANGE

* `AdminApiClient.nestGroup`/`unnestGroup` are removed in favor of
`reparentGroup`; `CreateContextRequest.groupName` is renamed to `name`. Downstream
`mero-react` exposes `useNestGroup`/`useUnnestGroup` hooks that need a
corresponding `useReparentGroup` (those hooks already 404 against current core).

Verified: 219 unit tests pass (2 new reparent tests RED→green), typecheck, lint,
build all clean.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>

* style(admin): trim verbose explainer comments on reparent/createContext

Reduce the narrative comments (wire path/body, "core renamed X") to terse
one-liners; the contract lives in the PR/commit, not inline.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>

## 5.0.0 (2026-06-23)

* fix(auth)!: align auth-api key/token/health methods with core wire contract (#52) ([bc1ceda](https://github.com/calimero-network/mero-js/commit/bc1ceda)), closes [#52](https://github.com/calimero-network/mero-js/issues/52) [#51](https://github.com/calimero-network/mero-js/issues/51)


### BREAKING CHANGE

* AuthApiClient.getChallenge() and isAuthed() removed (use
AdminApiClient.isAuthed()). Changed request/response types: RevokeTokenRequest
({client_id}), CreateKeyRequest/CreateKeyResponse, GenerateClientKeyRequest,
MockTokenRequest (snake_case); listRootKeys/listClientKeys now return
RootKey[]/ClientKey[] (RootKeysResponse/ClientKeysResponse, ChallengeResponse and
AuthStatus types removed); HealthResponse/IdentityResponse field changes.

Co-authored-by: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

## 4.0.0 (2026-06-22)

* fix(admin,auth)!: correct 4 wire-contract drifts vs core (alias, blob, context hash, key permissions ([4ad230a](https://github.com/calimero-network/mero-js/commit/4ad230a)), closes [#51](https://github.com/calimero-network/mero-js/issues/51)


### BREAKING CHANGE

* CreateAliasRequest is replaced by CreateContextAliasRequest /
CreateApplicationAliasRequest / CreateContextIdentityAliasRequest;
UploadBlobRequest gains optional hash/contextId; Context.rootHash is renamed to
Context.contextStateHash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* fix(auth)!: send { add, remove } delta for updateKeyPermissions

updateKeyPermissions sent `{ permissions }`, which core ignores — the call was a
silent no-op that echoed the old permissions back with 200. Core expects an
{ add, remove } delta (remove applied first, then add). The method now takes an
UpdateKeyPermissionsRequest delta and sends the correct body. Adds a
body-asserting test (the first guard on this endpoint's request shape).
* updateKeyPermissions(keyId, permissions: string[]) is now
updateKeyPermissions(keyId, { add?, remove? }).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

* refactor(admin): drop unsafe BodyInit cast in uploadBlob; cover ArrayBuffer path

Addresses meroreviewer warnings on uploadBlob:
- Pass request.data (Uint8Array | ArrayBuffer | Blob) directly as the body. All
  three are valid BodyInit, so the `as BodyInit` cast (which silenced type
  errors) is removed and the type is now compiler-enforced. fetch honors a
  Uint8Array view's byteOffset/byteLength, so the manual buffer slice (and its
  SharedArrayBuffer edge) is no longer needed.
- Add a test exercising the ArrayBuffer body path (the Blob path is the same
  verbatim pass-through), asserting the exact body object is streamed.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

## <small>3.0.1 (2026-06-17)</small>

* fix(admin): correct migrations-v2 response envelopes + AppVersionChanged event (#50) ([1038bb2](https://github.com/calimero-network/mero-js/commit/1038bb2)), closes [#50](https://github.com/calimero-network/mero-js/issues/50) [calimero-network/core#2773](https://github.com/calimero-network/core/issues/2773)

## 3.0.0 (2026-06-15)

* feat(admin)!: align admin SDK with migrations-v2 core API (#49) ([1766888](https://github.com/calimero-network/mero-js/commit/1766888)), closes [#49](https://github.com/calimero-network/mero-js/issues/49)


### BREAKING CHANGE

* `migrateMethod` is removed from `UpdateContextApplicationRequest`
and `UpgradeGroupRequest`. Callers must stop passing it; the node derives the
migrate method from the bundle ABI.

Co-authored-by: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

## <small>2.5.1 (2026-06-10)</small>

* fix(admin): tolerate a bare-null body in the metadata getters (#47) ([8ef7cfc](https://github.com/calimero-network/mero-js/commit/8ef7cfc)), closes [#47](https://github.com/calimero-network/mero-js/issues/47) [#36](https://github.com/calimero-network/mero-js/issues/36) [#45](https://github.com/calimero-network/mero-js/issues/45)

## 2.5.0 (2026-06-10)

* feat(admin): cascade flag on UpgradeGroupRequest (#46) ([86c9fa6](https://github.com/calimero-network/mero-js/commit/86c9fa6)), closes [#46](https://github.com/calimero-network/mero-js/issues/46)

## <small>2.4.1 (2026-06-09)</small>

* fix(admin): tolerate null metadata payload in metadata getters (#45) ([4491675](https://github.com/calimero-network/mero-js/commit/4491675)), closes [#45](https://github.com/calimero-network/mero-js/issues/45)

## 2.4.0 (2026-06-09)

* feat(admin): failed migration state + BundleMigration type + installFromRegistry (#2539) (#44) ([e72a3a5](https://github.com/calimero-network/mero-js/commit/e72a3a5)), closes [#2539](https://github.com/calimero-network/mero-js/issues/2539) [#44](https://github.com/calimero-network/mero-js/issues/44) [#2539](https://github.com/calimero-network/mero-js/issues/2539) [#2539](https://github.com/calimero-network/mero-js/issues/2539)

## 2.3.0 (2026-06-08)

* feat: migration-UX SDK surfaces — migration/cascade status, typed SSE, migrateMyEntries (6g, #2539)  ([3b1cc18](https://github.com/calimero-network/mero-js/commit/3b1cc18)), closes [#2539](https://github.com/calimero-network/mero-js/issues/2539) [#42](https://github.com/calimero-network/mero-js/issues/42)

## <small>2.2.1 (2026-05-22)</small>

* fix(admin): listSubgroups reads `subgroups` field from server response (#38) ([8772557](https://github.com/calimero-network/mero-js/commit/8772557)), closes [#38](https://github.com/calimero-network/mero-js/issues/38)

## 2.2.0 (2026-05-15)

* feat(admin): add joinSubgroupInheritance method (#37) ([7479e2c](https://github.com/calimero-network/mero-js/commit/7479e2c)), closes [#37](https://github.com/calimero-network/mero-js/issues/37) [calimero-network/core#2357](https://github.com/calimero-network/core/issues/2357)

## 2.1.0 (2026-05-12)

* feat: generic metadata records + capability constants (#35) ([d19bb64](https://github.com/calimero-network/mero-js/commit/d19bb64)), closes [#35](https://github.com/calimero-network/mero-js/issues/35) [#2338](https://github.com/calimero-network/mero-js/issues/2338)

## <small>2.0.1 (2026-05-09)</small>

* fix(admin): align listGroupMembers response shape with merod (#34) ([29be1b5](https://github.com/calimero-network/mero-js/commit/29be1b5)), closes [#34](https://github.com/calimero-network/mero-js/issues/34)

## 2.0.0 (2026-04-27)

* feat(admin)!: rename DefaultVisibility → SubgroupVisibility to match core (#33) ([ecd2101](https://github.com/calimero-network/mero-js/commit/ecd2101)), closes [#33](https://github.com/calimero-network/mero-js/issues/33) [calimero-network/core#2261](https://github.com/calimero-network/core/issues/2261) [#33](https://github.com/calimero-network/mero-js/issues/33)


### BREAKING CHANGE

* setDefaultVisibility, SetDefaultVisibilityRequest,
SetDefaultVisibilityResponseData, and GroupInfo.defaultVisibility have
been renamed to setSubgroupVisibility, SetSubgroupVisibilityRequest,
SetSubgroupVisibilityResponseData, and GroupInfo.subgroupVisibility
respectively. The HTTP route moved to /settings/subgroup-visibility and
the request body field is now subgroupVisibility.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>

* test(admin): cover SetSubgroupVisibilityRequest.requester forwarding

## <small>1.4.1 (2026-04-16)</small>

* fix: cleanup cards ([928326c](https://github.com/calimero-network/mero-js/commit/928326c))
* Add simple docs (#31) ([75b4a7a](https://github.com/calimero-network/mero-js/commit/75b4a7a)), closes [#31](https://github.com/calimero-network/mero-js/issues/31)
* Create static.yml ([0bb6bf9](https://github.com/calimero-network/mero-js/commit/0bb6bf9))

## 1.4.0 (2026-04-07)

* feat: align mero-js with core namespace model (#28) ([fb4b2ae](https://github.com/calimero-network/mero-js/commit/fb4b2ae)), closes [#28](https://github.com/calimero-network/mero-js/issues/28) [#29](https://github.com/calimero-network/mero-js/issues/29)

## 1.3.0 (2026-04-01)

* feat: add CloudClient with enableHA/disableHA methods (#27) ([8a4ae41](https://github.com/calimero-network/mero-js/commit/8a4ae41)), closes [#27](https://github.com/calimero-network/mero-js/issues/27)

## <small>1.2.1 (2026-03-30)</small>

* fix: skip npm publish when no new release is created (#24) ([424bba7](https://github.com/calimero-network/mero-js/commit/424bba7)), closes [#24](https://github.com/calimero-network/mero-js/issues/24)
* ci: comment out E2E tests until core releases X-Auth-Error fix (#23) ([adf2e9b](https://github.com/calimero-network/mero-js/commit/adf2e9b)), closes [#23](https://github.com/calimero-network/mero-js/issues/23)

## 1.2.0 (2026-03-30)

* feat: complete SDK — TokenStore, auth callback, RPC, SSE, WebSocket (#22) ([ba62a19](https://github.com/calimero-network/mero-js/commit/ba62a19)), closes [#22](https://github.com/calimero-network/mero-js/issues/22)

## 1.1.0 (2026-01-23)

* feat: add automatic token refresh and remove Tauri workarounds (#19) ([08761e9](https://github.com/calimero-network/mero-js/commit/08761e9)), closes [#19](https://github.com/calimero-network/mero-js/issues/19)

## <small>1.0.2 (2026-01-22)</small>

* fix: use node 24 for better npm OIDC support ([3df75fc](https://github.com/calimero-network/mero-js/commit/3df75fc))

## <small>1.0.1 (2026-01-22)</small>

* fix: publish to npm directly in workflow for OIDC support ([eba1ee2](https://github.com/calimero-network/mero-js/commit/eba1ee2))
* fix: use @semantic-release/npm plugin for OIDC support (#18) ([3dcf4de](https://github.com/calimero-network/mero-js/commit/3dcf4de)), closes [#18](https://github.com/calimero-network/mero-js/issues/18)

## 1.0.0 (2026-01-22)

* fix: add --provenance flag to npm publish for OIDC trusted publishing (#16) ([e7bbf13](https://github.com/calimero-network/mero-js/commit/e7bbf13)), closes [#16](https://github.com/calimero-network/mero-js/issues/16)
* fix: add missing conventional-changelog-conventionalcommits dependency (#15) ([2e9efed](https://github.com/calimero-network/mero-js/commit/2e9efed)), closes [#15](https://github.com/calimero-network/mero-js/issues/15)
* fix: add semantic-release plugins as dev dependencies ([6195a04](https://github.com/calimero-network/mero-js/commit/6195a04))
* fix: configure CI to run on feature branches and PRs only ([2a08a8c](https://github.com/calimero-network/mero-js/commit/2a08a8c))
* fix: ensure E2E tests only run on PRs to main/master ([c82cac0](https://github.com/calimero-network/mero-js/commit/c82cac0))
* fix: make npm version command idempotent (#17) ([10e47cd](https://github.com/calimero-network/mero-js/commit/10e47cd)), closes [#17](https://github.com/calimero-network/mero-js/issues/17)
* fix: remove explicit pnpm version from CI workflow ([ac5b998](https://github.com/calimero-network/mero-js/commit/ac5b998))
* fix: run CI on all branches including main/master ([004f7bf](https://github.com/calimero-network/mero-js/commit/004f7bf))
* fix: run unit tests only in release workflow ([62131e3](https://github.com/calimero-network/mero-js/commit/62131e3))
* fix: use @semantic-release/exec for OIDC trusted publishing (#13) ([13a7c9f](https://github.com/calimero-network/mero-js/commit/13a7c9f)), closes [#13](https://github.com/calimero-network/mero-js/issues/13)
* fix: use pnpm exec for semantic-release in CI ([7bcc042](https://github.com/calimero-network/mero-js/commit/7bcc042))
* fix: wrap fetch calls to prevent illegal invocation error (#11) ([0375a00](https://github.com/calimero-network/mero-js/commit/0375a00)), closes [#11](https://github.com/calimero-network/mero-js/issues/11)
* chore: remove main branch references (#10) ([637eb1e](https://github.com/calimero-network/mero-js/commit/637eb1e)), closes [#10](https://github.com/calimero-network/mero-js/issues/10)
* chore: update package.json ([afc3e1d](https://github.com/calimero-network/mero-js/commit/afc3e1d))
* chore: use master branch (#12) ([4e54ff3](https://github.com/calimero-network/mero-js/commit/4e54ff3)), closes [#12](https://github.com/calimero-network/mero-js/issues/12)
* chore(release): 1.0.0 [skip ci] ([3fd0176](https://github.com/calimero-network/mero-js/commit/3fd0176))
* Merge setup/ci-config: production-ready CI/CD setup ([daaeae5](https://github.com/calimero-network/mero-js/commit/daaeae5))
* feat: production-ready mero-js SDK ([b2cf046](https://github.com/calimero-network/mero-js/commit/b2cf046))
* feat: professional CI workflow setup ([c2281f7](https://github.com/calimero-network/mero-js/commit/c2281f7))

## 1.0.0 (2026-01-22)

* fix: add missing conventional-changelog-conventionalcommits dependency (#15) ([2e9efed](https://github.com/calimero-network/mero-js/commit/2e9efed)), closes [#15](https://github.com/calimero-network/mero-js/issues/15)
* fix: add semantic-release plugins as dev dependencies ([6195a04](https://github.com/calimero-network/mero-js/commit/6195a04))
* fix: configure CI to run on feature branches and PRs only ([2a08a8c](https://github.com/calimero-network/mero-js/commit/2a08a8c))
* fix: ensure E2E tests only run on PRs to main/master ([c82cac0](https://github.com/calimero-network/mero-js/commit/c82cac0))
* fix: remove explicit pnpm version from CI workflow ([ac5b998](https://github.com/calimero-network/mero-js/commit/ac5b998))
* fix: run CI on all branches including main/master ([004f7bf](https://github.com/calimero-network/mero-js/commit/004f7bf))
* fix: run unit tests only in release workflow ([62131e3](https://github.com/calimero-network/mero-js/commit/62131e3))
* fix: use @semantic-release/exec for OIDC trusted publishing (#13) ([13a7c9f](https://github.com/calimero-network/mero-js/commit/13a7c9f)), closes [#13](https://github.com/calimero-network/mero-js/issues/13)
* fix: use pnpm exec for semantic-release in CI ([7bcc042](https://github.com/calimero-network/mero-js/commit/7bcc042))
* fix: wrap fetch calls to prevent illegal invocation error (#11) ([0375a00](https://github.com/calimero-network/mero-js/commit/0375a00)), closes [#11](https://github.com/calimero-network/mero-js/issues/11)
* chore: remove main branch references (#10) ([637eb1e](https://github.com/calimero-network/mero-js/commit/637eb1e)), closes [#10](https://github.com/calimero-network/mero-js/issues/10)
* chore: update package.json ([afc3e1d](https://github.com/calimero-network/mero-js/commit/afc3e1d))
* chore: use master branch (#12) ([4e54ff3](https://github.com/calimero-network/mero-js/commit/4e54ff3)), closes [#12](https://github.com/calimero-network/mero-js/issues/12)
* Merge setup/ci-config: production-ready CI/CD setup ([daaeae5](https://github.com/calimero-network/mero-js/commit/daaeae5))
* feat: production-ready mero-js SDK ([b2cf046](https://github.com/calimero-network/mero-js/commit/b2cf046))
* feat: professional CI workflow setup ([c2281f7](https://github.com/calimero-network/mero-js/commit/c2281f7))

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial release of @calimero-network/mero-js
- Pure JavaScript SDK for Calimero using Web Standards
- HTTP client with retry logic and signal composition
- Auth API client
- Admin API client
- Zero dependencies - uses native Web APIs only
