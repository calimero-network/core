# Custom domains for node-served app frontends

Status: core part implemented (served origins, `served: true` on `/package-origins/{package}`, `GET /ops/served-origins`); the mero-tee, mdma, resolver and console parts are designed here and live in those repositories.

## Problem

A developer wants `kv.example.com` to show their app's frontend, served by the cluster from the package's signed bundle, with TLS ending inside the TDX node: the same guarantees as `https://{label}.apps.calimero.network/`.

Today none of the three parts that make that URL work accept another name:

- **The node** answers only `{label}.{apps_domain}`, `{cluster_label}.{apps_domain}` and deployment hosts (`crates/node/src/app_host.rs`, `Node::app_host_site`); any other `Host` reaches the node's own routes, which refuse a plain request.
- **The certificate** a node holds names its `public-url` host and `*.<apps-domain>` only (mero-tee `calimero-tls-cert.sh` `tls_names`, mdma `dispatcher/app/node_certs.py` `expected_hostnames`), so a browser following `kv.example.com CNAME <label>.apps.calimero.network` gets a name mismatch.
- **The resolver** (mdma `resolver/`, map from `manager/app/v2/app_routes.py::resolver_map`) routes only `[a-z2-7]{52}.apps.calimero.network` by SNI and closes anything else with `unrecognized_name`.

## Decision summary

1. **A custom domain is a registered origin with `served: true`.** It reuses `/package-origins/{package}`, `AddPackageOrigin` / `RemovePackageOrigin` and the package manager's passkey consent (#4846), not a parallel record. The consent payload names it: `package:{p} / __add_origin / { origin, rpId, served: true }`.
2. **Its own rp-id, never the cluster's.** Passkeys made on `kv.example.com` are for `kv.example.com` (or a registrable parent the developer names, `example.com`), separate from `calimero.network` passkeys; sessions opened there are bound to the package and name the origin, so removal ends them.
3. **TLS ends in the TD, in Traefik, with the node's existing in-enclave key.** mdma orders one certificate per custom domain over ACME DNS-01, through a CNAME the developer delegates (`_acme-challenge.kv.example.com CNAME <token>.<challenge zone>`), only for a key it has seen attested and only for a domain the cluster's log holds as served.
4. **The resolver routes the exact host by SNI** to the package's chosen cluster, from an allowlist (the map); it still never decrypts.
5. **merod owns no certificate.** Its seam is the log record, the serving, and `GET /ops/served-origins`, the list mdma reconciles certificates and routes against.

### Why a flag on registered origins, not a sibling command

Everything a custom domain needs after the bytes arrive is what a registered origin already has:

| Need | Registered origin (#4846) | Served origin adds |
| --- | --- | --- |
| Who may add / remove | `SystemState::manages_package`, passkey consent at apply (`authorize`) | nothing |
| Ceremonies from it | `api::app_origin` → `rp_with` under the origin's own rp-id | nothing |
| CORS | `lib.rs` `cors` via `app_origin` | nothing |
| Session binding | `SessionClaims::app` + `SessionClaims::origin`, refused once removed (`origin_holds`) | nothing |
| Consent page `returnTo` | `app_origin` | nothing |
| Uniqueness, limits, uninstall | `package_origins` keyed by origin, `MAX_*`, `forget_origins` | `MAX_SERVED_ORIGINS_PER_PACKAGE`, `MAX_SERVED_ORIGINS` |
| Serving the bundle | — | `Node::served_site` in `app_host::layer` |

A sibling record would duplicate all of that, and a domain could then be registered twice (once as an origin, once as a served domain) with two rp-ids. One map keyed by origin makes "who holds `https://kv.example.com`" one answer. Changing `served` on a held origin is refused (`409 ORIGIN_TAKEN`, "remove it first"), as changing its rp-id is.

The flag is in the consent so a consent signed by a client that predates served origins (`{ origin, rpId }`) can never be replayed to make a domain served: the two statements differ (`origins::add_args`, test `served_origins_carry_their_own_consent_and_limit`).

## End-to-end flow

```
Developer (console)          Cluster (core)                 mdma                         Developer DNS
---------------------        -------------------            -------------------------    ------------------------
1 "Add domain" kv.example.com
                                                            2 token, two records shown
                                                                                         3 kv CNAME custom.apps...
                                                                                           _acme-challenge.kv CNAME <token>.acme...
4 passkey: POST /package-origins/{p}
  {origin, served: true} ──> AddPackageOrigin (log)
                                                            5 GET /ops/served-origins ──> host in the log?
                                                            6 DNS check (both CNAMEs)
                                                            7 metadata tls-custom-hosts ─> node CSRs (same key)
                                                            8 CSR key == attested key? ACME DNS-01 via alias
                                                            9 metadata tls-cert-custom ─> Traefik SNI cert
                                                            10 resolver map hosts[kv] = cluster
Browser https://kv.example.com ── resolver (SNI splice) ── node Traefik (TLS) ── merod app_host::layer → bundle
```

### 1-3. Console: "Add domain" (mdma `manager/`, console)

On an environment's page (it names the package and the cluster), **Add domain** takes a host name and shows two records:

| Record | Purpose |
| --- | --- |
| `kv.example.com. CNAME custom.apps.calimero.network.` | Routing: the resolver's static address. An apex name uses `A`/`AAAA` to the same addresses (or the provider's ALIAS/ANAME). |
| `_acme-challenge.kv.example.com. CNAME <token>.acme.custom.calimero.network.` | Ownership and certificate issuance (DNS-01 delegation). |

`<token>` is 32 lowercase base32 characters of `SHA-256("calimero.custom-domain.v1\0" ‖ project_id ‖ "\0" ‖ host ‖ "\0" ‖ nonce)`, the nonce stored with the row, so the target names this project and this domain and nothing guessable: setting it is the DNS holder's statement that **this project** may have certificates for this name. A new row (after removal, or by another project) gets a new nonce and token.

The console then shows each record's state (`missing`, `wrong target`, `ok`), checked by the manager against two public resolvers (no cache longer than 60 s), and a **Check again** button. It also shows the CAA record to add (below) and the ACME account URI it names.

New mdma table `cloud_custom_domains`: `host` (unique among non-removed rows), `project_id`, `environment_id`, `cluster_id`, `token`, `nonce`, `status` (`pending-dns` → `pending-log` → `issuing` → `serving` → `removing`; `failed` with `last_error`), `dns_checked_at`, `verified_at`, `removed_at`. Certificate rows extend `cloud_node_certificates` with a nullable `custom_host` (one row per node per host), so issuance, renewal, ARI and backoff stay the existing code paths in `node_certs.py`.

### 4. The cluster's record (core, this PR)

The console (mero-js) registers the origin on the environment's cluster with the package manager's passkey:

```
POST /package-origins/{package} { "origin": "https://kv.example.com", "served": true, "rpId"?: "example.com" }
→ 412 CONFIRM_REQUIRED (challenge over package:{package} / __add_origin / { origin, rpId, served: true })
→ POST again with confirm → { package, origin, rpId, served: true, addedBy, addedAt }
```

Refused before any consent (`crates/node/src/api/origins.rs` `add`, `served_fits`):

| Case | Answer |
| --- | --- |
| A port, `http` (except `http://name.localhost:port` on a `--dev` node), path, upper case, an address | `400 BAD_ORIGIN` |
| The node serves no frontends (no `--apps-domain`) | `400 NOT_SERVING` |
| A name under the apps domain, or under the cluster's `--rp-id` | `400 BAD_ORIGIN` |
| A node's own API host, a cluster `--origins` entry, a frontend under the apps domain | `409 ORIGIN_TAKEN` |
| Held by another package, or held by this one with the other `served` | `409 ORIGIN_TAKEN` |
| More than `MAX_SERVED_ORIGINS_PER_PACKAGE` (4) served per package, `MAX_SERVED_ORIGINS` (64) per cluster, `MAX_ORIGINS_PER_PACKAGE` (8) in all | `429 QUOTA_EXCEEDED` |
| A replica, voter or learner on an older release | `409 CLUSTER_NOT_UPGRADED` |

The same rules apply in the state machine (`SystemState::add_package_origin`), so a command proposed by any node is judged by every node. The record is `PackageOrigin { package, rp_id, added_by, added_at, served }` in `SystemState::package_origins`; `served` is left out of the JSON when false, so state and log entries from before read and write the same bytes (`origins_from_before_served_ones_read_unserved_and_write_as_before`).

**Rolling upgrade.** A node on the previous release would read `served: true` as an unknown field and drop it: its state would hold the origin unserved, so it would not serve the domain but would still admit its ceremonies. To keep replicas from diverging, adding a served origin needs every replica, voter and learner on this release (`Node::require_upgraded_nodes`, as job nodes do). Plain origins are unaffected.

### 5. mdma reads the log

`GET /ops/served-origins` (ops token, sealed, after a read index):

```json
{ "fresh": true,
  "origins": [ { "origin": "https://kv.example.com", "host": "kv.example.com",
                 "label": "<app_host::label(package)>", "addedAt": "2026-11-05T10:00:00Z" } ] }
```

No package name and no account, as the rest of the Ops API: the package's `label` is what mdma already computes (`mdma_common.app_host.label`) and routes by. mdma reconciles each `cloud_custom_domains` row against the listing of **its** cluster: a row advances past `pending-log` only when the listing has its host with the label of the environment's package. A host the listing has and no row claims is ignored (logged once): the cluster's log alone never gets a certificate or a route.

### 6. Ownership verification

A domain is verified when, from the manager, both CNAMEs resolve to the shown targets on two public resolvers. The `_acme-challenge` CNAME is the proof that matters (its token names this project); the routing CNAME is checked so the console can say the site will be reachable, not as proof. Re-checked every 6 hours while `serving`, and before each renewal; a changed `_acme-challenge` target stops renewal and flags the row (`failed: delegation removed`), and the existing certificate is left to expire rather than revoked (the developer may be moving DNS providers).

### 7-9. Certificates (mero-tee + mdma `dispatcher/app/node_certs.py`)

The node's TLS key stays the one `calimero-init` generates at `/mnt/data/tls/key.pem` and whose attestation mdma already checked: the node CSR's quote binds `SHA-256(csr)` to the measured image, and mdma records the CSR key's SPKI hash (`CheckedCsr.key_sha256`). A custom-domain CSR needs no second quote: it is signed by that same key, and mdma accepts it only when its SPKI hash equals the attested one for that node. Only the TD holds the key, so only the attested node can produce such a CSR.

Exchange, over GCE like the node certificate:

| Direction | Channel | Shape |
| --- | --- | --- |
| mdma → node | instance metadata `tls-custom-hosts` | JSON array of hosts, sorted, lowercase: `["kv.example.com"]`. mdma's record, never the node's choice. |
| node → mdma | guest attribute `calimero/tls-csr-custom` | JSON object `{ "<host>": "<base64 DER CSR naming exactly that host>" }`, one CSR per host over `key.pem`, rebuilt only when the host set or the key changes. |
| mdma → node | instance metadata `tls-cert-custom` | JSON object `{ "<host>": "<PEM chain, leaf first>" }`. |

mdma, per (node, host): checks the CSR self-signature, that it names exactly `[host]`, that its key is the node's attested key (`key_sha256`), that the host is in `/ops/served-origins` and verified (6); then orders over ACME DNS-01, writing the TXT at `_acme-challenge.<token>.acme.custom.calimero.network`'s target. `certs.challenge_record_name` already maps a validation name to an alias; it gains a lookup in `cloud_custom_domains` beside `MDMA_TEE_ACME_CHALLENGE_ALIASES` (the static alias for `apps.calimero.network` stays). The challenge zone is its own Cloud DNS zone (`acme.custom.calimero.network`), delegated from `calimero.network`, written by a service account that may write TXT records there and nothing else.

One certificate per host, not one SAN certificate for all: a certificate lists its names in CT logs, and one customer's domain must not appear in another's certificate; one failing name must not block the others; and a removal does not reissue anyone else's.

The node side (mero-tee `calimero-tls-cert.sh`, `watch`) installs a chain only when every certificate parses and chains, the leaf has not expired, the leaf's key is `key.pem`'s, and the leaf names exactly the host it is filed under and that host is in `tls-custom-hosts`; it writes `/mnt/data/tls/custom/<host>.pem` and a Traefik dynamic file `/etc/traefik/dynamic/custom-certs.yml` listing `tls.certificates` (`certFile` each, `keyFile: /mnt/data/tls/key.pem`). Traefik picks the certificate by SNI and falls back to the default store's (the node certificate). The file provider watches that directory, so no restart drops connections. A host leaving `tls-custom-hosts` has its file and entry removed.

**Renewal** is the node certificate's path: a row goes back to `pending` at the CA's ARI time or `MDMA_TEE_CERT_RENEW_BEFORE_DAYS` ahead; the standing CSR is checked again against the attested key, which is re-attested on the node certificate's own schedule (TCB status can change), and the host must still be in the listing and verified.

**Budget.** GCE instance metadata holds 512 KiB per instance. A chain is about 4-6 KiB; `MAX_SERVED_ORIGINS` = 64 keeps `tls-cert-custom` under ~400 KiB beside everything else. Past that, the chains move to a per-host metadata key or another channel (open question 4).

### 10. Resolver (mdma `resolver/`, `manager/app/v2/app_routes.py`)

The route map gains an exact-host section beside the labels:

```json
{ "appsDomain": "apps.calimero.network",
  "routes": { "<label>": { ... } },
  "hosts":  { "kv.example.com": { "cluster": "eu-shared-1", "nodes": [ ... ], "allow": [ ... ] } },
  "refreshAfterSeconds": 30 }
```

`resolver_map` adds a host only for a row in `serving` (the log holds it, DNS verified, and at least one node of the cluster has its certificate installed, reported back by the node as guest attribute `calimero/tls-custom-installed`, a JSON array of hosts). Nodes listed for a host are those whose delivered `tls-cert-custom` contains it. `RouteTable.lookup` checks `hosts` by exact, lowercased SNI before taking a label; anything in neither is `unrecognized_name`, as now: the map is the allowlist. The cluster's ingress allowlist (`allow`) applies as for labels. Logs carry the host only as `custom:<first 8 hex of SHA-256(host)>`, as labels are logged without package names.

The resolver still never decrypts: it holds no key and no certificate. A wrong route reaches a node that has no certificate for the name (handshake fails) or a node of another cluster whose log does not hold the origin (no site; the `Host` reaches its sealed routes and is refused).

## Passkeys and sessions

- **rp-id**: the custom host by default; a registrable parent the developer controls when they name it (`rpId: "example.com"`), so one passkey works on all their registered origins under it. Never the cluster's `--rp-id` or a parent of it (`origins::covers_cluster`), and a served host under the cluster's rp-id is refused outright.
- **Separate passkeys**: a person who signs up on `kv.example.com` makes a passkey for that rp-id; it does not sign in on `calimero.network` origins, and a `calimero.network` passkey does not answer there (tested: `a_served_origin_serves_its_packages_frontend_and_binds_its_sessions`). The same person can link both to one account through the account's own passkey management on each origin.
- **Session**: bound to the package (`SessionClaims::app`) and naming the origin (`SessionClaims::origin`); `Principal::within` keeps it to the package's instances, and it is refused credentials, API keys, approvals, uploads. Step-ups come from the same origin under its rp-id. Removing the origin refuses those sessions and their refresh from then on.
- WebAuthn Related Origin Requests are not used, for the reasons in the protocol identities page (five registrable domains per rp-id, cluster-wide).

## CSP and what the page may reach

The page gets `app_host::page_csp`, unchanged: `default-src 'self'`, `connect-src 'self'` plus the attestation mirror (`https://cloud.calimero.network`) and every replica's API origin, `frame-ancestors 'self'`, `form-action 'self'`. `'self'` is now `https://kv.example.com`. The page reaches the nodes' API origins, which admit it through CORS because it is a registered origin, and verifies each node's quote before it signs anyone in, exactly as on the label host. The developer cannot widen the CSP; anything else (analytics, fonts from a CDN) has to be in the bundle.

## Multiple clusters

The log of each cluster is its own. A host is served by **one** cluster: the one whose environment the console row names. Another cluster's log may hold the same origin (the same developer staging a move, or anyone with a package there); mdma neither issues for nor routes to it, because no verified row with that cluster exists. Core does not and cannot prevent that registration (it has no view of other clusters), and it is harmless without a certificate and a route.

Moving a domain: add it served on cluster B (B's log), switch the console row to B's environment; mdma issues for B's nodes, flips the route once one B node reports it installed, then the developer removes it from A. Sessions are per cluster anyway (accounts live on one cluster), so users sign in again on B.

`{label}.apps.calimero.network` keeps working and still goes to the package's primary cluster; it is a different origin with the cluster's rp-id, so passkeys there and on the custom domain are distinct.

## Removal

`DELETE /package-origins/{package} { origin }` with the manager's passkey (`__remove_origin / { origin }`), or uninstalling the package's last release (`forget_origins`):

1. The node stops serving the host at once (`served_site` reads the state), refuses its ceremonies and CORS, and refuses every session opened there.
2. mdma's next reconcile finds it gone from `/ops/served-origins`: it drops the host from the resolver map, removes it from `tls-custom-hosts` and `tls-cert-custom`, stops renewal, and revokes the certificates with the ACME account key (reason `cessationOfOperation`).
3. The console tells the developer to delete both DNS records. A dangling routing CNAME is not a takeover risk on our side: a new row for that host (any project) gets a new token, so nobody else can get a certificate without the DNS holder setting the new `_acme-challenge` target.

Removing the console row without the log entry does the same from mdma's side and leaves the log entry inert (no certificate, no route); the console offers the passkey removal too.

## CT, CAA

- **CAA** (infra #183 adds it for `calimero.network`): the console shows `example.com. CAA 0 issue "letsencrypt.org; accounturi=<mdma's ACME account URI>; validationmethods=dns-01"` (RFC 8657). It pins issuance for the developer's names to Calimero Cloud's ACME account; mdma #650 records that URI. Optional for the developer; strongly suggested.
- **CT watcher** (mdma #650): watch CT logs for every verified custom host too. A certificate naming one of them whose key is not an attested node key (any key other than the `key_sha256` of a node mdma attested) is an alert to the operator and, through the console and e-mail, to the developer. It is how an issuance outside this flow (by whoever controls the name's DNS, or by a compromised mdma) is seen.

## Failure modes

| Failure | What happens | Seen as |
| --- | --- | --- |
| DNS not set or not propagated | Row stays `pending-dns`; re-checked with backoff (1 min → 1 h) for 7 days, then `failed` | Console record states |
| Log entry missing (passkey step not done) | `pending-log`; no certificate, no route | Console prompt to register |
| CAA forbids the CA | ACME order fails; `failed: CAA` | Console with the CAA line to add |
| ACME rate limits (50 certificates per registered domain per week; orders per account) | `ORDERS_PER_PASS` and backoff (`BACKOFF_BASE`..`BACKOFF_MAX`). Each node has its own key, so one order per (node, host): a 3-node cluster costs 3 certificates per domain and renewal | Console `issuing` with the retry time |
| Some nodes have the certificate, some not | Resolver lists only nodes reporting the host installed | Fewer nodes in rotation |
| No node has it (all expired, metadata failed) | Host dropped from the map: `unrecognized_name` | Console `failed`, operator alert |
| Renewal fails | Retries until expiry; console warns at 14 days | Console, alert |
| The cluster is down | No failover to another cluster (primary rule) | Outage, as for label hosts |
| Package uninstalled | Origin forgotten; as removal | Console row `removing` |
| Old node in the cluster | `409 CLUSTER_NOT_UPGRADED` on add | API answer |
| `Host` names the domain on another port | Not served (`served_origin_of` maps only 443 / no port) | 404 from node routes |

## What an attacker controlling the domain's DNS can and cannot do

Whoever controls `example.com`'s DNS controls the web origin `https://kv.example.com`. That is true of every registered origin and of any website; served origins do not change it, and the design does not pretend otherwise.

**Can:**

- Point `kv.example.com` at their own server and get a publicly trusted certificate for it from any CA (they can also change CAA). Their page is then the origin the cluster trusts for this package: it can run WebAuthn ceremonies for rp-id `kv.example.com` with users' passkeys for that rp-id and forward the assertions to the cluster, opening sessions **bound to this package** as those users, and step up confirm methods within it. A page that verifies the node's attestation would notice nothing, because the attacker's page simply does not verify.
- Point the name elsewhere or nowhere (denial of service), or swap the `_acme-challenge` target (renewal stops; console flags it).

**Cannot:**

- Reach anything outside the package: sessions from the origin are package-bound (no other package's instances, no credentials, API keys, approvals, uploads, upgrades), and its passkeys are for its own rp-id, so `calimero.network` passkeys and the cluster's own origins are out of reach.
- Make the cluster serve their code: the node serves only the package's signed bundle, verified against the publisher's signature.
- Obtain the node's TLS key or read a session the node terminates.
- Get a certificate from Calimero Cloud for a key of their own: mdma orders only for a CSR whose key is an attested node's.
- Register the origin for another package, or move it: that needs the package manager's passkey in the cluster's log; and a new console row needs the new token in `_acme-challenge`.
- Use a stale delegation: removal revokes the certificates, and a re-add issues a new token.

**Detection and limits.** The CT watcher sees a certificate for the host whose key is no attested node's and alerts both sides; the developer then removes the origin (all its sessions end). DNSSEC on the developer's zone and CAA with `accounturi` raise the bar against partial DNS compromise (a hijacked record at a registrar or provider that does not let the attacker change CAA). The protocol docs say plainly that a custom domain's sessions are only as safe as its DNS; the label host remains the choice for apps that want only Calimero's DNS in the trust base.

**An attacker controlling Calimero's challenge zone** (or mdma) can order certificates for every delegated custom domain to a key of their own: the same power they have today over `*.apps.calimero.network`. The CT watcher is the control, and is why it must cover custom hosts before they ship.

## Rollout

1. **core (this PR)**: `served` on `PackageOrigin` / `AddPackageOrigin` and in the consent; refusals and limits; serving in `app_host::layer` (`Node::served_site`); `GET /ops/served-origins`. Tests: `crates/node/tests/package_origins.rs::a_served_origin_serves_its_packages_frontend_and_binds_its_sessions`, `system::tests::served_origins_carry_their_own_consent_and_limit`, `system::tests::origins_from_before_served_ones_read_unserved_and_write_as_before`, `origins::tests::a_served_origin_is_an_https_host_without_a_port`, `app_host::tests::a_custom_domains_host_names_one_origin`. Nothing outside core uses it yet; a served origin without the parts below is a record and a site no browser can reach over TLS.
2. **clients**: mero-js `addPackageOrigin({ origin, rpId?, served })` and `served` in the listing; calimero-client-py the same plus `ops served-origins`; `meroctl` follow-up (`package origins add --served`).
3. **mero-tee**: `calimero-tls-cert.sh` handles `tls-custom-hosts` / `calimero/tls-csr-custom` / `tls-cert-custom` / `calimero/tls-custom-installed`; Traefik file provider `directory: /etc/traefik/dynamic`, `watch: true`; tests beside `scripts/ci/tests/tls-csr-test.sh` and `traefik-passthrough-test.sh`. Ships in an image release; nodes on older images simply never install custom certificates.
4. **infra**: Cloud DNS zone `acme.custom.calimero.network` delegated from `calimero.network`, a TXT-only writer for it; `custom.apps.calimero.network` A/AAAA at the resolver's static addresses; CAA for `calimero.network` (#183).
5. **mdma**: `cloud_custom_domains`, the DNS checker, `/ops/served-origins` reconcile, per-host ordering in `node_certs.py` (CSR key == attested key), challenge alias lookup in `certs.challenge_record_name`, delivery and renewal, revocation on removal, CT watcher coverage (#650 first), `resolver_map` `hosts`.
6. **resolver**: `hosts` section in `RouteTable.load` / `lookup`, `custom:<hash>` log field; tests in `resolver/tests/test_resolver.py` splicing a handshake for an exact host.
7. **console**: Add domain, records, status, CAA line, removal (with the passkey step through mero-js).

Order: 1 and 2 any time; 3 and 4 before 5; the CT watcher (#650) before 5 issues for anyone; 6 and 7 last. Until then the feature is invisible: no console entry point, and the API refuses nothing that it did before.

## Open questions

1. **Default rp-id**: the host (strict, one passkey per subdomain) or offer the registrable parent by default in the console (one passkey across the developer's subdomains, but any page of theirs on a registered origin under it shares it)? This PR defaults to the host, as registered origins do.
2. **One key or one per domain**: reusing the node's TLS key keeps attestation to one quote; a per-domain key would let a domain's certificate be revoked without touching others' key material, at the cost of a quote per CSR and more guest attributes. Proposed: reuse.
3. **Who calls the passkey step**: the console (mero-js against the cluster, with the manager's passkey for the cluster's rp-id) or the developer's own tooling (`meroctl`). Proposed: the console, with `meroctl` for CI-free setups.
4. **Scale past 64 served origins per cluster**: per-host metadata keys, or delivering chains through the node's sealed channel instead of metadata (which would need a merod route that writes into Traefik's directory, i.e. merod touching TLS files).
5. **Apex domains**: require `A`/`AAAA` to fixed resolver addresses (then those addresses can never change), or support only subdomains at first.
6. **Should the node refuse to serve a served origin until mdma confirms the certificate?** Not needed for safety (no TLS, no browser), so not done.
