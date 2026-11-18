"""
An organization's members' lockout risk (core's
`GET /orgs/{org}/members?detail=security`) through the facade:
`client.orgs.members(org=..., detail="security")`.

This checks the signature only; the route itself is exercised against a real
cluster in test_cloud_v2_api.py (skipped on a node without it).
"""

import inspect

from calimero_client_py.facade import Orgs


def test_members_takes_the_org_and_the_detail():
    params = list(inspect.signature(Orgs.members).parameters)
    assert params[1:] == ["org", "detail"]
