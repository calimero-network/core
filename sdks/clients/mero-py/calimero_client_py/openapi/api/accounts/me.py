from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.me_response_200 import MeResponse200
from typing import cast


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/auth/me",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, MeResponse200]]:
    if response.status_code == 200:
        response_200 = MeResponse200.from_dict(response.json())

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
) -> Response[Union[Error, MeResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, MeResponse200]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: the one
    made at sign-up, where its instances go unless they name another; its approvals are that
    organization's), recovery state, and how the caller came (`via`).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, MeResponse200]]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, MeResponse200]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: the one
    made at sign-up, where its instances go unless they name another; its approvals are that
    organization's), recovery state, and how the caller came (`via`).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, MeResponse200]
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, MeResponse200]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: the one
    made at sign-up, where its instances go unless they name another; its approvals are that
    organization's), recovery state, and how the caller came (`via`).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, MeResponse200]]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, MeResponse200]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: the one
    made at sign-up, where its instances go unless they name another; its approvals are that
    organization's), recovery state, and how the caller came (`via`).

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, MeResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
