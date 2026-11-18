from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.passkey_add_begin_response_200 import PasskeyAddBeginResponse200
from typing import cast


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/passkeys/add/begin",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PasskeyAddBeginResponse200]]:
    if response.status_code == 200:
        response_200 = PasskeyAddBeginResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PasskeyAddBeginResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, PasskeyAddBeginResponse200]]:
    """Start adding a passkey

     Creation options for another passkey (a device) on the caller's account. `options` are the WebAuthn
    options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PasskeyAddBeginResponse200]]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, PasskeyAddBeginResponse200]]:
    """Start adding a passkey

     Creation options for another passkey (a device) on the caller's account. `options` are the WebAuthn
    options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PasskeyAddBeginResponse200]
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, PasskeyAddBeginResponse200]]:
    """Start adding a passkey

     Creation options for another passkey (a device) on the caller's account. `options` are the WebAuthn
    options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PasskeyAddBeginResponse200]]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, PasskeyAddBeginResponse200]]:
    """Start adding a passkey

     Creation options for another passkey (a device) on the caller's account. `options` are the WebAuthn
    options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PasskeyAddBeginResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
