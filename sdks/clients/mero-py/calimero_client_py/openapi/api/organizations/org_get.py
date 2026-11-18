from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.org import Org
from typing import cast


def _get_kwargs(
    org: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/orgs/{org}".format(
            org=org,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, Org]]:
    if response.status_code == 200:
        response_200 = Org.from_dict(response.json())

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
) -> Response[Union[Error, Org]]:
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
) -> Response[Union[Error, Org]]:
    """One organization

     As the directory records it, with only the teams the caller reaches or manages and the people they
    see; and its `mode` (from its app teams), the accounts that may approve its releases
    (`releaseApprovers`), the keys it publishes under (`publishes`), and `newWorkStopped`: the release
    whose passed deadline stops new work in it (`after`, RFC 3339), `null` when none does. Those four
    are absent from a node older than they are (1.0.0-rc.1 sends none of them).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Org]]
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
) -> Optional[Union[Error, Org]]:
    """One organization

     As the directory records it, with only the teams the caller reaches or manages and the people they
    see; and its `mode` (from its app teams), the accounts that may approve its releases
    (`releaseApprovers`), the keys it publishes under (`publishes`), and `newWorkStopped`: the release
    whose passed deadline stops new work in it (`after`, RFC 3339), `null` when none does. Those four
    are absent from a node older than they are (1.0.0-rc.1 sends none of them).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Org]
    """

    return sync_detailed(
        org=org,
        client=client,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, Org]]:
    """One organization

     As the directory records it, with only the teams the caller reaches or manages and the people they
    see; and its `mode` (from its app teams), the accounts that may approve its releases
    (`releaseApprovers`), the keys it publishes under (`publishes`), and `newWorkStopped`: the release
    whose passed deadline stops new work in it (`after`, RFC 3339), `null` when none does. Those four
    are absent from a node older than they are (1.0.0-rc.1 sends none of them).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Org]]
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
) -> Optional[Union[Error, Org]]:
    """One organization

     As the directory records it, with only the teams the caller reaches or manages and the people they
    see; and its `mode` (from its app teams), the accounts that may approve its releases
    (`releaseApprovers`), the keys it publishes under (`publishes`), and `newWorkStopped`: the release
    whose passed deadline stops new work in it (`after`, RFC 3339), `null` when none does. Those four
    are absent from a node older than they are (1.0.0-rc.1 sends none of them).

    **Who:** signed in (a session or an API key).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Org]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
        )
    ).parsed
