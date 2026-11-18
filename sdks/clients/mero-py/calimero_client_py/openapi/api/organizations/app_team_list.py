from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_team_list_response_200 import AppTeamListResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    org: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/orgs/{org}/app-teams".format(
            org=org,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppTeamListResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppTeamListResponse200.from_dict(response.json())

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
) -> Response[Union[AppTeamListResponse200, Error]]:
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
) -> Response[Union[AppTeamListResponse200, Error]]:
    """The organization's app teams

     A member: each app team's package, id, name, `join`, member count, default capabilities, and whether
    the caller is in it or manages it (a guest: only their own).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppTeamListResponse200, Error]]
    """

    kwargs = _get_kwargs(
        org=org,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppTeamListResponse200, Error]]:
    """The organization's app teams

     A member: each app team's package, id, name, `join`, member count, default capabilities, and whether
    the caller is in it or manages it (a guest: only their own).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppTeamListResponse200, Error]
    """

    return sync_detailed(
        org=org,
        client=client,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppTeamListResponse200, Error]]:
    """The organization's app teams

     A member: each app team's package, id, name, `join`, member count, default capabilities, and whether
    the caller is in it or manages it (a guest: only their own).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppTeamListResponse200, Error]]
    """

    kwargs = _get_kwargs(
        org=org,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppTeamListResponse200, Error]]:
    """The organization's app teams

     A member: each app team's package, id, name, `join`, member count, default capabilities, and whether
    the caller is in it or manages it (a guest: only their own).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppTeamListResponse200, Error]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
        )
    ).parsed
