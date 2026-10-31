# Deployments owned by an organization: design proposal

Oct 31, 2026 · Status: **proposal, not implemented** · No runtime code changes with this document.

A developer deploys apps from Calimero Cloud: their releases are installed on a
cluster, and instances of them are created later, mostly from the app's own
frontend, one per customer organization. Several developers can deploy the same
package on the same shared cluster. Today the cluster records a release once,
owned by whichever account installed it first. This document adds a
**deployment**: the deployer's list of apps on a cluster and the team that
manages it, with customer organizations creating instances of the apps it offers.

Short version:

1. **A deployment is a list of apps offered and the team that manages them.**
   It is owned by an organization (the deployer's team on that cluster) or, for
   a developer on their own, an account. Each entry is an app name and the
   release it runs (`package@version`).
2. **Releases are shared underneath.** A release is stored once and may be
   offered by several deployments; it goes when no deployment offers it and no
   instance runs it. Removing an app from a deployment never touches another
   deployer.
3. **Customer organizations create instances of the apps a deployment offers.**
   Each instance records the deployment it came from, and the person creating
   it consents to that deployer. A customer org may use apps from several
   deployments.
4. **The deployment's managers list its customers**: the organizations with
   instances from it, with names, instance counts and versions, never members
   or app data. Another deployer's customers never appear, even on the same release.
5. **Frontends**: `/` keeps serving the newest release of a package. Two new
   addresses serve a specific one: `/app/{app}/`, the release that instance
   runs, and `/d/{deployment}/`, the deployment's entry for that package, which
   also tells the page which deployment to create instances under.
6. **A team manages a deployment.** Whoever holds `MANAGE_APPLICATION` in the
   owning organization may act on it, each with their own passkey, checked at
   apply on every node. Passkeys stay on accounts; nothing about a person is
   written on a deployment.
7. **Rotation happens at each link** (device, person, right, owner), each a
   signed command. A guard keeps an organization from owning a deployment it
   could lose: at least two members who can manage it.
8. **Only Cloud users deploy.** Creating a deployment needs a deployer grant
   signed with the cluster's operator key, which Calimero Cloud holds and gives
   only to its workspaces. On a cluster that requires deployments (fixed at
   genesis), apps are created only from a deployment's offers, so an account
   that merely found the node's address can use apps but never install one. A
   dedicated cluster is granted only to the workspace that owns it.
9. **Depends on #4791** (proofs at apply for memberships, invites and passkeys):
   without it, the membership step of the chain is checked only at the API.

## 1. Today, from the code

| What | Where |
| --- | --- |
| A package is recorded once; its first installer is its `owner` and the only account that may uninstall | `crates/node/src/system.rs`, `PackageRecord::owner`; `SystemCmd::UninstallPackage` (`p.owner != actor` is refused) |
| A release records its first installer and, since rc.4, everyone who installed it again | `PackageVersion::installed_by`, `PackageVersion::installers` |
| A release is shown only to accounts that may use it | `SystemState::sees_release` (shipped since rc.3; the console's G12 warning is removed in mdma #616) |
| Uninstalling is refused while any app, of any account, runs a WASM only that release provides | `SystemCmd::UninstallPackage` |
| Frontends: `/` redirects to the newest release, `/{version}/` serves a release | `crates/node/src/app_host.rs` (module doc, `serve`) |
| An app records its owner, organization and bundle, not where it was deployed from | `crates/node/src/types.rs`, `AppRecord` |
| `MANAGE_APPLICATION` lets a member upgrade an organization's apps; a publisher binding extends it to every instance of a package | `SystemState::manages_apps`; `SystemState::publishers`, `SystemCmd::BindPublisher` |
| Upgrades and release approvals carry the actor's passkey consent, verified at apply | `proof.rs`, `system::authorize` |
| Memberships, invites and passkey additions are judged at the API, not proven at apply | core #4791 adds the proofs |
| Any signed-in account may install a release from the operator's registry and create apps from what it sees; sign-up is open to anyone who reaches a node | `crates/node/src/api.rs`, `package_install` (`installer(...).account()`, no other check); `Node::install_from_registry` |
| Operator decisions carry an Ed25519 signature under the operator key recorded at genesis, verified at apply | `SystemCmd::Genesis { operator_key }`; `system::authorize` (`verify_operator`) |
| Genesis fixes the cluster's governance once, before any account exists | `SystemCmd::Genesis`, `Governance` |

## 2. Goals and non-goals

Goals:
- Two deployers of the same release do not affect each other: neither can remove
  what the other offers or runs, and neither sees the other's customers.
- A deployer's teammates manage its deployment with their own passkeys.
- A deployer can see which organizations use its apps.
- A customer's page always matches the instance behind it.
- Only Calimero Cloud's users deploy, and a dedicated cluster serves only its owner's deployments.
- Every right is proven at apply, on every node, from passkey signatures in the log.

Non-goals:
- Account or organization recovery after every passkey is lost (planned work, not here).
- Changing who upgrades an instance (owner or publisher, from the package's
  manifest). Section 11 asks whether the deployer should get it too.
- Billing. The listing gives mdma what per-deployer billing would need; pricing is mdma's.

## 3. The model

```
 CLUSTER
 ┌──────────────────────────────────────────────┐
 │ DEPLOYMENT  acme-labs                        │
 │   owner: org acme-labs-team                  │──members──▶ ana, bo (MANAGE_APPLICATION)
 │   apps:  notes  com.acme.notes@1.2.0         │                 └─▶ their passkeys
 │          board  com.acme.board@0.4.0         │
 └───────────────┬──────────────────────────────┘
                 │ instances created from its apps
     ┌───────────┼─────────────────────┐
     ▼           ▼                     ▼
  ORG globex   ORG initech           ORG dana
  notes 1.2.0  notes 1.2.0,          board 0.4.0
               board 0.4.0           (and chat, from deployment hooli)

 RELEASES (stored once): com.acme.notes@1.2.0 ◀── offered by acme-labs and by hooli
                         com.acme.board@0.4.0 ◀── offered by acme-labs
```

`SystemState::deployments: BTreeMap<DeploymentId, Deployment>`:

```rust
pub struct Deployment {
    pub id: DeploymentId,                      // typed id, like an app's
    pub name: String,                          // shown at consent ("acme-labs")
    pub owner: Owner,                          // Owner::Org(OrgId) | Owner::Account(AccountId)
    pub apps: BTreeMap<String, Offer>,         // app name -> what it offers
    pub created_by: AccountId,
    pub created_at: u64,
    pub transfer: Option<PendingTransfer>,
}

pub struct Offer {
    pub package: String,
    pub version: String,                       // the release new instances get
    pub added_by: AccountId,
    pub added_at: u64,
}
```

- One deployment per owner per cluster is the expected shape (a workspace's
  deployment); nothing forbids more, for example staging and production.
- An offer names one release. Offering a newer version replaces the entry: new
  instances get it, and existing instances move when they are upgraded.
- A release is **offered** while any deployment's entry names it, and **used**
  while any instance runs it. It is removed when it is neither.
- `AppRecord` gains `deployment: Option<DeploymentId>`. An instance created before
  this has none and appears in no deployment's listing.
- `PackageRecord::owner` and `installers` stop deciding who may remove a
  release; offers do.

## 4. Commands and who may run them

Every command below carries its actor's passkey consent over the exact statement,
verified at apply (`system::authorize`), and spends its nonce. "Manager" means:
for `Owner::Org`, a member holding `MANAGE_APPLICATION` org-wide there; for
`Owner::Account`, that account.

| Command | Effect | Authority, judged at apply |
| --- | --- | --- |
| `CreateDeployment { name, owner }` | An empty deployment | `Owner::Org`: the actor holds `MANAGE_APPLICATION` there and the org passes the lockout guard (section 9). `Owner::Account`: the actor is that account |
| `OfferApp { deployment, app, package, version }` | Installs the release if absent (as `InstallPackage` does today, pinned signer and all), then adds or replaces the entry | A manager |
| `WithdrawApp { deployment, app }` | Removes the entry. Refused while an instance created from this deployment runs that app's package. Removes the release when it is no longer offered or used | A manager |
| `OfferDeployment { deployment, to }` | Records a pending transfer | A manager |
| `AcceptDeployment { deployment }` | Moves ownership to `to` | `MANAGE_APPLICATION` in `to`, which passes the lockout guard |
| `CancelDeploymentTransfer { deployment }` | Drops a pending transfer | Either side |
| `DeleteDeployment { deployment }` | Removes an empty deployment | A manager |
| `CreateApp { app }` with `app.deployment` and `app.offer` | As today, plus: the deployment exists, offers that app, and the instance runs exactly the offered release; the creator's consent names the deployment | The creator, as today |

`UninstallPackage` stays for the operator and for releases no deployment offers
(records from before this change), and refuses one that is offered.

Reads:
- `GET /deployments` lists the deployments the caller manages; `GET /deployments/{id}`
  is public by name (its apps and owner name), for frontends and consent.
- `GET /deployments/{id}/orgs` lists, for each customer organization with an
  instance from the deployment: the org's id and name, its instances' app names
  from the deployment's list, the versions they run, and when the first was
  created. It never lists members, teams, instance names the customer chose, or
  app data. Managers only, after `sys_fresh`.
- `/ops/*` reports gain the deployment id next to each app, ids only, as today.

## 5. Who may deploy

Accounts are open: anyone who reaches a node can sign up, and must, to use a
deployer's apps. So deploying is gated separately, with the operator key that
already signs operator decisions:

| Command | Effect | Authority, judged at apply |
| --- | --- | --- |
| `GrantDeployer { owner, until }` | `owner` (an org or an account) may create deployments until `until` | The operator's signature (`verify_operator`) over the exact grant |
| `RevokeDeployer { owner }` | No new deployments or offers for `owner`; its deployments stay, and their instances run on | The operator's signature |

- `CreateDeployment` and `OfferApp` require a live grant for the deployment's owner.
- Genesis gains `deployments: open | granted` (in `Governance`, fixed once). On a
  `granted` cluster `package_install` and `POST /apps` without an offer are
  refused for accounts: every app is created from a deployment's offer, so every
  instance is attributable to a deployer, and an account that found the node's
  address uses apps but never installs one. `open` keeps today's behaviour for
  local and self-run clusters.
- Calimero Cloud (mdma) holds each cloud cluster's operator key. It signs a grant
  when a workspace makes its first deploy on a cluster, and revokes it when the
  workspace is deleted or suspended. Who it grants depends on the cluster:

| Cluster | Who may create deployments | Who may use the apps |
| --- | --- | --- |
| Shared | Every Calimero Cloud workspace (granted on its first deploy there) | Anyone who signs up, through a deployment's offers |
| Dedicated | Only the workspace that owns the cluster; mdma refuses a grant for any other | Anyone who signs up, through the owner's deployments |
| Self-run (`deployments: open`) | Any account, as today | Any account |
- A grant does not let mdma act inside a deployment: offering, withdrawing and
  transferring still need a manager's passkey (section 8). The operator can stop
  new deployments; it cannot run or change an existing one.

## 6. Customers

- A customer org creates an instance of an app a deployment offers, usually from
  that app's frontend opened at `/d/{deployment}/`. The consent the creator signs
  names the deployment ("You are starting notes from acme-labs").
- A customer org may use apps from several deployments; each instance records
  its own, and each deployer lists only the orgs with instances from it.
- Only instances belong to a deployment, not the org. An org is a customer of a
  deployment while it has at least one instance from it; deleting its last one
  ends that, with nothing else to undo. The customer is in control, and core
  keeps no record beyond the instances (decided; a signed join record can be
  added later if billing or offers to past customers need it).

## 7. Frontends

`app_host` keeps `/` (the newest release) and `/{version}/`, and adds:

- **`/app/{app}/`** redirects to `/{version}/` of the release that instance runs,
  so a link a frontend hands to a customer always opens the frontend that
  matches the customer's instance.
- **`/d/{deployment}/`** redirects to `/{version}/?deployment={id}` for the
  release the deployment offers for this package. `/.well-known/calimero.json`
  gains `deployments` (id, name, the offered version), so the page reads the one
  it was opened for. mero-js passes it on `createApp`.

Both stay `GET` and `HEAD` of public files: no account data. The deployment's
name is what the customer sees at consent anyway.

## 8. A team manages a deployment

When a teammate offers, withdraws or transfers, every node checks this chain,
all from the replicated log:

1. The actor's passkey is registered to their own account on this cluster (a
   passkey registration, proven at apply by #4791).
2. An admin of the owning org invited them, with their role and capabilities,
   signed by the admin's passkey (#4791).
3. They redeemed the invite with their own passkey session (#4791).
4. The deployment is owned by that org (`CreateDeployment` or `AcceptDeployment`, signed).
5. The command carries their passkey assertion over the exact statement, unused,
   and at apply they hold `MANAGE_APPLICATION` in that org.

mdma never holds a passkey, and the operator key signs only operator decisions,
so neither can make or extend this chain.

Passkeys are registered for one cluster's relying party, so a deployment on
eu-shared-2 is managed by passkeys registered on eu-shared-2. The console says
so when a teammate first opens a deployment on a cluster.

### Rotation

| What changes | Command | Signed by |
| --- | --- | --- |
| A device | Add a passkey to the account, then remove the old one; removal ends its sessions | Another passkey of the same account |
| A person | Invite or remove a member | An admin of the owning org |
| A right | Grant or withdraw `MANAGE_APPLICATION` | An admin of the owning org |
| The owner | `OfferDeployment`, then `AcceptDeployment` | A manager on each side |

One change at the organization covers its whole deployment; nothing is
rewritten per app or per deployment.

## 9. The lockout guard

If every manager loses every passkey, nobody can sign for the deployment again
until recovery exists. So:

- `CreateDeployment` for an org and `AcceptDeployment` require the org to have
  at least two members holding `MANAGE_APPLICATION` (`MIN_DEPLOYMENT_MANAGERS = 2`).
  A developer on their own creates an `Owner::Account` deployment instead.
- Removing a member or withdrawing the capability is refused when it would leave
  an org that owns a deployment below the minimum.
- The console warns when a manager has only one passkey on the cluster.

## 10. Calimero Cloud (mdma)

- A workspace's team org and deployment on each cluster are created at its
  first deploy there, named after the workspace, and recorded on the workspace.
  A personal workspace with one member gets an `Owner::Account` deployment.
- Deploying an app is `OfferApp`; deploying a new version of it replaces the
  entry; deleting the app from the console is `WithdrawApp`.
- Inviting a workspace member also mints the cluster invite with the right
  capabilities; they redeem it with their own passkey the first time they open
  the deployment on that cluster. Removing a member removes them there too, both
  signed by an admin in the browser as today.
- Roles map to capabilities: owner and admin hold `MANAGE_APPLICATION` and
  `APPROVE_RELEASES`; developer may offer apps and create instances (section 11).
- The console shows the deployment as the workspace's apps on that cluster, and
  each app's customer organizations from `GET /deployments/{id}/orgs`, joined
  with usage from `/metrics`.

## 11. Open questions

1. **Can a developer withdraw an app?** Proposed: no, managers only.
2. **Does a customer org belong to a deployment, or only its instances?**
   Decided: only its instances (section 6).
3. **Is the deploying org the publisher?** Proposed: separate. A workspace can
   deploy another publisher's public package and manage its own deployment
   without being bound to the signing key.
4. **Should a deployer upgrade the instances created from its deployment?**
   Today the manifest says owner or publisher. A third value, `deployer`, would
   fit customers who expect the provider to keep them current. Proposed: later.
5. **Minimum managers**: two, enforced in core (proposed), or a console warning only.
6. **Customer org names in the listing**: shown (proposed, the customer consented
   to that deployer), or ids only.
7. **Can a dedicated cluster's owner hold deployer grants without trusting
   Calimero?** As proposed, mdma signs the grants, so the owner trusts mdma not
   to grant someone else. An alternative is a genesis governance value naming
   the owner (`deployers: owner`), whose own passkey grants deployers there.
   Proposed: operator grants first, the owner option with dedicated self-service.

## 12. Phases

| Phase | What | Needs |
| --- | --- | --- |
| 0 | #4791: memberships, invites, passkeys proven at apply | #4738 merged |
| 1 | `Deployment`, `CreateDeployment`, `OfferApp`, `WithdrawApp`; records from before migrate to one `Owner::Account` deployment per installer, offering the releases it installed | Phase 0 |
| 1b | `GrantDeployer`, `RevokeDeployer`, `deployments: granted` at genesis; mdma signs grants (dedicated: owner only) | Phase 1 |
| 2 | `/app/{app}/` and `/d/{deployment}/`; `deployments` in `/.well-known/calimero.json`; `GET /deployments/{id}` | Phase 1 |
| 3 | `AppRecord::deployment`, the consent text, `GET /deployments/{id}/orgs`; mero-js passes the deployment | Phase 1 |
| 4 | Transfer and the lockout guard | Phase 1 |
| 5 | mdma: team orgs and deployments on clusters, invites mirrored, the customer list on the app page | Phases 1 to 4 |

## 13. Tests, red first

- Two deployments offer the same release; each withdraws only its own entry;
  the release stays while the other offers it or any instance runs it, and goes after.
- A withdraw is refused while an instance created from that deployment runs that app.
- An instance can be created only from an app the deployment offers, at the
  offered release; one naming another package, another version, or a deployment
  the creator does not see, is refused.
- A customer org with instances from two deployments appears in both listings,
  each showing only its own instances; a third deployer of the same release sees neither.
- A member with `MANAGE_APPLICATION` offers and withdraws with their passkey; a
  member without it, a removed member, and a forged command without the
  assertion are refused at apply on every node.
- A removed passkey's session can no longer act; the account's other passkey can.
- `/app/{app}/` opens the release the instance runs after another deployer offers a newer one.
- Without a grant, `CreateDeployment` and `OfferApp` are refused at apply; a
  grant without the operator's signature, or signed by another key, is refused.
- On a `granted` cluster an account's `package_install`, and `POST /apps` naming
  no offer, are refused; creating an instance from an offer still works for a
  customer who only signed up.
- A revoked owner's existing instances keep running; it can no longer offer.
- A transfer needs both sides; an org below the minimum cannot own a deployment
  or drop below it while it owns one.
- Migration: a data directory from before (`a09547b2b`-style fixture) opens with
  one account-owned deployment per installer, and its instances list nowhere.
