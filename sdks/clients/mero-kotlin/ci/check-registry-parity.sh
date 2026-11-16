#!/usr/bin/env bash
#
# check-registry-parity.sh — every public SDK method must be reachable from the
# sample app's SDK Explorer.
#
# The sample app doubles as the SDK's living documentation: if a method exists on
# CalimeroClient but no `SDKOperation` invokes it, the explorer quietly
# stops covering part of the surface. This script diffs the two lists and fails
# with the missing names.
#
# Methods that intentionally have no explorer entry go in EXCLUDED below, with a
# reason — internal plumbing, or things the UI drives on your behalf.
#
# Usage: ./ci/check-registry-parity.sh   (from the repo root)

set -euo pipefail

CORE_SRC="mero-core/src/main/kotlin/com/calimero/mero"
REGISTRY="sample-app/src/main/kotlin/com/calimero/mero/sample/explorer/OperationRegistry.kt"

# Internal / non-explorer methods, each with a reason.
EXCLUDED=(
  signUp             # driven by the sign-in screen (a passkey ceremony)
  signIn             # ditto
  signOut            # the Home tab's Sign out
  forget             # local only: drops the session without a request
  useApiKey          # credential plumbing, not a request
  useSession         # ditto
  onSessionChange    # ditto
  events             # SSE stream — consumed live by the chat screen, not a form
  presenceStream     # SSE stream, as events
  put                # raw escape hatch; get and post have entries
  delete             # ditto
  postConfirmed      # raw escape hatch behind the confirmed methods
  deleteConfirmed    # ditto
  appsReportAll      # every page of appsReport (which has an entry)
  openConsentPage    # requestConsent + a navigation: the Approvals card on Home
  waitForConsent     # polling around consentResult (which has an entry)
  upgradeOrg         # operator-scale upgrade runs: not a phone's job
  retryUpgradeRun    # ditto
  upgradePublished   # publisher upgrade runs: ditto
  approveRelease     # refused to an app's own session: approvals go through requestConsent
  approveOutbound    # ditto
  bindPublisher      # a package signing key's job, not a phone's
  setMemberMetadata  # the chat sets display names through MeroAppBackend.setMemberName
  uploadBundle       # development-only raw WASM upload
  uploadPackage      # development-only .mpk upload
  opsApps            # operator token only
  opsOrgs            # ditto
  setReleaseDeadline # ditto
  liftReleaseDeadline # ditto
  api                # the generated client (MeroApi) under the facade properties: plumbing, not a request
  connect            # how the client is made (by cluster name), not a request; the sample signs in by node URL
  downloadBlobGranted # core rc.7 routes, kept out of the sample as in the Swift sample
  blobContent        # ditto
  appXcalls          # ditto
  grantDeployer      # ditto (the cluster owner's)
  deployers          # ditto
  revokeDeployer     # ditto
  createDeployment   # ditto
  deployments        # ditto
  deployment         # ditto
  deleteDeployment   # ditto
  offerApp           # ditto
  withdrawApp        # ditto
  deploymentOrgs     # ditto
  checkConsentCode   # run by openConsentPage / ConsentLauncher
  signUpPolicy       # core rc.9/rc.10 routes, kept out of the sample as in the Swift sample
  uploadFile         # ditto
  packageOrigins     # ditto (a package manager's, for a web frontend)
  addPackageOrigin   # ditto
  removePackageOrigin # ditto
  setRecoveryKey     # ditto (an organization owner's recovery seed)
  startRecovery      # ditto
  cancelRecovery     # ditto
  finishRecovery     # ditto
)

methods() {
  # Public members of the class only: four-space indent, no visibility modifier.
  grep -hE '^    (suspend )?fun ' "$CORE_SRC/CalimeroClient.kt" \
    | grep -ohE 'fun `?[a-zA-Z0-9_]+' | sed -E 's/^fun `?//' | sort -u
}

# Third string literal of each op(...) is the method name.
registry_names() {
  grep -ohE '"[a-zA-Z0-9_.]+", *"[^"]+", *"[a-zA-Z0-9_.]+"' "$REGISTRY" \
    | awk -F'", *"' '{ gsub(/"/, "", $3); print $3 }' | sort -u
}

missing=""
reg=$(registry_names)
while read -r m; do
  [ -n "$m" ] || continue
  skip=false
  for e in "${EXCLUDED[@]}"; do [ "$m" = "$e" ] && skip=true && break; done
  $skip && continue
  # Registry entries may be qualified (e.g. rpc.execute) — match the bare tail too.
  if ! grep -qE "(^|\.)${m}$" <<< "$reg"; then
    missing="$missing $m"
  fi
done <<< "$(methods)"

echo "SDK methods:        $(methods | wc -l | tr -d ' ')"
echo "Registry entries:   $(printf '%s\n' "$reg" | grep -c . | tr -d ' ')"

if [ -n "$missing" ]; then
  echo
  echo "✗ SDK methods with no SDK Explorer entry:"
  for m in $missing; do echo "    - $m"; done
  echo
  echo "Add an SDKOperation for each in $REGISTRY,"
  echo "or list it in EXCLUDED in this script with a reason."
  exit 1
fi

echo "✓ every public SDK method has an SDK Explorer entry"
