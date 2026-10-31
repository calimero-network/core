# Deployments owned by an organization: design proposal

Oct 31, 2026 · Status: **proposal, not implemented** · No runtime code changes with this document.

A developer deploys an app from Calimero Cloud: the release is installed on a
cluster, and instances of it are created later, mostly from the app's own
frontend, one per customer organization. Several developers can deploy the same
package on the same shared cluster. Today the cluster records a release once,
owned by whichever account installed it first. This document makes a
deployment a thing of its own, owned by an organization, managed by that
organization's members with their own passkeys, and able to list the customer
organizations running on it.

Short version:

1. **A deployment is a claim on a release, owned by an organization** (or, for a
   developer on their own, an account). Several deployments may claim the same
   release. Each deployer removes only its own claim; the release goes when no
   claim is left and no app runs it.
2. **Frontends**: `/` keeps serving the newest release. Two new addresses serve
   a specific one: `/app/{app}/`, the release that app runs, and `/d/{deployment}/`,
   the release a deployment claims, which also tells the page which deployment
   to create instances under.
3. **Instances name the deployment they were created from.** The person creating
   one sees which deployer they join and consents to it. The deployment's
   managers can then list the organizations running on it: ids, names, instance
   counts, versions, never members or app data.
4. **A team manages a deployment.** Whoever holds `MANAGE_APPLICATION` in the
   owning organization may act on it, each with their own passkey, checked at
   apply on every node. Passkeys stay on accounts; nothing about a person is
   written on a deployment.
5. **Rotation happens at each link** (device, person, right, owner), each a
   signed command. A guard keeps an organization from owning deployments it
   could lose: at least two members who can manage them.
6. **Depends on #4791** (proofs at apply for memberships, invites and passkeys):
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

## 2. Goals and non-goals

Goals:
- Two deployers of the same release do not affect each other: neither can delete
  what the other relies on, and neither sees the other's customers.
- A deployer's teammates manage its deployments with their own passkeys.
- A deployer can see which organizations run on its deployment.
- A customer's page always matches the instance behind it.
- Every right is proven at apply, on every node, from passkey signatures in the log.

Non-goals:
- Account or organization recovery after every passkey is lost (planned work, not here).
- Changing who upgrades an instance (owner or publisher, from the package's
  manifest). Section 9 asks whether deployers should get it too.
- Billing. The listing gives mdma what per-deployer billing would need; pricing is mdma's.

## 3. The model

```
Deployment ──owned by──▶ Organization (or Account) ──members──▶ Account ──▶ Passkeys
     │                                                      (MANAGE_APPLICATION)
     ├──claims──▶ Release (package, version)
     └──◀──created from── App instance ──in──▶ Customer organization
```

`SystemState::deployments: BTreeMap<DeploymentId, Deployment>`:

```rust
pub struct Deployment {
    pub id: DeploymentId,          // typed id, like an app's
    pub owner: Owner,              // Owner::Org(OrgId) | Owner::Account(AccountId)
    pub package: String,
    pub version: String,           // the release it claims
    pub created_by: AccountId,
    pub created_at: u64,
    pub transfer: Option<PendingTransfer>,
}
```

A deployment claims exactly one release. Upgrading a deployment to a new version
is a new claim on the new release followed by removing the old one (section 4),
so a deployer's history is a sequence of claims.

`AppRecord` gains `deployment: Option<DeploymentId>`. An app created before this
has none and appears in no deployment's listing.

## 4. Commands and who may run them

Every command below carries its actor's passkey consent over the exact statement,
verified at apply (`system::authorize`), and spends its nonce.

| Command | Effect | Authority, judged at apply |
| --- | --- | --- |
| `Deploy { package, version, owner }` | Installs the release if absent (as `InstallPackage` does today, pinned signer and all), then records the claim | `Owner::Org`: the actor holds `MANAGE_APPLICATION` org-wide there, and the org passes the lockout guard (section 7). `Owner::Account`: the actor is that account |
| `Undeploy { deployment }` | Removes the claim. Refused while an app created from this deployment runs its release. When no claim is left and no app runs the release, the release is removed as `UninstallPackage` removes it | `MANAGE_APPLICATION` in the owning org, or the owning account |
| `OfferDeployment { deployment, to }` | Records a pending transfer | `MANAGE_APPLICATION` in the owning org |
| `AcceptDeployment { deployment }` | Moves ownership to `to` | `MANAGE_APPLICATION` in `to`, and `to` passes the lockout guard |
| `CancelDeploymentTransfer { deployment }` | Drops a pending transfer | Either side |
| `CreateApp { app }` with `app.deployment` | As today, plus: the deployment exists, claims a release of the app's package that the creator sees, and the creator's consent names the deployment | The creator, as today |

`UninstallPackage` stays for the operator and for releases with no claim (records
from before this change), and refuses a release any deployment claims.

Reads:
- `GET /deployments` lists the deployments the caller manages.
- `GET /deployments/{id}/orgs` lists, for each customer organization with an app
  created from the deployment, the org's id and name, how many apps it has there,
  which versions they run, and when the first was created. It never lists members,
  teams, app names chosen by the customer, or app data. Access is
  `MANAGE_APPLICATION` in the owning org, after `sys_fresh`.
- `/ops/*` reports gain the deployment id next to each app, ids only, as today.

## 5. Frontends

`app_host` keeps `/` (the newest release) and `/{version}/`, and adds:

- **`/app/{app}/`** redirects to `/{version}/` of the release that app runs, so a
  link a frontend hands to a customer always opens the frontend that matches
  the customer's instance.
- **`/d/{deployment}/`** redirects to `/{version}/?deployment={id}` for the
  release the deployment claims. `/.well-known/calimero.json` gains `deployments`
  for the package (id, owner name, version), so the page can read the one it was
  opened for. mero-js passes it on `createApp`.

Both stay `GET` and `HEAD` of public files: no account data, and the deployment's
owner name is what the customer sees at consent anyway.

## 6. A team manages a deployment

When a teammate upgrades or undeploys, every node checks this chain, all from
the replicated log:

1. The actor's passkey is registered to their own account on this cluster (a
   passkey registration, proven at apply by #4791).
2. An admin of the owning org invited them, with their role and capabilities,
   signed by the admin's passkey (#4791).
3. They redeemed the invite with their own passkey session (#4791).
4. The deployment is owned by that org (`Deploy` or `AcceptDeployment`, signed).
5. The command carries their passkey assertion over the exact statement, unused,
   and at apply they hold `MANAGE_APPLICATION` in that org.

mdma never holds a passkey, and the operator key signs only operator decisions,
so neither can make or extend this chain.

Passkeys are registered for one cluster's relying party, so a deployment on
eu-shared-2 is managed by passkeys registered on eu-shared-2. The console says
so when a teammate first opens an app on a cluster.

### Rotation

| What changes | Command | Signed by |
| --- | --- | --- |
| A device | Add a passkey to the account, then remove the old one; removal ends its sessions | Another passkey of the same account |
| A person | Invite or remove a member | An admin of the owning org |
| A right | Grant or withdraw `MANAGE_APPLICATION` | An admin of the owning org |
| The owner | `OfferDeployment`, then `AcceptDeployment` | An admin on each side |

One change at the organization covers every deployment it owns; nothing is
rewritten per deployment.

## 7. The lockout guard

If every member who can manage an organization's deployments loses every
passkey, nobody can sign for them again until recovery exists. So:

- `Deploy` to an org and `AcceptDeployment` require the org to have at least two
  members holding `MANAGE_APPLICATION` (`MIN_DEPLOYMENT_MANAGERS = 2`). A
  developer on their own deploys as `Owner::Account` instead.
- Removing a member or withdrawing the capability is refused when it would leave
  an org that owns deployments below the minimum.
- The console warns when a manager has only one passkey on the cluster.

## 8. Calimero Cloud (mdma)

- A workspace's organization on each cluster is created at its first deploy
  there (an org named after the workspace) and recorded on the workspace.
- Deploying sends `Deploy { owner: Org }` for that org, or `Owner::Account` for a
  personal workspace with a single member.
- Inviting a workspace member also mints the cluster invite with the right
  capabilities; they redeem it with their own passkey the first time they open
  an app on that cluster. Removing a member removes them there too, both signed
  by an admin in the browser as today.
- Roles map to capabilities: owner and admin hold `MANAGE_APPLICATION` and
  `APPROVE_RELEASES`; developer may deploy and create instances (section 9).
- The app page lists the deployment's customer organizations from
  `GET /deployments/{id}/orgs`, and joins them with usage from `/metrics`.

## 9. Open questions

1. **Can a developer undeploy?** Proposed: no, `MANAGE_APPLICATION` (owner, admin, or granted) only.
2. **Is the deploying org the publisher?** Proposed: separate. A workspace can
   deploy another publisher's public package and manage its own deployment
   without being bound to the signing key.
3. **Should a deployer upgrade the instances created from its deployment?**
   Today the manifest says owner or publisher. A third value, `deployer`, would
   fit customers who expect the provider to keep them current. Proposed: later.
4. **Minimum managers**: two, enforced in core (proposed), or a console warning only.
5. **Customer org names in the listing**: shown (proposed, the customer consented
   to join that deployer), or ids only.

## 10. Phases

| Phase | What | Needs |
| --- | --- | --- |
| 0 | #4791: memberships, invites, passkeys proven at apply | #4738 merged |
| 1 | `Deployment`, `Deploy`, `Undeploy`; records from before migrate to one `Owner::Account` deployment per installer of each release | Phase 0 |
| 2 | `/app/{app}/` and `/d/{deployment}/`; `deployments` in `/.well-known/calimero.json` | Phase 1 |
| 3 | `AppRecord::deployment`, the consent text, `GET /deployments/{id}/orgs`; mero-js passes the deployment | Phase 1 |
| 4 | Transfer and the lockout guard | Phase 1 |
| 5 | mdma: workspace orgs on clusters, invites mirrored, the listing on the app page | Phases 1 to 4 |

## 11. Tests, red first

- Two accounts deploy the same release; each undeploys only its own claim; the
  release stays while the other's claim or any app remains, and goes after.
- An undeploy is refused while an app created from that deployment runs it.
- A member with `MANAGE_APPLICATION` undeploys with their passkey; a member
  without it, a removed member, and a forged command without the assertion are
  refused at apply on every node.
- A removed passkey's session can no longer act; the account's other passkey can.
- `/app/{app}/` opens the release the app runs after another deployer installs a newer one.
- An app created from `/d/{deployment}/` carries the deployment; one naming a
  deployment of another package, or one the creator does not see, is refused.
- `GET /deployments/{id}/orgs` lists only orgs created from that deployment,
  never another deployer's, and only to its managers.
- A transfer needs both sides; an org below the minimum cannot own a deployment
  or drop below it while it owns one.
- Migration: a data directory from before (`a09547b2b`-style fixture) opens with
  one account-owned deployment per installer, and its apps list nowhere.
