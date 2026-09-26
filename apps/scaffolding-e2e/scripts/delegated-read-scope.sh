#!/bin/sh
#
# Two delegated sessions on one node read the same routes, and see different
# things.
#
# `delegated-session.sh` proves a keyholder can obtain a session, read and write.
# It reads through `/contexts/:id/query` only, so the routes opened to a
# delegated session -- the listings, the single-resource reads and the four
# context read sub-resources -- have never been driven against a running node at
# all. They are covered by unit tests over the permission layer, and by handler
# tests over hand-built stores; neither can see a real node's membership.
#
# The property under test is not "does a member get a 200". It is that a SECOND
# account, authenticated on the same node at the same moment, is served none of
# the first one's rows. That is the tenant isolation the whole scoped-read series
# exists to provide, and nothing before this asserted it end to end.
#
# The stranger is a real, fully-valid session. It is not an unauthenticated
# caller and not a malformed token: `account_proof` authenticates an ACCOUNT and
# deliberately says nothing about membership, so the stranger logs in perfectly
# well and must then be shown nothing. A test that used a broken credential would
# pass against a node that had no scoping at all.
#
# Usage: delegated-read-scope.sh <node> <node_key> <context> <namespace>
#                               <member_secret> <member_credential>
#                               <stranger_secret> <stranger_credential>

set -e

. "$(dirname "$0")/account-api.sh"

NODE="$1"
NODE_KEY="$2"
CONTEXT="$3"
NAMESPACE="$4"
MEMBER_SECRET="$5"
MEMBER_CREDENTIAL="$6"
STRANGER_SECRET="$7"
STRANGER_CREDENTIAL="$8"

[ -n "${NODE}" ] || fail "usage: delegated-read-scope.sh <node> <node_key> <context> <namespace> <member_secret> <member_credential> <stranger_secret> <stranger_credential>"
[ -n "${NODE_KEY}" ] || fail "no node key given"
[ -n "${CONTEXT}" ] || fail "no context id given"
[ -n "${NAMESPACE}" ] || fail "no namespace id given"
[ -n "${MEMBER_SECRET}" ] || fail "no member device secret given"
[ -n "${MEMBER_CREDENTIAL}" ] || fail "no member credential given"
[ -n "${STRANGER_SECRET}" ] || fail "no stranger device secret given"
[ -n "${STRANGER_CREDENTIAL}" ] || fail "no stranger credential given"

URL=$(node_url "${NODE}") || fail "could not resolve ${NODE}'s URL"

# The two sessions must differ in exactly one thing: which account signed. Same
# node, same routes, same moment -- so a difference in what comes back can only
# be the scoping.
MEMBER_TOKEN=$(mint_session "${URL}" "${NODE_KEY}" "${MEMBER_SECRET}" "${MEMBER_CREDENTIAL}")
echo "member session minted"
STRANGER_TOKEN=$(mint_session "${URL}" "${NODE_KEY}" "${STRANGER_SECRET}" "${STRANGER_CREDENTIAL}")
echo "stranger session minted -- authentication is not membership"

# The stranger logging in AT ALL is part of the criterion. If this ever starts
# failing, the scoping below stops being exercised while still reporting green,
# because every assertion on a refused caller passes for the wrong reason.
[ "${MEMBER_TOKEN}" != "${STRANGER_TOKEN}" ] || fail "both sessions got the same token"

# GET a route and leave the status in REQ_CODE and the body in REQ_BODY.
#
# An empty token omits the header rather than sending `Bearer ` with nothing
# after it: the two are different requests, and only the first is the
# "no credential at all" case the last assertion is about.
req() {
    _path="$1"
    _token="$2"
    _out=$(mktemp)
    if [ -n "${_token}" ]; then
        REQ_CODE=$(curl -sS -o "${_out}" -w '%{http_code}' \
            -H "Authorization: Bearer ${_token}" "${URL}${_path}")
    else
        REQ_CODE=$(curl -sS -o "${_out}" -w '%{http_code}' "${URL}${_path}")
    fi
    REQ_BODY=$(cat "${_out}")
    rm -f "${_out}"
}

expect_code() {
    _path="$1"
    _token="$2"
    _who="$3"
    _want="$4"
    req "${_path}" "${_token}"
    [ "${REQ_CODE}" = "${_want}" ] \
        || fail "${_who} GET ${_path}: expected ${_want}, got ${REQ_CODE} -- ${REQ_BODY}"
    echo "  ok ${_who} GET ${_path} -> ${REQ_CODE}"
}

# Present/absent rather than a 200: a listing that scopes correctly and a listing
# that is simply broken both return 200, and only the body tells them apart.
expect_lists() {
    _path="$1"
    _token="$2"
    _who="$3"
    _needle="$4"
    req "${_path}" "${_token}"
    [ "${REQ_CODE}" = "200" ] || fail "${_who} GET ${_path}: expected 200, got ${REQ_CODE}"
    echo "${REQ_BODY}" | grep -q "${_needle}" \
        || fail "${_who} GET ${_path} did not list ${_needle}: ${REQ_BODY}"
    echo "  ok ${_who} GET ${_path} lists ${_needle}"
}

expect_omits() {
    _path="$1"
    _token="$2"
    _who="$3"
    _needle="$4"
    req "${_path}" "${_token}"
    # 200, not a refusal: a listing NARROWS for a caller in no groups, it does
    # not refuse. A 403 here would mean the endpoint stopped answering rather
    # than started filtering, and the isolation would be untested.
    [ "${REQ_CODE}" = "200" ] || fail "${_who} GET ${_path}: expected 200, got ${REQ_CODE}"
    if echo "${REQ_BODY}" | grep -q "${_needle}"; then
        fail "${_who} GET ${_path} LEAKED ${_needle}: ${REQ_BODY}"
    fi
    echo "  ok ${_who} GET ${_path} omits ${_needle}"
}

echo "--- the member sees its own ---"
expect_lists "/admin-api/contexts" "${MEMBER_TOKEN}" member "${CONTEXT}"
expect_lists "/admin-api/namespaces" "${MEMBER_TOKEN}" member "${NAMESPACE}"
expect_code "/admin-api/contexts/${CONTEXT}" "${MEMBER_TOKEN}" member 200
expect_code "/admin-api/contexts/${CONTEXT}/identities" "${MEMBER_TOKEN}" member 200
expect_code "/admin-api/contexts/${CONTEXT}/identities-owned" "${MEMBER_TOKEN}" member 200
expect_code "/admin-api/contexts/${CONTEXT}/storage" "${MEMBER_TOKEN}" member 200
expect_code "/admin-api/namespaces/${NAMESPACE}" "${MEMBER_TOKEN}" member 200
expect_code "/admin-api/namespaces/${NAMESPACE}/groups" "${MEMBER_TOKEN}" member 200

# The one read whose BODY is checked as well as its status. A context names its
# owning group, and the group here is the namespace the scenario created, so this
# is the only route that proves the answer is about THIS context rather than a
# well-formed empty one.
expect_lists "/admin-api/contexts/${CONTEXT}/group" "${MEMBER_TOKEN}" member "${NAMESPACE}"

# A namespace IS a root group, so its id is a group id and the group reads are
# reachable with the same value. `/groups/:id/contexts` is checked on its body
# for the same reason the listings are: it is a listing, and an empty one
# answers 200 just as happily as a correct one.
expect_code "/admin-api/groups/${NAMESPACE}" "${MEMBER_TOKEN}" member 200
expect_lists "/admin-api/groups/${NAMESPACE}/contexts" "${MEMBER_TOKEN}" member "${CONTEXT}"

echo "--- the stranger sees none of it ---"
# The listings filter; they do not refuse.
expect_omits "/admin-api/contexts" "${STRANGER_TOKEN}" stranger "${CONTEXT}"
expect_omits "/admin-api/namespaces" "${STRANGER_TOKEN}" stranger "${NAMESPACE}"

# Naming an id must not reach what the listing hid -- otherwise scoping the
# listing is decoration.
#
# The two families answer with DIFFERENT codes, and that is current behaviour
# being pinned rather than endorsed: the namespace reads refuse with 404 so a
# single-resource read cannot be used to ask "what else is there", while the
# context reads refuse with 403, which confirms to a caller holding a guessed id
# that the context exists. Both are asserted exactly as they are; making them
# agree is a behaviour change and belongs in its own change, not in a test.
expect_code "/admin-api/contexts/${CONTEXT}" "${STRANGER_TOKEN}" stranger 403
expect_code "/admin-api/contexts/${CONTEXT}/identities" "${STRANGER_TOKEN}" stranger 403
expect_code "/admin-api/contexts/${CONTEXT}/identities-owned" "${STRANGER_TOKEN}" stranger 403
expect_code "/admin-api/contexts/${CONTEXT}/storage" "${STRANGER_TOKEN}" stranger 403
expect_code "/admin-api/contexts/${CONTEXT}/group" "${STRANGER_TOKEN}" stranger 403
expect_code "/admin-api/namespaces/${NAMESPACE}" "${STRANGER_TOKEN}" stranger 404
expect_code "/admin-api/namespaces/${NAMESPACE}/groups" "${STRANGER_TOKEN}" stranger 404

# The group pair answers 404 like the namespace pair, not 403 like the context
# pair -- the same split recorded above, and pinned here on both sides so the
# inconsistency is visible in one place rather than inferred from two files.
expect_code "/admin-api/groups/${NAMESPACE}" "${STRANGER_TOKEN}" stranger 404
expect_code "/admin-api/groups/${NAMESPACE}/contexts" "${STRANGER_TOKEN}" stranger 404

# Containment: the group reads that were NOT opened must stay shut. A delegated
# session reaching a roster or a signing key would be the leak this batch is
# meant to avoid, and `/groups/:id` is a catch-all prefix away from all of them.
# Real routes, every one: the guard runs BEFORE routing, so a 403 comes back for
# a path that does not exist either, and a typo here would assert nothing.
for shut in members member-devices subgroups settings/default-capabilities; do
    req "/admin-api/groups/${NAMESPACE}/${shut}" "${MEMBER_TOKEN}"
    [ "${REQ_CODE}" = "403" ] \
        || fail "member GET /admin-api/groups/*/${shut}: expected 403, got ${REQ_CODE} -- ${REQ_BODY}"
    echo "  ok /admin-api/groups/*/${shut} stays shut to a delegated session -> 403"
done

# Opening these to a delegated session must not have opened them to the world.
req "/admin-api/contexts" ""
[ "${REQ_CODE}" = "401" ] || fail "an unauthenticated caller got ${REQ_CODE} from the contexts listing, expected 401"
echo "  ok unauthenticated GET /admin-api/contexts -> 401"

echo "delegated read scoping holds: the member is served its own, the stranger none of it"
