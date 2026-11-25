"""Deep cases: the statement a node signs that a passkey session is an account's (`POST /auth/account-proof`)."""

from __future__ import annotations

import json
import uuid

import ed25519_sign
from upgrade_deep import DeepCase, check, eq, ops, people, refused
from upgrade_path import case

DOMAIN = b"calimero.account-proof.v1\0"
AUDIENCE = "mdma"
TTL = 5 * 60


@case
class AccountProofs(DeepCase):
    name = "account-proofs"
    area = "Accounts"
    seeds = "an account's proof of its passkey session, asked of each node and checked against that node's receipt key"
    verifies = ("in each phase, from every node, old and new: the statement (cluster, node, account, nonce, "
                "audience, five minutes), the key the operator reads for the node "
                "(`GET /ops/nodes/{id}/receipt-key`) and an Ed25519 signature over the domain and the statement's "
                "canonical JSON, rebuilt here; an API key and a short nonce refused")

    @staticmethod
    def _prove(w, p, n):
        nonce = uuid.uuid4().hex
        proof = p.raw("post", "auth/account-proof", {"nonce": nonce, "audience": AUDIENCE}, node=n, approve=False)
        st = proof["statement"]
        eq((st["account"], st["nonce"], st["audience"], st["node"]), (p.id, nonce, AUDIENCE, n),
           f"the statement node {n} signed")
        eq(st["expiresAt"] - st["issuedAt"], TTL, f"the proof's life on node {n}")
        key = ops(w, "GET", f"ops/nodes/{n}/receipt-key", node=n)["key"]
        eq(proof["key"], key, f"the key node {n} signed with")
        message = DOMAIN + json.dumps(st, sort_keys=True, separators=(",", ":")).encode()
        check(ed25519_sign.verify(bytes.fromhex(key), message, bytes.fromhex(proof["signature"])),
              f"node {n}'s signature does not verify under its receipt key: {proof}")

    def populate(self, w):
        return {}

    def check(self, w, s):
        p = people(w, "ime")[0]
        for n in w.members():
            self._prove(w, p, n)
            body = {"nonce": uuid.uuid4().hex, "audience": AUDIENCE}
            refused("FORBIDDEN", lambda n=n: p.http("POST", "auth/account-proof", body, node=n),
                    f"an API key asking node {n} for a proof")
            refused("BAD_NONCE", lambda n=n: p.raw("post", "auth/account-proof", {"nonce": "short", "audience": AUDIENCE},
                                                    node=n, approve=False), f"a short nonce on node {n}")
