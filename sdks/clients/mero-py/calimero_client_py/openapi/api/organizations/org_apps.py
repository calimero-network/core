from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.org_apps_response_200 import OrgAppsResponse200
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    org: str,
    *,
    after: Union[Unset, str] = UNSET,
    limit: Union[Unset, int] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["after"] = after

    params["limit"] = limit

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/orgs/{org}/apps".format(
            org=org,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OrgAppsResponse200]]:
    if response.status_code == 200:
        response_200 = OrgAppsResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

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
) -> Response[Union[Error, OrgAppsResponse200]]:
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
    after: Union[Unset, str] = UNSET,
    limit: Union[Unset, int] = UNSET,
) -> Response[Union[Error, OrgAppsResponse200]]:
    """The organization's apps

     As `GET /auth/apps`, for the apps the caller reaches or manages; `404` to a non-member.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        after (Union[Unset, str]):
        limit (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgAppsResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        after=after,
        limit=limit,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    *,
    client: AuthenticatedClient,
    after: Union[Unset, str] = UNSET,
    limit: Union[Unset, int] = UNSET,
) -> Optional[Union[Error, OrgAppsResponse200]]:
    """The organization's apps

     As `GET /auth/apps`, for the apps the caller reaches or manages; `404` to a non-member.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        after (Union[Unset, str]):
        limit (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgAppsResponse200]
    """

    return sync_detailed(
        org=org,
        client=client,
        after=after,
        limit=limit,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
    after: Union[Unset, str] = UNSET,
    limit: Union[Unset, int] = UNSET,
) -> Response[Union[Error, OrgAppsResponse200]]:
    """The organization's apps

     As `GET /auth/apps`, for the apps the caller reaches or manages; `404` to a non-member.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        after (Union[Unset, str]):
        limit (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgAppsResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        after=after,
        limit=limit,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    *,
    client: AuthenticatedClient,
    after: Union[Unset, str] = UNSET,
    limit: Union[Unset, int] = UNSET,
) -> Optional[Union[Error, OrgAppsResponse200]]:
    """The organization's apps

     As `GET /auth/apps`, for the apps the caller reaches or manages; `404` to a non-member.

    **Who:** signed in (a session or an API key).

    Args:
        org (str):
        after (Union[Unset, str]):
        limit (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgAppsResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
            after=after,
            limit=limit,
        )
    ).parsed
