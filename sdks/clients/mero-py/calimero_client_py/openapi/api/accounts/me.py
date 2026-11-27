from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.account_view import AccountView
from ...models.error import Error
from typing import cast


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/auth/me",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AccountView, Error]]:
    if response.status_code == 200:
        response_200 = AccountView.from_dict(response.json())

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
) -> Response[Union[AccountView, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[AccountView, Error]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: one a
    sign-up made before cluster version 1.0.0-rc.32, an organization like any other from that version; a
    sign-up makes none from then on), recovery state, and how the caller came (`via`). A session opened
    on an instance's own site reads the account's id and name and that instance's organization alone:
    `passkeys` empty, no recovery state.

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AccountView, Error]]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AccountView, Error]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: one a
    sign-up made before cluster version 1.0.0-rc.32, an organization like any other from that version; a
    sign-up makes none from then on), recovery state, and how the caller came (`via`). A session opened
    on an instance's own site reads the account's id and name and that instance's organization alone:
    `passkeys` empty, no recovery state.

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AccountView, Error]
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[AccountView, Error]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: one a
    sign-up made before cluster version 1.0.0-rc.32, an organization like any other from that version; a
    sign-up makes none from then on), recovery state, and how the caller came (`via`). A session opened
    on an instance's own site reads the account's id and name and that instance's organization alone:
    `passkeys` empty, no recovery state.

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AccountView, Error]]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AccountView, Error]]:
    """The caller's account

     The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: one a
    sign-up made before cluster version 1.0.0-rc.32, an organization like any other from that version; a
    sign-up makes none from then on), recovery state, and how the caller came (`via`). A session opened
    on an instance's own site reads the account's id and name and that instance's organization alone:
    `passkeys` empty, no recovery state.

    **Who:** signed in (a session or an API key).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AccountView, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
