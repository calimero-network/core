"""
Calimero Client Python Library

A Python client for Calimero 1.0 clusters, over core's Rust `calimero-client`,
built with PyO3. `Client` is the preferred name of the client class;
`CloudClient` is the same class under the name earlier releases used.
"""

from importlib.metadata import version as _installed_version

# Declared once, in Cargo.toml.
__version__ = _installed_version("calimero-client-py")
__author__ = "Calimero Network"
__email__ = "team@calimero.network"

from calimero_client_py import (
    Client,
    CloudClient,
    CloudError,
    FeatureNotActive,
    LoginRequired,
    SoftPasskey,
    WrongNode,
)

__all__ = [
    "Client",
    "CloudClient",
    "CloudError",
    "FeatureNotActive",
    "LoginRequired",
    "SoftPasskey",
    "WrongNode",
]
