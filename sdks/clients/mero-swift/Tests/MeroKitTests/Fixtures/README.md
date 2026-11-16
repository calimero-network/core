# Response fixtures

Bodies captured **verbatim** from a live `merod`, named for the core release
they came from. `rc32-*` is the current pin (`ci/core-version`); the `rc29-*`
files are kept because an older node's response is itself a case worth
decoding — `rc29-node-identity.json` has no `holdsAccountRoot`, which is how
`Rc32SurfaceTests` checks the field defaults instead of failing the response.

Nothing here is hand-assembled from what the SDK expects, which is the point: a
fixture written to match the model can only confirm the model agrees with
itself.

That is not hypothetical. `joinNamespace` shipped broken against core rc.25
because every test asserted the *request* — verb, path, body — and none decoded
a realistic reply, so a renamed response field went unnoticed until the call
threw on every join. And the previous invitation model named 2 of the envelope's
5 keys and 5 of the signed body's 6, with a full green suite, because each test
built its input from the same model it then asserted on.

## Core's own wire fixtures (`rc83-*`)

`rc83-*.json` are not captures from a live node. They are core's own wire
fixtures, copied verbatim from `crates/server/primitives/fixtures/wire/` at tag
`0.11.0-rc.83` and renamed `rc83-<dir>-<name>-<req|res>.json`. Core's suite
asserts its serializers write exactly these bytes, so they are as authoritative
as a capture and need no running node. Refresh them with:

```sh
git -C core show 0.11.0-rc.83:crates/server/primitives/fixtures/wire/groups/group_info.res.json \
  > rc83-groups-group-info-res.json
```

## Refreshing after a core bump

Boot a node at the pinned release (TESTING.md §4a) and re-capture. An
application has to be installed first — since rc.32 that means coordinates the
node's own `[registry]` can resolve, and `merod init` writes the public registry
into a fresh config:

```sh
TOK=…   # POST /auth/token, see TESTING.md §4b
B=http://localhost:4001/admin-api
AUTH="Authorization: Bearer $TOK"
JSON='Content-Type: application/json'

APP=$(curl -s -X POST "$B/install-application" -H "$AUTH" -H "$JSON" \
  -d '{"package":"com.calimero.chat","version":"3.1.1"}' \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["data"]["applicationId"])')
NS=$(curl -s -X POST "$B/namespaces" -H "$AUTH" -H "$JSON" \
  -d "{\"applicationId\":\"$APP\"}" \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["data"]["namespaceId"])')

curl -s "$B/identity"                  -H "$AUTH"  # rc32-node-identity.json
curl -s "$B/account/devices"           -H "$AUTH"  # rc32-account-devices.json
curl -s "$B/account/applications"      -H "$AUTH"  # rc32-account-applications.json
curl -s "$B/groups/$NS/member-devices" -H "$AUTH"  # rc32-member-devices.json
curl -s "$B/alias/list/device"         -H "$AUTH"  # rc32-alias-list-device.json
curl -s "$B/namespaces"                -H "$AUTH"  # rc32-namespaces.json  (appVersion, no upgradePolicy)
curl -s "$B/groups/$NS"                -H "$AUTH"  # rc32-group-info.json  (groupStateHash)

curl -s -X POST "$B/namespaces/$NS/invite" -H "$AUTH" -H "$JSON" \
  -d '{"inviteeIdentity":"11…11"}'                 # rc32-namespace-invitation.json

# The two install shapes: coordinates, and the stale URL body core now refuses.
curl -s -X POST "$B/install-application" -H "$AUTH" -H "$JSON" \
  -d '{"package":"com.calimero.chat","version":"3.1.1"}'   # rc32-install-application.json
curl -s -X POST "$B/install-application" -H "$AUTH" -H "$JSON" \
  -d '{"url":"https://example/a.mpk","metadata":[]}'       # rc32-install-application-url-refused.json
```

An SSE frame needs a subscription and a write, not just a GET
(`rc32-sse-state-mutation.json`):

```sh
curl -sN "http://localhost:4001/sse?token=$TOK" > sse.log &   # read session_id
curl -s -X POST http://localhost:4001/sse/subscription -H "$AUTH" -H "$JSON" \
  -d "{\"id\":\"$SID\",\"method\":\"subscribe\",\"params\":{\"contextIds\":[\"$CTX\"]}}"
# then any state-changing RPC on $CTX; the frame lands in sse.log
```

Ids, signatures and multiaddrs differ per node, so a re-capture will not be a
clean diff. What matters is the **key set** and the shape of each value.

A diff against the committed file is itself the signal: if a field appeared or
changed shape, that is a wire change to handle, not a fixture to overwrite
quietly.

## TEE fixtures (`tee/`)

Copied verbatim from mero-js `src/sealed/fixtures/` and
`src/agent-gate/fixtures/`, so the Swift port is pinned to the same inputs as
the JavaScript one:

- `mero-tee-v2.3.86.*`, `mero-tee-v2.3.87.*`: real mero-tee node releases as
  the release workflow published them — `published-mrtds.json`, its cosign
  bundle and its detached `.sig` — plus a copy with one byte changed
  (`tampered`) and the same release's `release-provenance.json` (signed by the
  node workflow, but not a measurements file).
- `mero-kms-v2.3.87.kms-attestation-policy.json*`: a real mero-kms policy and
  its bundle (signed by the KMS workflow, `role: "kms"`).
- `tdx_quote.bin`, `tdx_quote_collateral.json`: dcap-qvl's sample TDX quote and
  the Intel-signed collateral for it, valid on 2025-06-27.
- `rust-sealed-bundle.json`: an HPKE bundle mero-agent-gate (Rust, the `hpke`
  crate) sealed.
