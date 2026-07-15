#!/bin/sh
# Auth-seam e2e: prove that a client token minted the way the auth frontend
# mints it can call the routes the SDK uses — against a real running merod
# with embedded auth, over real HTTP.
#
# This is the test that was missing when 0.11.0-rc.9 shipped scope
# enforcement: tokens minted by the login flow 403'd on every SDK route and
# every app looped back to login. Unit tests couldn't see it (each layer was
# individually "correct"); only the minted-token-vs-real-middleware seam
# shows it.
#
# Companion pins:
#   crates/auth/tests/client_token_contract.rs      (validator-level, in-process)
#   auth-frontend  src/__tests__/client-key-permissions.test.tsx
#   mero-react     tests/e2e/client-token.test.ts
#
# Usage: e2e-auth-seam.sh [NODE_URL]
#   NODE_URL defaults to http://localhost:4001. The node must be freshly
#   initialised with --auth-mode embedded (first login bootstraps the root
#   user with the credentials below).

# POSIX sh, not bash: merobox's script step hardcodes /bin/sh (dash on
# Ubuntu CI), ignoring the shebang. No pipefail — every pipeline's output
# is captured and validated explicitly below.
set -eu

NODE_URL="${1:-http://localhost:4001}"
USERNAME="${MERO_E2E_USER:-dev}"
# Must satisfy the provider's configured minimum length (default 8) — the
# bootstrap path enforces it for every NEW credential.
PASSWORD="${MERO_E2E_PASS:-dev-password}"

# The exact permission strings mero-react demands for AppMode.MultiContext
# (getPermissionsForMode) and auth-frontend forwards untouched.
MULTI_CONTEXT_PERMISSIONS='["context:create","context:list","context:execute"]'

PASS=0
FAIL=0

check() { # check <label> <expected> <actual>
  local label="$1" expected="$2" actual="$3"
  if [ "$actual" = "$expected" ]; then
    echo "ok   $label ($actual)"
    PASS=$((PASS + 1))
  else
    echo "FAIL $label: expected $expected, got $actual"
    FAIL=$((FAIL + 1))
  fi
}

check_admitted() { # check_admitted <label> <actual>
  # The auth layer speaks 401 (unauthenticated) and 403 (denied); any other
  # status means the request got PAST authorization (the handler may still
  # 4xx on the body, which is fine for these probes).
  local label="$1" actual="$2"
  if [ "$actual" = "401" ] || [ "$actual" = "403" ]; then
    echo "FAIL $label: rejected by the auth layer ($actual)"
    FAIL=$((FAIL + 1))
  else
    echo "ok   $label ($actual, not 401/403)"
    PASS=$((PASS + 1))
  fi
}

status_of() { # status_of <method> <path> <token> [body]
  local method="$1" path="$2" token="$3" body="${4:-}"
  curl -s -o /dev/null -w '%{http_code}' -m 10 -X "$method" \
    -H "Authorization: Bearer $token" \
    -H 'Content-Type: application/json' \
    ${body:+-d "$body"} \
    "$NODE_URL$path"
}

echo "== auth-seam e2e against $NODE_URL =="

# 1. Bootstrap root login (first login on a fresh embedded-auth node creates
#    the root key with admin).
LOGIN_BODY=$(jq -n --arg u "$USERNAME" --arg p "$PASSWORD" --argjson ts "$(date +%s)" \
  '{auth_method: "user_password", public_key: $u, client_name: "auth-seam-e2e",
    permissions: ["admin"], timestamp: $ts,
    provider_data: {username: $u, password: $p}}')
ROOT_RESPONSE=$(curl -s -m 10 -X POST "$NODE_URL/auth/token" \
  -H 'Content-Type: application/json' \
  -d "$LOGIN_BODY")
ROOT_TOKEN=$(echo "$ROOT_RESPONSE" | jq -r '.data.access_token // empty')
[ -n "$ROOT_TOKEN" ] || { echo "FATAL: root login failed: $ROOT_RESPONSE"; exit 1; }
echo "ok   root login (bootstrap)"

# 2. Mint a client key exactly like auth-frontend does for multi-context
#    mode: empty context binding, requested permissions passed through.
MINTED=$(curl -s -m 10 -X POST "$NODE_URL/admin/client-key" \
  -H "Authorization: Bearer $ROOT_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"context_id\":\"\",\"context_identity\":\"\",\"permissions\":$MULTI_CONTEXT_PERMISSIONS}")
CLIENT_TOKEN=$(echo "$MINTED" | jq -r '.data.access_token // empty')
[ -n "$CLIENT_TOKEN" ] || { echo "FATAL: client-key mint failed: $MINTED"; exit 1; }
echo "ok   client key minted with $MULTI_CONTEXT_PERMISSIONS"

# 3. The routes the SDK uses, called with ONLY the client token.
#    /auth/validate is the session gate (mero-react >= PR #41).
check "GET /auth/validate (session gate)" 200 \
  "$(status_of GET /auth/validate "$CLIENT_TOKEN")"

check "GET /admin-api/contexts (context:list)" 200 \
  "$(status_of GET /admin-api/contexts "$CLIENT_TOKEN")"

# Permission layer must admit these; the handler may still 4xx on the bodies
# (anything but 401/403 proves the token authenticated AND was authorized).
check_admitted "POST /jsonrpc (context:execute) admitted" \
  "$(status_of POST /jsonrpc "$CLIENT_TOKEN" '{"jsonrpc":"2.0","id":1,"method":"execute","params":{}}')"

check_admitted "POST /admin-api/contexts (context:create) admitted" \
  "$(status_of POST /admin-api/contexts "$CLIENT_TOKEN" '{}')"

# 4. KNOWN GAP pin: namespace routes have no permission mappings, so the
#    /admin-api/* default-deny makes them admin-only. When namespace
#    mappings land, THIS ASSERTION MUST FLIP to expect success — that
#    failure is the signal to update the seam, not an accident.
check "POST /admin-api/namespaces is admin-only (pinned gap)" 403 \
  "$(status_of POST /admin-api/namespaces "$CLIENT_TOKEN" '{"applicationId":"x","name":"seam"}')"

# 5. Regression pin (the rc.9 outage shape): a token whose context
#    permissions are scoped to an application id must be REJECTED — those
#    scopes parse as context ids and can never satisfy the Global route
#    requirements. If this ever starts passing, scope enforcement broke.
APP_SCOPED=$(curl -s -m 10 -X POST "$NODE_URL/admin/client-key" \
  -H "Authorization: Bearer $ROOT_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"context_id":"","context_identity":"","permissions":["context:create[9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu]","context:list[9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu]","context:execute[9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu]"]}')
APP_SCOPED_TOKEN=$(echo "$APP_SCOPED" | jq -r '.data.access_token // empty')
[ -n "$APP_SCOPED_TOKEN" ] || { echo "FATAL: app-scoped mint failed: $APP_SCOPED"; exit 1; }

check "app-id-scoped token rejected on GET /admin-api/contexts" 403 \
  "$(status_of GET /admin-api/contexts "$APP_SCOPED_TOKEN")"
check "app-id-scoped token rejected on POST /jsonrpc" 403 \
  "$(status_of POST /jsonrpc "$APP_SCOPED_TOKEN" '{"jsonrpc":"2.0","id":1,"method":"execute","params":{}}')"

echo "== $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
