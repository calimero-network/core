"""Deep cases: the cluster's sign-up policy, changed by the operator.

Other cases read the policy as `open` in every phase, so each check here changes it, sees every node
refuse a sign-up and changes it back."""

from __future__ import annotations

from upgrade_deep import DeepCase, anyone, eq, operator, refused, until
from upgrade_path import case


@case
class SignUpPolicy(DeepCase):
    name = "sign-up-policy"
    area = "Accounts"
    seeds = "the operator's sign-up policy changed to `invite` and to `closed` and back to `open` (`POST /ops/sign-up`)"
    verifies = ("in each phase, on every node: the policy as `GET /sign-up` reads it and a sign-up "
                "(`register/begin`) refused with `SIGN_UP_REFUSED` while it is not `open`, accepted once it is")

    @staticmethod
    def _begin(w, node):
        return anyone(w, "POST", "auth/passkey/register/begin", {"name": "policy-probe"}, node=node)

    def _enforced(self, w, policy):
        operator(w, "POST", "ops/sign-up", {"policy": policy})
        try:
            for n in w.members():
                until(f"the policy on node {n}", lambda n=n: eq(anyone(w, "GET", "sign-up", node=n).get("policy"),
                                                                 policy, f"the policy on node {n}"), 15)
                refused("SIGN_UP_REFUSED", lambda n=n: self._begin(w, n), f"a sign-up on node {n} under {policy}",
                        retry_for=10)
        finally:
            operator(w, "POST", "ops/sign-up", {"policy": "open"})

    def populate(self, w):
        return {"policies": ["invite", "closed"]}

    def check(self, w, s):
        for policy in s["policies"]:
            self._enforced(w, policy)
        for n in w.members():
            eq(anyone(w, "GET", "sign-up", node=n).get("policy"), "open", f"the policy on node {n}")
            self._begin(w, n)
