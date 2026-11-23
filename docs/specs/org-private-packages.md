# Org-private packages: design

Nov 23, 2026 · Status: core C1 to C5 implemented; C6, app-registry and mdma not yet.
Spans core, app-registry and mdma.

## The rule

A package an organization publishes as private is used only by that organization, on any cluster it shares with others.

On a cluster a release is one of three kinds, and core records which:

| Kind | Who may use it |
| --- | --- |
| Operator catalog (`installed_by` is `OPERATOR`) | everyone, as today |
| Public registry release | everyone, once installed |
| Private release (a private registry package, or an upload) | members of the organizations it was installed for |

Two rights are judged apart:

- **Use** a release: list it, read its versions, pipelines and capability card, install it, create an instance of it, upgrade to it, offer it in a deployment, name it as a pipeline module or a unit member.
  For a private release: only a member of an organization it was installed for (an installer too only while such a member), its installing accounts for one installed for no organization, and the operator for its own installs.
- **Read the facts** of the release an instance runs (its name and version in reports and update notices, the instance's ABI): whoever reaches that instance, as today.
  An organization that makes an instance reachable (grants, guests, public links, a deployment's customers) shares that instance, never the release.
  The release an instance runs is the package and version its record names; another package shipping the same WASM at that version is not it.
  An update shown to such a reader leaves out the releases a unit's members would move to unless they may use them, as those are chosen for the instance's owner.

Today `SystemState::sees_release` is one rule for both: any installer, anyone sharing any org with an installer, anyone reaching an app running it, and (in the package routes) any session bound to the package's frontend.

## Exposures on master (8a2154551)

Reproduced by tests that assert the secure behaviour and fail today (branch `test/private-package-repro-master`, 57fe56e99):

1. **A frontend-bound session opens any package.**
   Any account signs in on `https://{app_host::label(package)}.{apps_domain}` (a label works even for a package with no frontend) and gets a session bound to that package.
   `api/packages.rs` (`package_json`, pipelines, capabilities) and `resolve_release_parts` take `principal.bound_app() == Some(package)` in place of `sees_release`, so that session lists the versions, reads pipelines and the full ABI, and creates and runs its own instance.
   Test: `a_bound_session_does_not_open_a_package_its_account_cannot_see` (`tests/cluster.rs`).
2. **Any deployer installs another org's private package from the registry.**
   `install_from_registry` fetches with the cluster's credential alone; app-registry asks mdma (`POST /internal/registry/authorize`), which allows per cluster (`orgs.entitled(org, cluster)`), never per requesting account.
   Test: `another_orgs_account_cannot_install_a_private_package_from_the_registry` (`tests/deployers.rs`); its registry serves anyone, so it proves core's half, and the mdma half is from the code.
   In production only the missing per-cluster credential blocks this (mdma #814 mints one and is held for this fix).

Read in code, not reproduced yet:

3. **Org overlap.** `sees_release`'s `shares_org` admits anyone sharing any organization with any installer.
4. **Reach is use.** Anyone who reaches an app running a release may create their own instances of it.
5. **Unit members ignore visibility.** `api::apps::unit_members` resolves a `uses: { mode: create }` reference against every installed release of the package, so an account's own package naming another org's private package gets an instance of it.

Not exposed (verified): another org reading the victim's own instance, its state or files; any account route serving raw WASM or `.mpk` bytes.

## Fixes

### Core (one release; stacked PRs)

- **C1. A bound session sees what its account sees.**
  Drop the `bound_app() == Some(package)` grant from `package_json`, the pipelines and capabilities routes, `resolve_release_parts` (`frontend`), `bundle_named`, the consent page's `requested_release` and `app_update`'s `sees`.
- **C2. An install names its organization.**
  `POST /packages/install { package, version, org }` and `POST /apps { package }` (which installs first, into the instance's org) record `PackageVersion::orgs`, added to on each install for another org.
  The caller is a member of `org` (judged again at apply); `org` is required when the caller belongs to more than one.
  A caller in no organization installs for none: the release is recorded private with no `orgs` and is its installing accounts' alone, so development flows (`meroctl local dev`, pushes) keep working; an install for an organization later adds it, and the organization rule then holds.
  A member of the package's Cloud org may install it for any cluster org they belong to: nothing maps Cloud orgs to cluster orgs, and that is the member's own deliberate share.
- **C3. Kind and the two rights.**
  `InstallPackage` records `PackageVersion::visibility: public | private`.
  Core learns it from the registry itself: the install first fetches without a credential, and a release served to anyone is public; else (a `404`, or a `401`/`403` asking for a sign-in) it fetches with the credential and the account (C4) and records private. An upload is private.
  `sees_release` splits into the two rights above; `shares_org` goes, and the reach clause stays only in the read-the-facts rule (`report::named_release`, `app_update`).
  A record without `visibility` keeps today's rule; no cluster holds a private registry install yet, and none gets a credential before this release (mdma, below).
  `Feature::PrivateReleases` gates the new fields; a `DeepCase` covers a release installed before and after.
  Everything that picks a release by its version follows the use right: an update on its own moves an app only to a release its owner may use; an app team is named after, and a package named for one, an upgrade key or an outbound approval is found from, the newest release the actor may use (an outbound approval: reads); a package with none answers as a missing one.
  A first approval of an update and a unit's upgrade name only a target their account may use (or, for an instance from a deployment, the offer that serves it); any other answers as a missing version, before anything of it is read.
  A deployment's offer serves only while someone who acts for its owner may use the release: for a create, any upgrade (one app, an organization's, the operator's or a unit's run), an update on its own and the update an instance is shown.
  A deployment's secrets reach a release its offers use only while someone who acts for its owner may use that release.
  An install again that would leave its installer a release it may not use is refused before it is logged.
  A registry install is held to the deadline of the package's publisher only for an installer that may use the package; anyone else is judged as for a package not installed.
  A route reads a release a client names only through the state's lookups for the two rights (`release_used` and its kin), and a gate test (`tests/release_lookups.rs`) fails on any other read of the package table, or of the release a bundle record names, under `api/` and `consent_page`.
  Apply judges the rules it can from the logged version that brings `Feature::PrivateReleases`, so earlier entries replay as they applied.
- **C4. The registry learns who installs.**
  A private install's fetch sends the installing account (`x-calimero-install-account: <account id>`) with the cluster credential; `calimero-app-downloader`'s `HttpRegistry` takes the header.
  A re-fetch of a release the log already records (`release_mpk`: other nodes, `release_wasm`, frontends) sends none and is pinned by the recorded hash.
- **C5. Unit members follow the creator.**
  `unit_members` picks only releases the creator may use, ones signed by the same signer as the instance's release (a publisher composing its own packages), or, for an instance from a deployment, ones that deployment offers.
  The rule is the same for every unit, from a deployment or not, and for a unit's preview and upgrade; the member's create runs exactly the release it chose.
  A pipeline module is pinned at install only from a release the installer may use that is public or the operator's, of the installing package's own signer, or, for a private installing release, private to every organization it is for (`SystemState::may_pin`); an install again of that release keeps its pins and is refused `UNKNOWN_MODULE` when a pin no longer meets the rule for the organization it adds.
- **C6. Signing in on a private package's frontend is gated. Deferred: the next step after this release, with members-only frontend files.**
  The node does not get an invite at sign-in today, so admitting invite holders needs an optional `invite` on `login/finish` and mero-js and mero-react sending it; until then C1 leaves a stranger's session with nothing to see.
  At sign-in, sign-up finish and refresh on a package's origin, a private release's origin opens a session only for an account that may use it, reaches an instance running it, or carries an unredeemed invite to its org or to such an instance.
  A package a deployment offers keeps an open door (the offer is public, and a new customer signs in before any instance exists), as do public and catalog packages.
  A refusal answers as an unknown label does, so it does not say the package exists.

### app-registry

- **R1.** Pass `x-calimero-install-account` to mdma's authorize as `account`, and put it in the authorize cache key (`cloud-authorize.js` `cacheKey`).

### mdma

- **M1.** With `account`: allowed only if `DeveloperNodeAccount(cluster, account)` is verified and that developer is a member of the package's owning Cloud org, besides today's `entitled`; record `(cluster, package, version)` as authorized.
  An account not linked to a developer is refused, with a message to link it in the console.
- **M2.** Without `account`: allowed only for a `(cluster, package, version)` M1 authorized, or a product package mdma itself installs as operator (#818, a deliberate exception). No setting: no cluster holds a credential from before.
- **M3.** mdma operator-installs only product packages.
- **M4 (#814).** A cluster gets its registry credential only when it runs the core release with C2 to C4; an earlier core sends no account, and M2 would refuse its installs anyway.

## Rollout

1. Core C1 to C5, released (C6 follows).
2. app-registry R1 and mdma M1 to M3, deployed (independent of 1: authorize judges `account` when sent).
3. mdma #814 as M4, merged once 1 and 2 are out.

## Tests

- Core: the two repro tests turn green; red-first tests for exposures 3, 4 and 5 (5 also through a deployment's offer) and for C6 (member, guest, invite holder, deployment customer, stranger); a stranger's frontend create of a public registry package still works; a customer of a deployment still reads its instance's release name; a release from before keeps today's rule (`DeepCase`); an install for org B by a member of A and B is used in B only; an installer who left the org loses it; a private install for no organization is its installers' alone until an install for an organization adds one.
- app-registry: authorize forwards `account` and caches per account.
- mdma: authorize with a verified member, an unverified account, a non-member, no account before and after M1 recorded it, a product package; a cluster below the core release gets no credential.

## Accepted for now

- A private app's frontend files stay public by name (the page loads before sign-in); members-only files are a later phase.
- mdma trusts the account named under the cluster credential; a node-signed proof would remove that trust.
- A package made private after a cluster installed it as public stays public there.
- Receipts and run results stay limited to those who handle the app's files.
- `/.well-known/calimero.json` on a package's frontend host lists its versions; it goes with the sign-in gate (C6).
- A frontend-bound session's `POST /apps` for a package that is not installed answers `403`, which tells it apart from a private one; also with C6.
- On a `--dev` cluster, uploading under a taken package name answers the signer-pin message, naming the pinned signer.
- A registry credential placed anywhere but the registry URL's userinfo is not left out of the anonymous first fetch.
