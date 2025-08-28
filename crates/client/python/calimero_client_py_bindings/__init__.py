"""
Calimero Client Python Bindings

A comprehensive Python client library for Calimero Network APIs,
built with PyO3 for high performance and native integration.
"""

__version__ = "0.1.0"
__author__ = "Calimero Network"
__email__ = "team@calimero.network"

# Import main functions and classes from the Rust bindings
from ._calimero_client_py_bindings import (
    create_connection,
    create_client,
    ConnectionInfo,
    Client,
    JwtToken,
    ClientError,
    AuthMode,
)

# Re-export main types
__all__ = [
    "create_connection",
    "create_client", 
    "ConnectionInfo",
    "Client",
    "JwtToken",
    "ClientError",
    "AuthMode",
]
