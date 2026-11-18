"""
Per-package web origins (core's `/package-origins/{package}`) through the
facade (`client.packages.origins_*`): listing, the passkey step-up on adding
and removing one, `served=` for a custom domain (and the operator's
`client.ops.served()`), and `origin=` on sign-up and sign-in.

These check the signatures only; the routes themselves are exercised against
a real cluster in test_cloud_v2_api.py (skipped on a node without them).
"""

import inspect

from calimero_client_py import Client
from calimero_client_py.facade import Ops, Packages


def params(cls, method) -> list:
    return list(inspect.signature(getattr(cls, method)).parameters)


def test_the_origin_methods_take_what_the_routes_take():
    assert params(Packages, "origins_list")[1:] == ["package"]
    assert params(Packages, "origins_add")[1:] == [
        "package",
        "origin",
        "rp_id",
        "served",
        "confirm",
    ]
    assert params(Packages, "origins_remove")[1:] == ["package", "origin", "confirm"]


def test_the_operator_lists_served_origins():
    assert params(Ops, "served")[1:] == []


def test_sign_up_and_sign_in_take_an_optional_origin():
    for method in ("sign_up", "sign_in"):
        origin = inspect.signature(getattr(Client, method)).parameters["origin"]
        assert origin.default is None, method


def test_delete_confirmed_takes_an_optional_body():
    sig = inspect.signature(Client.delete_confirmed)
    assert list(sig.parameters)[1:] == ["path", "passkey", "body"]
