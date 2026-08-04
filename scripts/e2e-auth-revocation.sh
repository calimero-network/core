#!/bin/sh
# Revoked-token e2e (issue #3069): prove that a token whose key has been
# revoked is rejected with `403` and `X-Auth-Error: token_revoked` — through
# every request-auth path merod exposes — against a real running merod with
# embedded auth, over real HTTP.
#
# The bug this pins: `KeyManager::get_key` hid a revoked key (returned `None`),
# so every verification path collapsed "revoked" into the generic `401` "key
# not found" and `AuthError::TokenRevoked` (→ 403) was unreachable. The fix
# looks the key up with `get_key_including_invalid` and branches on
# `is_revoked()`. A unit test can pin each layer; only this seam proves the
# minted-token-vs-real-middleware chain actually emits the 403 header.
#
# Companion in-process pins:
#   crates/auth/src/auth/token/jwt.rs        verify_token_string_* tests
#   crates/auth/src/auth/middleware.rs       test_revoked_key_token_verification
#   crates/server/src/auth.rs                revoked_token_verification_yields_forbidden_end_to_end
#
# The complementary "absent key → 401" criterion is NOT exercisable over HTTP
# (forging a token for a never-existed key needs the signing secret), so it is
# left to the in-process tests above; a revoked key still EXISTS in the store,
# which is exactly what makes the revoked-vs-absent distinction load-bearing.
#
# Usage: e2e-auth-revocation.sh [NODE_URL]
#   NODE_URL defaults to http://localhost:5001. The node must be freshly
#   initialised with --auth-mode embedded and an admin account minted at init
#   (MERO_AUTH_ADMIN_USER / MERO_AUTH_ADMIN_PASSWORD, matching MERO_E2E_USER /
#   MERO_E2E_PASS below — binary-mode merobox inherits the caller's env).

# POSIX sh, not bash: merobox's script step hardcodes /bin/sh (dash on Ubuntu
# CI), ignoring the shebang. No pipefail — every pipeline's output is captured
# and validated explicitly below.
set -eu

NODE_URL="${1:-http://localhost:5001}"
USERNAME="${MERO_E2E_USER:-dev}"
# Must satisfy the provider's configured minimum length (default 8).
PASSWORD="${MERO_E2E_PASS:-dev-password}"

PASS=0
FAIL=0

check() { # check <label> <expected> <actual>
  label="$1"; expected="$2"; actual="$3"
  if [ "$actual" = "$expected" ]; then
    echo "ok   $label ($actual)"
    PASS=$((PASS + 1))
  else
    echo "FAIL $label: expected $expected, got $actual"
    FAIL=$((FAIL + 1))
  fi
}

# Dump response headers followed by a final `STATUS <code>` line, discarding
# the body. One capture yields both the status and any `X-Auth-Error` hint.
raw() { # raw <method> <path> <token> [body]
  method="$1"; path="$2"; token="$3"; body="${4:-}"
  curl -s -o /dev/null -D - -m 10 -X "$method" \
    -H "Authorization: Bearer $token" \
    -H 'Content-Type: application/json' \
    ${body:+-d "$body"} \
    -w 'STATUS %{http_code}\n' \
    "$NODE_URL$path"
}

status_from()   { echo "$1" | sed -n 's/^STATUS //p' | tr -d '\r'; }
# Header names are case-insensitive; lowercase before matching.
hint_from()     { echo "$1" | tr -d '\r' | awk 'tolower($1)=="x-auth-error:"{print $2; exit}'; }
authuser_from() { echo "$1" | tr -d '\r' | awk 'tolower($1)=="x-auth-user:"{print $2; exit}'; }

expect_revoked() { # expect_revoked <label> <method> <path> <token>
  label="$1"; out=$(raw "$2" "$3" "$4")
  st=$(status_from "$out"); hint=$(hint_from "$out")
  if [ "$st" = "403" ] && [ "$hint" = "token_revoked" ]; then
    echo "ok   $label (403 token_revoked)"
    PASS=$((PASS + 1))
  else
    echo "FAIL $label: expected 403 + token_revoked, got ${st:-?} + ${hint:-none}"
    FAIL=$((FAIL + 1))
  fi
}

echo "== auth-revocation e2e against $NODE_URL =="

# 1. Root login. The admin root key was minted at `merod init`; the login path
#    never mints keys, so this authenticates the existing admin.
LOGIN_BODY=$(jq -n --arg u "$USERNAME" --arg p "$PASSWORD" \
  --argjson ts "$(date +%s)" \
  '{auth_method: "user_password", public_key: $u, client_name: "auth-revocation-e2e",
    permissions: ["admin"], timestamp: $ts,
    provider_data: {username: $u, password: $p}}')
ROOT_RESPONSE=$(curl -s -m 10 -X POST "$NODE_URL/auth/token" \
  -H 'Content-Type: application/json' \
  -d "$LOGIN_BODY")
ROOT_TOKEN=$(echo "$ROOT_RESPONSE" | jq -r '.data.access_token // empty')
[ -n "$ROOT_TOKEN" ] || { echo "FATAL: root login failed: $ROOT_RESPONSE"; exit 1; }
ROOT_KEY_ID=$(authuser_from "$(raw GET /auth/validate "$ROOT_TOKEN")")
[ -n "$ROOT_KEY_ID" ] || { echo "FATAL: could not resolve root key id"; exit 1; }
echo "ok   root login (init-minted admin)"

# 2. Mint a client key scoped to context:list — enough to be ADMITTED on
#    GET /admin-api/contexts before revocation, so the 403 afterwards is
#    unambiguously a revocation, not a permission denial.
MINTED=$(curl -s -m 10 -X POST "$NODE_URL/admin/client-key" \
  -H "Authorization: Bearer $ROOT_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"context_id":"","context_identity":"","permissions":["context:list"]}')
CLIENT_TOKEN=$(echo "$MINTED" | jq -r '.data.access_token // empty')
[ -n "$CLIENT_TOKEN" ] || { echo "FATAL: client-key mint failed: $MINTED"; exit 1; }

# Baseline: the fresh token authenticates and is admitted on its scoped route.
VALIDATE_OUT=$(raw GET /auth/validate "$CLIENT_TOKEN")
check "fresh client token accepted at /auth/validate" 200 "$(status_from "$VALIDATE_OUT")"
CLIENT_ID=$(authuser_from "$VALIDATE_OUT")
[ -n "$CLIENT_ID" ] || { echo "FATAL: could not resolve client id"; exit 1; }
check "fresh client token admitted at GET /admin-api/contexts" 200 \
  "$(status_from "$(raw GET /admin-api/contexts "$CLIENT_TOKEN")")"

# 3. Revoke the client key (the admin action a real operator performs).
REVOKE_STATUS=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -X DELETE \
  -H "Authorization: Bearer $ROOT_TOKEN" \
  "$NODE_URL/admin/keys/$ROOT_KEY_ID/clients/$CLIENT_ID")
check "revoke client key (DELETE /admin/keys/:root/clients/:client)" 200 "$REVOKE_STATUS"

# 4. The revoked token must now be 403 + token_revoked through EACH request-auth
#    path — the same token, the same node, three different middlewares:
#    - calimero-server AuthGuardService → unauthorized_response  (/admin-api/*)
#    - mero-auth embedded auth_middleware                        (/admin/*)
#    - proxy-shaped validate_handler                             (/auth/validate)
expect_revoked "server guard: GET /admin-api/contexts" GET /admin-api/contexts "$CLIENT_TOKEN"
expect_revoked "embedded middleware: GET /admin/keys/clients" GET /admin/keys/clients "$CLIENT_TOKEN"
expect_revoked "validate_handler: GET /auth/validate" GET /auth/validate "$CLIENT_TOKEN"

echo "== $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
