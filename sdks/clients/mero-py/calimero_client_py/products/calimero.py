"""`Calimero.from_env()`: a client from a service's environment, and `kv()`: the
generated Confidential KV client on it."""

import os
from typing import TYPE_CHECKING, Any, Dict, Mapping, Optional, Tuple

from ..client import Client

if TYPE_CHECKING:
    from .kv.client import KvClient

__all__ = ["Calimero"]

PROFILE = (
    "locked-read-only"  # the default: the release image profile a production node runs
)


class Calimero:
    """A client signed in with `CALIMERO_API_KEY` to `CALIMERO_CLUSTER` (or the
    dev node `CALIMERO_NODE_URL`), trusting the `CALIMERO_TEE_PROFILE` image
    (default `locked-read-only`) of releases from `CALIMERO_MIN_RELEASE`;
    `client` is it, for every route `kv()` does not cover."""

    def __init__(self, client: Client, store: Optional[str] = None):
        self.client = client
        self._store = store

    @classmethod
    def from_env(cls, env: Mapping[str, str] = os.environ) -> "Calimero":
        """Connect as `env` says. Raises `ValueError` naming the variable that
        is missing or wrong; a mock quote passes only with
        `CALIMERO_ALLOW_UNATTESTED=1`, which a cluster refuses."""
        cluster, node_url, api_key, policy = _settings(env)
        if cluster:
            client = Client.connect(cluster, **policy)
        else:
            client = Client(node_url, **policy)
        client.use_api_key(api_key)
        return cls(client, env.get("CALIMERO_KV_STORE") or None)

    def kv(self, store: Optional[str] = None) -> "KvClient":
        """The Confidential KV `store` (its app id; default `CALIMERO_KV_STORE`)."""
        # The generated client imports `calimero`, which imports this package.
        from .kv.client import KvClient

        store = store or self._store
        if not store:
            raise ValueError(
                "CALIMERO_KV_STORE is not set: set it to the store's app id,"
                " or pass one to kv()"
            )
        return KvClient(self.client, store)


def _settings(env: Mapping[str, str]) -> Tuple[Any, Any, str, Dict[str, Any]]:
    """`(cluster, node_url, api_key, policy)` from `env`, or the error naming
    the variable at fault. An empty variable is an unset one."""
    cluster = env.get("CALIMERO_CLUSTER") or None
    node_url = env.get("CALIMERO_NODE_URL") or None
    api_key = env.get("CALIMERO_API_KEY") or None
    min_release = env.get("CALIMERO_MIN_RELEASE") or None
    unattested = env.get("CALIMERO_ALLOW_UNATTESTED") or None
    profile = env.get("CALIMERO_TEE_PROFILE") or None
    if cluster and node_url:
        raise ValueError("set CALIMERO_CLUSTER or CALIMERO_NODE_URL, not both")
    if not cluster and not node_url:
        raise ValueError(
            "CALIMERO_CLUSTER is not set: the cluster to connect to"
            " (CALIMERO_NODE_URL for a dev node)"
        )
    if unattested and unattested != "1":
        raise ValueError("CALIMERO_ALLOW_UNATTESTED must be 1 or unset")
    if unattested and cluster:
        raise ValueError(
            "CALIMERO_ALLOW_UNATTESTED is for a dev node at CALIMERO_NODE_URL,"
            " never a cluster"
        )
    if not api_key:
        raise ValueError("CALIMERO_API_KEY is not set")
    if not min_release and not unattested:
        raise ValueError(
            "CALIMERO_MIN_RELEASE is not set: the oldest mero-tee release to"
            " trust, e.g. 2.3.109"
        )
    if profile and not min_release:
        raise ValueError(
            "CALIMERO_TEE_PROFILE needs CALIMERO_MIN_RELEASE: a profile is trusted"
            " within a release"
        )
    policy: Dict[str, Any] = {"allow_mock": bool(unattested)}
    if min_release:
        # As the console's policyFor: naming a debug* profile is the opt-in a
        # debug image needs.
        policy.update(
            tee_profiles=[profile or PROFILE],
            min_release=min_release,
            allow_debug_profiles=bool(profile) and profile.startswith("debug"),
        )
    return cluster, node_url, api_key, policy
