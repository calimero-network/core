"""The Python client for Calimero 1.0.

`Client` (also `CloudClient`, the name earlier releases used) talks to one
node of a Calimero 1.0 cluster. The routes are `calimero_client_py.openapi`,
generated from core's OpenAPI document, on a transport that seals each request
to the node's attested key (`calimero_client_py.sealed`); the attestation, the
sealed channel, the event streams and the keys the client signs with are the
native extension's (`calimero_client_py._native`), over core's Rust
`calimero-client`.
"""

from ._native import (
    VERSION,
    CloudError,
    FeatureNotActive,
    LoginRequired,
    SoftPasskey,
    WrongNode,
)
from .client import Client, CloudClient

__all__ = [
    "Client",
    "CloudClient",
    "CloudError",
    "FeatureNotActive",
    "LoginRequired",
    "SoftPasskey",
    "WrongNode",
    "VERSION",
]
