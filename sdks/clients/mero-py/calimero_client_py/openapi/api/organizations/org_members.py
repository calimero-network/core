from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.org_members_detail import OrgMembersDetail
from ...models.org_members_response_200 import OrgMembersResponse200
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    org: str,
    *,
    detail: Union[Unset, OrgMembersDetail] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_detail: Union[Unset, str] = UNSET
    if not isinstance(detail, Unset):
        json_detail = detail.value

    params["detail"] = json_detail

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/orgs/{org}/members".format(
            org=org,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OrgMembersResponse200]]:
    if response.status_code == 200:
        response_200 = OrgMembersResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OrgMembersResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
    detail: Union[Unset, OrgMembersDetail] = UNSET,
) -> Response[Union[Error, OrgMembersResponse200]]:
    r"""Members' lockout risk

     `detail=security` (the only listing: without it, or with another value, `400 BAD_REQUEST`), the
    owner or an admin: each member's passkeys (`1`, `2` or `\"3+\"`), recovery code, last active UTC day
    and sessions. `403` below admin, `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        detail (Union[Unset, OrgMembersDetail]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgMembersResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        detail=detail,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    *,
    client: AuthenticatedClient,
    detail: Union[Unset, OrgMembersDetail] = UNSET,
) -> Optional[Union[Error, OrgMembersResponse200]]:
    r"""Members' lockout risk

     `detail=security` (the only listing: without it, or with another value, `400 BAD_REQUEST`), the
    owner or an admin: each member's passkeys (`1`, `2` or `\"3+\"`), recovery code, last active UTC day
    and sessions. `403` below admin, `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        detail (Union[Unset, OrgMembersDetail]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgMembersResponse200]
    """

    return sync_detailed(
        org=org,
        client=client,
        detail=detail,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
    detail: Union[Unset, OrgMembersDetail] = UNSET,
) -> Response[Union[Error, OrgMembersResponse200]]:
    r"""Members' lockout risk

     `detail=security` (the only listing: without it, or with another value, `400 BAD_REQUEST`), the
    owner or an admin: each member's passkeys (`1`, `2` or `\"3+\"`), recovery code, last active UTC day
    and sessions. `403` below admin, `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        detail (Union[Unset, OrgMembersDetail]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgMembersResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        detail=detail,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    *,
    client: AuthenticatedClient,
    detail: Union[Unset, OrgMembersDetail] = UNSET,
) -> Optional[Union[Error, OrgMembersResponse200]]:
    r"""Members' lockout risk

     `detail=security` (the only listing: without it, or with another value, `400 BAD_REQUEST`), the
    owner or an admin: each member's passkeys (`1`, `2` or `\"3+\"`), recovery code, last active UTC day
    and sessions. `403` below admin, `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        detail (Union[Unset, OrgMembersDetail]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgMembersResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
            detail=detail,
        )
    ).parsed
