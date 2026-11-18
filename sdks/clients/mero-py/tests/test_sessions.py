"""
The account's sessions (core #4894's `/auth/sessions`) through the facade:
`client.auth.sessions_list()` lists them and `client.auth.session_revoke(
session=...)` ends one.

These check the signatures only; the routes themselves are exercised against
a real cluster in test_cloud_v2_api.py (skipped on a node without them).
"""

import inspect

from calimero_client_py.facade import Auth

UNSET = inspect.signature(Auth.activity_list).parameters["after"].default


def params(method) -> list:
    return list(inspect.signature(getattr(Auth, method)).parameters)


def test_sessions_list_takes_nothing():
    assert params("sessions_list")[1:] == []


def test_session_revoke_takes_the_session_id():
    assert params("session_revoke")[1:] == ["session"]


def test_me_documents_the_passkey_times():
    from calimero_client_py.openapi.models import MeResponse200PasskeysItem

    doc = MeResponse200PasskeysItem.__doc__ or ""
    for field in ("added_at", "last_sign_in_at"):
        assert field in doc, field


def test_activity_list_takes_an_optional_after():
    sig = inspect.signature(Auth.activity_list)
    assert list(sig.parameters)[1:] == ["after"]
    assert sig.parameters["after"].default is UNSET
