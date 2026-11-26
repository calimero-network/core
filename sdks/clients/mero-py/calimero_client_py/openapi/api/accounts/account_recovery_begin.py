from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.account_recovery_begin_body import AccountRecoveryBeginBody
from ...models.error import Error
from ...models.passkey_challenge import PasskeyChallenge
from typing import cast


def _get_kwargs(
    *,
    body: AccountRecoveryBeginBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/recovery/begin",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PasskeyChallenge]]:
    if response.status_code == 200:
        response_200 = PasskeyChallenge.from_dict(response.json())

        return response_200

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PasskeyChallenge]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryBeginBody,
) -> Response[Union[Error, PasskeyChallenge]]:
    """Start an account recovery

     Signed out, with the recovery code: creation options for the passkey that will replace the lost
    ones. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their
    shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (AccountRecoveryBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PasskeyChallenge]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryBeginBody,
) -> Optional[Union[Error, PasskeyChallenge]]:
    """Start an account recovery

     Signed out, with the recovery code: creation options for the passkey that will replace the lost
    ones. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their
    shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (AccountRecoveryBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PasskeyChallenge]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryBeginBody,
) -> Response[Union[Error, PasskeyChallenge]]:
    """Start an account recovery

     Signed out, with the recovery code: creation options for the passkey that will replace the lost
    ones. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their
    shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (AccountRecoveryBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PasskeyChallenge]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryBeginBody,
) -> Optional[Union[Error, PasskeyChallenge]]:
    """Start an account recovery

     Signed out, with the recovery code: creation options for the passkey that will replace the lost
    ones. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their
    shape is WebAuthn's.

    **Who:** anyone.

    Args:
        body (AccountRecoveryBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PasskeyChallenge]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
